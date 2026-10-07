# SPDX-License-Identifier: AGPL-3.0-only

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $TaskRoot,

    [Parameter(Mandatory)]
    [string] $TestRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:CaseCount = 0
$script:CurrentCase = $null
$script:Completed = $false

function Assert-True {
    param(
        [Parameter(Mandatory)][bool] $Condition,
        [Parameter(Mandatory)][string] $Message
    )
    if (-not $Condition) { throw $Message }
}

function Complete-Case {
    param([Parameter(Mandatory)][string] $Name)
    $script:CaseCount++
    Write-Output "PASS $Name"
}

function Invoke-FailCase {
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][scriptblock] $Action,
        [Parameter(Mandatory)][string] $MessagePattern
    )
    $script:CurrentCase = $Name
    try {
        & $Action | Out-Null
    }
    catch {
        if ($_.Exception.Message -notmatch $MessagePattern) {
            throw "case '$Name' failed for the wrong reason: $($_.Exception.Message)"
        }
        Complete-Case -Name $Name
        return
    }
    throw "case '$Name' unexpectedly succeeded"
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string] $Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-PpocrModelContentSha256 {
    param(
        [Parameter(Mandatory)][string] $Detector,
        [Parameter(Mandatory)][string] $Recognizer,
        [Parameter(Mandatory)][string] $Dictionary
    )
    $text = "actingcommand.ppocr-model-set.v1`0detector`0$Detector`0recognizer`0$Recognizer`0dictionary`0$Dictionary`0classifier`0none`0"
    $hasher = [Security.Cryptography.SHA256]::Create()
    try {
        $digest = $hasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($text))
    }
    finally {
        $hasher.Dispose()
    }
    ([BitConverter]::ToString($digest) -replace '-', '').ToLowerInvariant()
}

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Text
    )
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function Write-JsonFixture {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)] $Value
    )
    Write-Utf8NoBom -Path $Path -Text (($Value | ConvertTo-Json -Depth 40) + "`n")
    Get-Sha256 -Path $Path
}

function New-ZipFixture {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][object[]] $Entries
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    $file = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite)
    $archive = [IO.Compression.ZipArchive]::new(
        $file,
        [IO.Compression.ZipArchiveMode]::Create,
        $false
    )
    try {
        foreach ($specification in $Entries) {
            $entry = $archive.CreateEntry(
                [string]$specification.name,
                [IO.Compression.CompressionLevel]::NoCompression
            )
            if ($null -ne $specification.PSObject.Properties['symlink'] -and
                [bool]$specification.symlink) {
                $entry.ExternalAttributes = -1577123840 # 0xA1FF0000: Unix symlink with 0777 mode.
            }
            if ($null -ne $specification.PSObject.Properties['content'] -and
                $null -ne $specification.content) {
                $bytes = [Text.Encoding]::UTF8.GetBytes([string]$specification.content)
                $output = $entry.Open()
                try {
                    $output.Write($bytes, 0, $bytes.Length)
                }
                finally {
                    $output.Dispose()
                }
            }
        }
    }
    finally {
        $archive.Dispose()
        $file.Dispose()
    }
    Get-Item -LiteralPath $Path -ErrorAction Stop
}

function New-ZipFixtureManifest {
    param(
        [Parameter(Mandatory)][string] $SourcesManifestPath,
        [Parameter(Mandatory)][string] $FixtureRoot,
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $ArchivePath,
        [Parameter(Mandatory)][int] $MaximumFiles,
        [Parameter(Mandatory)][long] $MaximumBytes,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $RequiredFiles,
        [string[]] $AllowedFiles
    )
    $manifest = Get-Content -LiteralPath $SourcesManifestPath -Raw | ConvertFrom-Json -Depth 100
    $definition = $manifest.components.'platform-tools-37.0.1'
    $archive = Get-Item -LiteralPath $ArchivePath -ErrorAction Stop
    $definition.version = 'synthetic-zip-fixture-v1'
    $definition.archive.url = "https://example.invalid/$Name.zip"
    $definition.archive.relative_path = "downloads/$Name.zip"
    $definition.archive.size = [long]$archive.Length
    $definition.archive.sha256 = Get-Sha256 -Path $archive.FullName
    $definition.extract_relative_path = "fixtures/$Name"
    $definition.max_extract_file_count = $MaximumFiles
    $definition.max_extract_bytes = $MaximumBytes
    $definition.required_files = @($RequiredFiles)
    if ($PSBoundParameters.ContainsKey('AllowedFiles')) {
        $definition | Add-Member -NotePropertyName extract_allowlist -NotePropertyValue @($AllowedFiles) -Force
    } else {
        $definition.PSObject.Properties.Remove('extract_allowlist')
    }
    $manifest.cache_layout.directory_name = "windows-tools-$Name"
    $manifestPath = Join-Path $FixtureRoot "$Name.manifest.json"
    [void](Write-JsonFixture -Path $manifestPath -Value $manifest)
    $manifestPath
}

function Invoke-ZipFixtureMaterializer {
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $ArchivePath,
        [Parameter(Mandatory)][int] $MaximumFiles,
        [Parameter(Mandatory)][long] $MaximumBytes,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $RequiredFiles,
        [string[]] $AllowedFiles
    )
    $manifestArguments = @{
        SourcesManifestPath = $sourcesManifest
        FixtureRoot = $zipFixtureRoot
        Name = $Name
        ArchivePath = $ArchivePath
        MaximumFiles = $MaximumFiles
        MaximumBytes = $MaximumBytes
        RequiredFiles = $RequiredFiles
    }
    if ($PSBoundParameters.ContainsKey('AllowedFiles')) {
        $manifestArguments.AllowedFiles = $AllowedFiles
    }
    $fixtureManifest = New-ZipFixtureManifest @manifestArguments
    & $materializer `
        -TaskRoot $testRootFull `
        -CacheRoot (Join-Path $testRootFull "cache/$Name") `
        -Component platform-tools-37.0.1 `
        -SourcesManifestPath $fixtureManifest `
        -AcceptAndroidSdkLicense `
        -PrivateDownloadSourcePath $ArchivePath `
        -PrivateDownloadDeadlineMilliseconds 5000
}

function New-ArtifactFixture {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $Mode,
        [Parameter(Mandatory)][string] $ArtifactName,
        [Parameter(Mandatory)][ValidateSet('Runtime', 'Tools')][string] $ArtifactKind,
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][string] $CommitSha,
        [Parameter(Mandatory)][string] $TreeSha,
        [Parameter(Mandatory)][string] $CargoLockSha256,
        [Parameter(Mandatory)][bool] $CorruptPayload,
        [ValidateSet('platform-tools-v2', 'platform-tools-v3')][string] $ToolsLayout = 'platform-tools-v3'
    )
    $directory = Join-Path $Root "artifacts/$Mode/$ArtifactName"
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $payloads = if ($ArtifactKind -ceq 'Runtime') {
        @(
            @{ name = 'actingcommand-actingd.exe'; content = 'synthetic actingd payload' },
            @{ name = 'actingctl.exe'; content = 'synthetic actingctl payload' },
            @{ name = 'actingd.config.example.json'; content = '{"schema_version":"actingcommand.actingd.config.v1","state_root":"","bind_host":"127.0.0.1","bind_port":0,"secret_fingerprint_salt":"","instances":[]}' },
            @{ name = 'INSTALL.md'; content = 'synthetic installation instructions' },
            @{ name = 'RELEASE-NOTES.md'; content = 'synthetic unreleased candidate notes' }
        )
    } else {
        @(
            @{ name = 'actinglab.exe'; content = 'synthetic actinglab payload' },
            @{ name = 'actingledger.exe'; content = 'synthetic actingledger payload' },
            @{ name = 'actingcommand-vision-provider-check.exe'; content = 'synthetic provider-check payload' },
            @{ name = 'actingcommand-device-test.exe'; content = 'synthetic device-test payload' }
        ) + @(
            if ($ToolsLayout -ceq 'platform-tools-v3') {
                @{ name = 'actingwatch.exe'; content = 'synthetic watchdog launcher payload' }
            }
        ) + @(
            @{ name = 'platform-tools/adb.exe'; content = 'synthetic adb payload' },
            @{ name = 'platform-tools/AdbWinApi.dll'; content = 'synthetic AdbWinApi payload' },
            @{ name = 'platform-tools/AdbWinUsbApi.dll'; content = 'synthetic AdbWinUsbApi payload' },
            @{ name = 'platform-tools/NOTICE.txt'; content = 'synthetic platform-tools notice' },
            @{ name = 'platform-tools/source.properties'; content = 'Pkg.Revision=37.0.1' }
        )
    }
    $records = @()
    foreach ($payload in $payloads) {
        $path = Join-Path $directory $payload.name
        Write-Utf8NoBom -Path $path -Text $payload.content
        $item = Get-Item -LiteralPath $path
        $records += [ordered]@{
            path = $payload.name
            size_bytes = [int64]$item.Length
            sha256 = Get-Sha256 -Path $path
        }
    }
    $manifest = [ordered]@{
        repository = $Repository
        commit_sha = $CommitSha
        tree_sha = $TreeSha
        cargo_lock_sha256 = $CargoLockSha256
        rust_toolchain = "stable-x86_64-pc-windows-msvc`nrustc 1.test.0 (fixture)"
        target = 'x86_64-pc-windows-msvc'
        configuration = 'release'
        workflow_run_id = 101
        workflow_run_attempt = 1
        source_artifact_name = $ArtifactName
        files = $records
    }
    if ($ArtifactKind -ceq 'Runtime') {
        $manifest.runtime_payload_layout = 'distribution-v1'
    } else {
        $manifest.tools_payload_layout = $ToolsLayout
    }
    Write-Utf8NoBom -Path (Join-Path $directory 'BUILD-MANIFEST.json') -Text (($manifest | ConvertTo-Json -Depth 8) + "`n")
    if ($CorruptPayload) {
        $corruptName = if ($ArtifactKind -ceq 'Runtime') { 'actingctl.exe' } else { 'actinglab.exe' }
        Add-Content -LiteralPath (Join-Path $directory $corruptName) -Value 'corrupt' -NoNewline
    }
}

function New-FakeGh {
    param([Parameter(Mandatory)][string] $Root)
    $scriptPath = Join-Path $Root 'fake-gh.ps1'
    $commandPath = Join-Path $Root 'fake-gh.cmd'
    $scriptText = @'
$ErrorActionPreference = 'Stop'
$root = $env:ACTINGCOMMAND_FAKE_GH_ROOT
$mode = $env:ACTINGCOMMAND_FAKE_GH_MODE
$sourceSha = $env:ACTINGCOMMAND_FAKE_GH_SOURCE_SHA
$treeSha = $env:ACTINGCOMMAND_FAKE_GH_TREE_SHA
$repository = $env:ACTINGCOMMAND_FAKE_GH_REPOSITORY
$scriptArgs = @($args)

function Value-After([string] $Name) {
    $index = [Array]::IndexOf($scriptArgs, $Name)
    if ($index -lt 0 -or $index + 1 -ge $scriptArgs.Count) { throw "missing $Name" }
    $scriptArgs[$index + 1]
}

if ($args.Count -ge 2 -and $args[0] -ceq 'run' -and $args[1] -ceq 'list') {
    $run = [ordered]@{
        databaseId = 101; headSha = $sourceSha; status = 'completed'; conclusion = 'success'
        workflowName = 'Windows exact-SHA build'; attempt = 1; url = 'https://example.invalid/run/101'
    }
    if ($mode -ceq 'ambiguous-run') { @($run, ([ordered]@{ databaseId = 102; headSha = $sourceSha; status = 'completed'; conclusion = 'success'; workflowName = 'Windows exact-SHA build'; attempt = 1; url = 'https://example.invalid/run/102' })) | ConvertTo-Json -Compress }
    else { @($run) | ConvertTo-Json -Compress }
    exit 0
}
if ($args.Count -ge 2 -and $args[0] -ceq 'run' -and $args[1] -ceq 'view') {
    [ordered]@{ databaseId = 101; headSha = $sourceSha; status = 'completed'; conclusion = 'success'; workflowName = 'Windows exact-SHA build'; attempt = 1; url = 'https://example.invalid/run/101' } | ConvertTo-Json -Compress
    exit 0
}
if ($args.Count -ge 2 -and $args[0] -ceq 'api') {
    $endpoint = [string]$args[1]
    if ($endpoint -like '*/git/commits/*') {
        [ordered]@{ sha = $sourceSha; tree = [ordered]@{ sha = $treeSha } } | ConvertTo-Json -Compress
        exit 0
    }
    if ($endpoint -like '*/contents/Cargo.lock*') {
        $bytes = [IO.File]::ReadAllBytes((Join-Path $root 'Cargo.lock'))
        [ordered]@{ encoding = 'base64'; content = [Convert]::ToBase64String($bytes) } | ConvertTo-Json -Compress
        exit 0
    }
    if ($endpoint -like '*/actions/runs/*/artifacts*') {
        [object[]]$artifacts = if ($mode -ceq 'missing-artifact') {
            @()
        } else {
            @(
                [pscustomobject][ordered]@{ name = "actingcommand-runtime-$sourceSha"; expired = $false },
                [pscustomobject][ordered]@{ name = "actingcommand-tools-$sourceSha"; expired = $false }
            )
        }
        [ordered]@{ total_count = [int]$artifacts.Count; artifacts = $artifacts } | ConvertTo-Json -Depth 4 -Compress
        exit 0
    }
}
if ($args.Count -ge 2 -and $args[0] -ceq 'run' -and $args[1] -ceq 'download') {
    $name = Value-After '--name'
    $destination = Value-After '--dir'
    $source = Join-Path $root "artifacts/$mode/$name"
    if (-not (Test-Path -LiteralPath $source -PathType Container)) { throw "fixture artifact is missing: $source" }
    Get-ChildItem -LiteralPath $source -File -Recurse | ForEach-Object {
        $target = Join-Path $destination ([IO.Path]::GetRelativePath($source, $_.FullName))
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        Copy-Item -LiteralPath $_.FullName -Destination $target
    }
    exit 0
}
Write-Error "unsupported fake gh arguments: $($args -join ' ')"
exit 2
'@
    Write-Utf8NoBom -Path $scriptPath -Text $scriptText
    Write-Utf8NoBom -Path $commandPath -Text "@echo off`r`npwsh.exe -NoLogo -NoProfile -File `"%~dp0fake-gh.ps1`" %*`r`n"
    $commandPath
}

$taskRootFull = [IO.Path]::GetFullPath($TaskRoot).TrimEnd('\')
$testRootFull = [IO.Path]::GetFullPath($TestRoot).TrimEnd('\')
if ([IO.Path]::GetPathRoot($taskRootFull) -cne 'D:\' -or
    -not (Test-Path -LiteralPath $taskRootFull -PathType Container)) {
    throw 'TaskRoot must be an existing D-drive directory'
}
$taskPrefix = $taskRootFull + '\'
if (-not $testRootFull.StartsWith($taskPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'TestRoot must be a strict child of TaskRoot'
}
if (Test-Path -LiteralPath $testRootFull) {
    throw 'TestRoot must not already exist'
}
New-Item -ItemType Directory -Path $testRootFull | Out-Null

try {
    $repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
    $downloader = Join-Path $PSScriptRoot 'Get-ExactBuildArtifact.ps1'
    $materializer = Join-Path $PSScriptRoot 'Materialize-TaskToolCache.ps1'
    $sourcesManifest = Join-Path $PSScriptRoot 'windows-tool-sources.v1.json'
    $workflow = Join-Path $repoRoot '.github/workflows/windows-remote-build.yml'

    $script:CurrentCase = 'parse-and-workflow-structure'
    foreach ($path in @($downloader, $materializer, $PSCommandPath)) {
        $tokens = $null
        $errors = $null
        [void][Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
        Assert-True -Condition ($errors.Count -eq 0) -Message "PowerShell parser errors in $path"
    }
    $workflowText = Get-Content -LiteralPath $workflow -Raw
    foreach ($required in @(
        'name: Windows exact-SHA build',
        'actingcommand-runtime-$env:SOURCE_SHA',
        'actingcommand-tools-$env:SOURCE_SHA',
        'BUILD-MANIFEST.json',
        "'\A[0-9a-f]{40}\z'",
        "'x86_64-pc-windows-msvc'",
        "github.event_name == 'pull_request' && 7 || 30"
    )) {
        Assert-True -Condition $workflowText.Contains($required) -Message "workflow is missing '$required'"
    }
    $runtimeSplit = '\$runtimeFiles\s*=\s*@\(\s*''actingcommand-actingd\.exe'',\s*''actingctl\.exe'',\s*''actingd\.config\.example\.json'',\s*''INSTALL\.md'',\s*''RELEASE-NOTES\.md''\s*\)'
    $toolsSplit = '\$toolFiles\s*=\s*@\(\s*''actinglab\.exe'',\s*''actingledger\.exe'',\s*''actingcommand-vision-provider-check\.exe'',\s*''actingcommand-device-test\.exe'',\s*''actingwatch\.exe''\s*\)'
    Assert-True -Condition ([regex]::IsMatch($workflowText, $runtimeSplit)) -Message 'workflow Runtime artifact split is not exact'
    Assert-True -Condition ([regex]::IsMatch($workflowText, $toolsSplit)) -Message 'workflow Tools artifact split is not exact'
    foreach ($retired in @(
        '--package actingcommand-ppocr-onnx-json-provider',
        'actingcommand_ppocr_onnx_json_provider.dll',
        'ac_fastdeploy_ppocr.dll'
    )) {
        Assert-True -Condition (-not $workflowText.Contains($retired)) -Message "workflow still builds or stages the retired vision provider: '$retired'"
    }
    Assert-True -Condition $workflowText.Contains("'platform-tools-v3'") -Message 'workflow Tools manifest does not declare platform-tools-v3'
    foreach ($field in @(
        'repository', 'commit_sha', 'tree_sha', 'cargo_lock_sha256', 'rust_toolchain',
        'target', 'configuration', 'workflow_run_id', 'workflow_run_attempt',
        'source_artifact_name', 'runtime_payload_layout', 'files', 'path', 'size_bytes', 'sha256'
    )) {
        Assert-True -Condition $workflowText.Contains("$field =") -Message "workflow manifest is missing field '$field'"
    }
    $uses = @([regex]::Matches($workflowText, '(?m)^\s*uses:\s*([^\s#]+)') | ForEach-Object { $_.Groups[1].Value })
    Assert-True -Condition ($uses.Count -eq 3) -Message 'workflow must contain exactly three pinned action uses'
    foreach ($use in $uses) {
        Assert-True -Condition ($use -cmatch '@[0-9a-f]{40}$') -Message "workflow action is not full-SHA pinned: $use"
    }
    $sourceManifest = Get-Content -LiteralPath $sourcesManifest -Raw | ConvertFrom-Json -Depth 100
    Assert-True -Condition ($sourceManifest.schema_version -ceq 'actingcommand.windows_tool_sources.v1') -Message 'tool source schema mismatch'
    Assert-True -Condition ($sourceManifest.components.'ppocrv6-medium-source'.compatibility.state -ceq 'PendingVerification') -Message 'Paddle source archives must retain the explicit conversion boundary'
    $ort = $sourceManifest.components.'onnxruntime-gpu-1.24.4'
    Assert-True -Condition ($ort.version -ceq 'v1.24.4') -Message 'ONNX Runtime version is not frozen'
    Assert-True -Condition ([long]$ort.archive.size -eq 280958859) -Message 'ONNX Runtime archive size is not frozen'
    Assert-True -Condition ($ort.archive.sha256 -ceq 'ef3337a0b8184eb8beec310f7c83bd50376b3eefc43aab84ac8e452f6987df0a') -Message 'ONNX Runtime archive SHA-256 is not frozen'
    Assert-True -Condition (@($ort.extract_allowlist).Count -eq 3) -Message 'ONNX Runtime extraction allowlist is not exact'
    Assert-True -Condition ($null -eq $sourceManifest.components.PSObject.Properties['provider-v0.3']) -Message 'the retired provider-v0.3 component is still declared'
    Complete-Case -Name $script:CurrentCase

    $fixtureRoot = Join-Path $testRootFull 'fake-gh'
    New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
    $repository = 'HS7097/ActingCommand-Runtime'
    $sourceSha = '0123456789abcdef0123456789abcdef01234567'
    $treeSha = '89abcdef0123456789abcdef0123456789abcdef'
    $runtimeArtifactName = "actingcommand-runtime-$sourceSha"
    $toolsArtifactName = "actingcommand-tools-$sourceSha"
    Write-Utf8NoBom -Path (Join-Path $fixtureRoot 'Cargo.lock') -Text "fixture-lock`n"
    $lockSha = Get-Sha256 -Path (Join-Path $fixtureRoot 'Cargo.lock')
    New-ArtifactFixture -Root $fixtureRoot -Mode 'success' -ArtifactName $runtimeArtifactName -ArtifactKind Runtime -Repository $repository -CommitSha $sourceSha -TreeSha $treeSha -CargoLockSha256 $lockSha -CorruptPayload $false
    New-ArtifactFixture -Root $fixtureRoot -Mode 'success' -ArtifactName $toolsArtifactName -ArtifactKind Tools -Repository $repository -CommitSha $sourceSha -TreeSha $treeSha -CargoLockSha256 $lockSha -CorruptPayload $false
    New-ArtifactFixture -Root $fixtureRoot -Mode 'wrong-hash' -ArtifactName $runtimeArtifactName -ArtifactKind Runtime -Repository $repository -CommitSha $sourceSha -TreeSha $treeSha -CargoLockSha256 $lockSha -CorruptPayload $true
    $fakeGh = New-FakeGh -Root $fixtureRoot
    $env:ACTINGCOMMAND_FAKE_GH_ROOT = $fixtureRoot
    $env:ACTINGCOMMAND_FAKE_GH_SOURCE_SHA = $sourceSha
    $env:ACTINGCOMMAND_FAKE_GH_TREE_SHA = $treeSha
    $env:ACTINGCOMMAND_FAKE_GH_REPOSITORY = $repository

    $script:CurrentCase = 'artifact-positive-exact-selection'
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'success'
    $positiveOutput = Join-Path $testRootFull 'downloads/positive'
    $positiveJson = & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath $positiveOutput -GhExecutable $fakeGh
    $positive = $positiveJson | ConvertFrom-Json -Depth 20
    Assert-True -Condition ($positive.status -ceq 'PASS') -Message 'positive artifact verification did not report PASS'
    foreach ($name in @('actingcommand-actingd.exe', 'actingctl.exe', 'actingd.config.example.json', 'INSTALL.md', 'RELEASE-NOTES.md')) {
        Assert-True -Condition (
            @($positive.verified_files).Count -eq 5 -and
            (Test-Path -LiteralPath (Join-Path $positiveOutput $name) -PathType Leaf)
        ) -Message "positive Runtime distribution payload was not published: $name"
    }
    Complete-Case -Name $script:CurrentCase

    $script:CurrentCase = 'artifact-tools-positive-exact-selection'
    $toolsOutput = Join-Path $testRootFull 'downloads/tools-positive'
    $toolsJson = & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Tools -TaskRoot $testRootFull -OutputPath $toolsOutput -GhExecutable $fakeGh
    $tools = $toolsJson | ConvertFrom-Json -Depth 20
    Assert-True -Condition ($tools.status -ceq 'PASS') -Message 'Tools artifact verification did not report PASS'
    Assert-True -Condition (@($tools.verified_files).Count -eq 10) -Message 'Tools artifact verifier did not freeze exactly ten platform-tools-v3 payloads'
    $adbFixture = Get-Item -LiteralPath (Join-Path $toolsOutput 'platform-tools/adb.exe') -ErrorAction Stop
    Assert-True -Condition ($adbFixture.Length -gt 0) -Message 'Tools artifact platform-tools payload is missing or empty'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $toolsOutput 'ac_fastdeploy_ppocr.dll'))) -Message 'Tools artifact still carries the retired vision provider'
    Assert-True -Condition (Test-Path -LiteralPath (Join-Path $toolsOutput 'actingwatch.exe') -PathType Leaf) -Message 'platform-tools-v3 Tools artifact lacks actingwatch.exe'
    Complete-Case -Name $script:CurrentCase

    # Workflow #374: the historical platform-tools-v2 layout (no actingwatch.exe) stays accepted.
    $script:CurrentCase = 'artifact-tools-v2-historical-layout'
    New-ArtifactFixture -Root $fixtureRoot -Mode 'tools-v2' -ArtifactName $toolsArtifactName -ArtifactKind Tools -Repository $repository -CommitSha $sourceSha -TreeSha $treeSha -CargoLockSha256 $lockSha -CorruptPayload $false -ToolsLayout 'platform-tools-v2'
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'tools-v2'
    $toolsV2Output = Join-Path $testRootFull 'downloads/tools-v2'
    $toolsV2 = (& $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Tools -TaskRoot $testRootFull -OutputPath $toolsV2Output -GhExecutable $fakeGh) | ConvertFrom-Json -Depth 20
    Assert-True -Condition ($toolsV2.status -ceq 'PASS') -Message 'historical platform-tools-v2 Tools artifact was not accepted'
    Assert-True -Condition (@($toolsV2.verified_files).Count -eq 9) -Message 'platform-tools-v2 Tools artifact did not verify exactly nine payloads'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $toolsV2Output 'actingwatch.exe'))) -Message 'platform-tools-v2 Tools artifact carries actingwatch.exe'
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'success'
    Complete-Case -Name $script:CurrentCase

    Invoke-FailCase -Name 'artifact-wrong-sha' -MessagePattern 'found 0' -Action {
        & $downloader -Repository $repository -SourceSha '1123456789abcdef0123456789abcdef01234567' -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath (Join-Path $testRootFull 'downloads/wrong-sha') -GhExecutable $fakeGh
    }
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'missing-artifact'
    Invoke-FailCase -Name 'artifact-missing-artifact' -MessagePattern 'found 0' -Action {
        & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath (Join-Path $testRootFull 'downloads/missing') -GhExecutable $fakeGh
    }
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'ambiguous-run'
    Invoke-FailCase -Name 'artifact-ambiguous-run' -MessagePattern 'found 2' -Action {
        & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath (Join-Path $testRootFull 'downloads/ambiguous') -GhExecutable $fakeGh
    }
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'wrong-hash'
    Invoke-FailCase -Name 'artifact-payload-hash-mismatch' -MessagePattern 'size mismatch|SHA-256 mismatch' -Action {
        & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath (Join-Path $testRootFull 'downloads/wrong-hash') -GhExecutable $fakeGh
    }
    $env:ACTINGCOMMAND_FAKE_GH_MODE = 'success'
    Invoke-FailCase -Name 'artifact-output-outside-task-root' -MessagePattern 'strict child' -Action {
        & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath 'D:\outside-issue194-test-output' -GhExecutable $fakeGh
    }
    Invoke-FailCase -Name 'artifact-output-overwrite' -MessagePattern 'overwrite is prohibited' -Action {
        & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Runtime -TaskRoot $testRootFull -OutputPath $positiveOutput -GhExecutable $fakeGh
    }

    $zipFixtureRoot = Join-Path $testRootFull 'zip-fixtures'
    New-Item -ItemType Directory -Path $zipFixtureRoot -Force | Out-Null

    $script:CurrentCase = 'materializer-empty-zip-root-placeholder-positive'
    $emptyRootArchive = Join-Path $zipFixtureRoot 'empty-root-positive.zip'
    $emptyRootFixtureBase64 = 'UEsDBBQAAAAAANmVGF0AAAAAAAAAAAAAAAAAAAAAUEsDBBQAAAAAANmVGF0AAAAAAAAAAAAAAAAIAAAAcGF5bG9hZC9QSwMEFAAAAAAA2ZUYXeg6aiAMAAAADAAAABAAAABwYXlsb2FkL3Rvb2wuYmluZml4dHVyZS10b29sUEsDBBQAAAAAANmVGF1GZmPdDgAAAA4AAAASAAAAcGF5bG9hZC9jb25maWcudHh0Zml4dHVyZS1jb25maWdQSwECFAAUAAAAAADZlRhdAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAEAAAAAUEsBAhQAFAAAAAAA2ZUYXQAAAAAAAAAAAAAAAAgAAAAAAAAAAAAQAP1BHgAAAHBheWxvYWQvUEsBAhQAFAAAAAAA2ZUYXeg6aiAMAAAADAAAABAAAAAAAAAAAAAAAIABRAAAAHBheWxvYWQvdG9vbC5iaW5QSwECFAAUAAAAAADZlRhdRmZj3Q4AAAAOAAAAEgAAAAAAAAAAAAAAgAF+AAAAcGF5bG9hZC9jb25maWcudHh0UEsFBgAAAAAEAAQA4gAAALwAAAAAAA=='
    [IO.File]::WriteAllBytes($emptyRootArchive, [Convert]::FromBase64String($emptyRootFixtureBase64))
    $emptyRootResult = Invoke-ZipFixtureMaterializer `
        -Name 'empty-root-positive' `
        -ArchivePath $emptyRootArchive `
        -MaximumFiles 2 `
        -MaximumBytes 26 `
        -RequiredFiles @('payload/tool.bin', 'payload/config.txt')
    Assert-True -Condition ($emptyRootResult.state -ceq 'Ready') -Message 'empty ZIP root fixture was not Ready'
    $emptyRootProvenance = Get-Content -LiteralPath $emptyRootResult.provenance_path -Raw | ConvertFrom-Json -Depth 100
    $emptyRootComponent = $emptyRootProvenance.components.'platform-tools-37.0.1'
    Assert-True -Condition ([long]$emptyRootComponent.extracted_bytes -eq 26) -Message 'empty ZIP root placeholder consumed byte budget'
    Assert-True -Condition (@($emptyRootComponent.extracted_files).Count -eq 2) -Message 'empty ZIP root placeholder consumed file budget'
    $emptyRootOutput = Join-Path $emptyRootResult.cache_root 'fixtures/empty-root-positive'
    $emptyRootFiles = @(Get-ChildItem -LiteralPath $emptyRootOutput -Recurse -Force -File)
    $emptyRootDirectories = @(Get-ChildItem -LiteralPath $emptyRootOutput -Recurse -Force -Directory)
    Assert-True -Condition ($emptyRootFiles.Count -eq 2) -Message 'empty ZIP root placeholder created an output file'
    Assert-True -Condition ($emptyRootDirectories.Count -eq 1 -and $emptyRootDirectories[0].Name -ceq 'payload') -Message 'empty ZIP root placeholder created an output directory'
    Assert-True -Condition ((Get-Content -LiteralPath (Join-Path $emptyRootOutput 'payload/tool.bin') -Raw) -ceq 'fixture-tool') -Message 'declared ZIP tool payload changed'
    Assert-True -Condition ((Get-Content -LiteralPath (Join-Path $emptyRootOutput 'payload/config.txt') -Raw) -ceq 'fixture-config') -Message 'declared ZIP config payload changed'
    Complete-Case -Name $script:CurrentCase

    $nonemptyUnnamedArchive = Join-Path $zipFixtureRoot 'nonempty-unnamed.zip'
    [IO.File]::WriteAllBytes(
        $nonemptyUnnamedArchive,
        [Convert]::FromBase64String('UEsDBBQAAAAAAJqVGF2G1JN2CQAAAAkAAAAAAAAAbm90LWVtcHR5UEsBAhQAFAAAAAAAmpUYXYbUk3YJAAAACQAAAAAAAAAAAAAAAAAAAIABAAAAAFBLBQYAAAAAAQABAC4AAAAnAAAAAAA=')
    )
    Invoke-FailCase -Name 'materializer-nonempty-unnamed-zip-entry' -MessagePattern 'unnamed root entry must be empty' -Action {
        Invoke-ZipFixtureMaterializer -Name 'nonempty-unnamed' -ArchivePath $nonemptyUnnamedArchive -MaximumFiles 1 -MaximumBytes 16 -RequiredFiles @() | Out-Null
    }

    $traversalDirectoryArchive = Join-Path $zipFixtureRoot 'traversal-directory.zip'
    New-ZipFixture -Path $traversalDirectoryArchive -Entries @(
        [pscustomobject]@{ name = '../escape/'; content = $null }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-traversal-directory' -MessagePattern 'unsafe path segment|escapes its controlled root' -Action {
        Invoke-ZipFixtureMaterializer -Name 'traversal-directory' -ArchivePath $traversalDirectoryArchive -MaximumFiles 1 -MaximumBytes 1 -RequiredFiles @() | Out-Null
    }

    $traversalFileArchive = Join-Path $zipFixtureRoot 'traversal-file.zip'
    New-ZipFixture -Path $traversalFileArchive -Entries @(
        [pscustomobject]@{ name = '../escape.txt'; content = 'x' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-traversal-file' -MessagePattern 'unsafe path segment|escapes its controlled root' -Action {
        Invoke-ZipFixtureMaterializer -Name 'traversal-file' -ArchivePath $traversalFileArchive -MaximumFiles 1 -MaximumBytes 1 -RequiredFiles @() | Out-Null
    }
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $testRootFull 'escape.txt'))) -Message 'traversal ZIP fixture wrote outside the extraction root'

    $absolutePathArchive = Join-Path $zipFixtureRoot 'absolute-path.zip'
    New-ZipFixture -Path $absolutePathArchive -Entries @(
        [pscustomobject]@{ name = 'C:/outside.txt'; content = 'x' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-absolute-path' -MessagePattern 'non-empty relative path' -Action {
        Invoke-ZipFixtureMaterializer -Name 'absolute-path' -ArchivePath $absolutePathArchive -MaximumFiles 1 -MaximumBytes 1 -RequiredFiles @() | Out-Null
    }

    $symlinkArchive = Join-Path $zipFixtureRoot 'symlink.zip'
    New-ZipFixture -Path $symlinkArchive -Entries @(
        [pscustomobject]@{ name = 'payload/link'; content = 'target'; symlink = $true }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-symlink' -MessagePattern 'symbolic link' -Action {
        Invoke-ZipFixtureMaterializer -Name 'symlink' -ArchivePath $symlinkArchive -MaximumFiles 1 -MaximumBytes 6 -RequiredFiles @() | Out-Null
    }

    $duplicateArchive = Join-Path $zipFixtureRoot 'duplicate.zip'
    New-ZipFixture -Path $duplicateArchive -Entries @(
        [pscustomobject]@{ name = 'payload/value.txt'; content = 'a' },
        [pscustomobject]@{ name = 'payload/value.txt'; content = 'b' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-duplicate' -MessagePattern 'duplicate or case-colliding selected path' -Action {
        Invoke-ZipFixtureMaterializer -Name 'duplicate' -ArchivePath $duplicateArchive -MaximumFiles 2 -MaximumBytes 2 -RequiredFiles @() | Out-Null
    }

    $caseCollisionArchive = Join-Path $zipFixtureRoot 'case-collision.zip'
    New-ZipFixture -Path $caseCollisionArchive -Entries @(
        [pscustomobject]@{ name = 'payload/Value.txt'; content = 'a' },
        [pscustomobject]@{ name = 'payload/value.txt'; content = 'b' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-case-collision' -MessagePattern 'duplicate or case-colliding selected path' -Action {
        Invoke-ZipFixtureMaterializer -Name 'case-collision' -ArchivePath $caseCollisionArchive -MaximumFiles 2 -MaximumBytes 2 -RequiredFiles @() | Out-Null
    }

    $allowlistArchive = Join-Path $zipFixtureRoot 'allowlist-positive.zip'
    New-ZipFixture -Path $allowlistArchive -Entries @(
        [pscustomobject]@{ name = 'payload/keep.txt'; content = 'keep' },
        [pscustomobject]@{ name = 'payload/skip.txt'; content = 'skip' }
    ) | Out-Null
    $script:CurrentCase = 'materializer-zip-nonempty-allowlist-positive'
    $allowlistResult = Invoke-ZipFixtureMaterializer `
        -Name 'allowlist-positive' `
        -ArchivePath $allowlistArchive `
        -MaximumFiles 1 `
        -MaximumBytes 4 `
        -RequiredFiles @('payload/keep.txt') `
        -AllowedFiles @('payload/keep.txt')
    $allowlistOutput = Join-Path $allowlistResult.cache_root 'fixtures/allowlist-positive'
    Assert-True -Condition (Test-Path -LiteralPath (Join-Path $allowlistOutput 'payload/keep.txt') -PathType Leaf) -Message 'allowlisted ZIP entry was not extracted'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $allowlistOutput 'payload/skip.txt'))) -Message 'non-allowlisted ZIP entry was extracted'
    $allowlistProvenance = Get-Content -LiteralPath $allowlistResult.provenance_path -Raw | ConvertFrom-Json -Depth 100
    Assert-True -Condition (@($allowlistProvenance.components.'platform-tools-37.0.1'.extracted_files).Count -eq 1) -Message 'non-empty allowlist did not select exactly one entry'
    Complete-Case -Name $script:CurrentCase

    $missingAllowlistArchive = Join-Path $zipFixtureRoot 'missing-allowlist.zip'
    New-ZipFixture -Path $missingAllowlistArchive -Entries @(
        [pscustomobject]@{ name = 'payload/value.txt'; content = 'x' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-missing-allowlist' -MessagePattern 'missing allowlisted files' -Action {
        Invoke-ZipFixtureMaterializer -Name 'missing-allowlist' -ArchivePath $missingAllowlistArchive -MaximumFiles 1 -MaximumBytes 1 -RequiredFiles @() -AllowedFiles @('payload/missing.txt') | Out-Null
    }

    $fileLimitArchive = Join-Path $zipFixtureRoot 'file-limit.zip'
    New-ZipFixture -Path $fileLimitArchive -Entries @(
        [pscustomobject]@{ name = 'payload/one.txt'; content = 'a' },
        [pscustomobject]@{ name = 'payload/two.txt'; content = 'b' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-file-limit' -MessagePattern 'exceeds the bounded maximum of 1 files' -Action {
        Invoke-ZipFixtureMaterializer -Name 'file-limit' -ArchivePath $fileLimitArchive -MaximumFiles 1 -MaximumBytes 2 -RequiredFiles @() | Out-Null
    }

    $byteLimitArchive = Join-Path $zipFixtureRoot 'byte-limit.zip'
    New-ZipFixture -Path $byteLimitArchive -Entries @(
        [pscustomobject]@{ name = 'payload/value.txt'; content = 'xx' }
    ) | Out-Null
    Invoke-FailCase -Name 'materializer-zip-byte-limit' -MessagePattern 'exceeds the bounded maximum of 1 bytes' -Action {
        Invoke-ZipFixtureMaterializer -Name 'byte-limit' -ArchivePath $byteLimitArchive -MaximumFiles 1 -MaximumBytes 1 -RequiredFiles @() | Out-Null
    }

    $script:CurrentCase = 'materializer-download-body-timeout-is-bounded'
    $timeoutFixtureRoot = Join-Path $testRootFull 'download-timeout-fixture'
    $timeoutSource = Join-Path $timeoutFixtureRoot 'stalled-body.bin'
    Write-Utf8NoBom -Path $timeoutSource -Text (('bounded-stalled-body-fixture-' * 128) + "`n")
    $timeoutSourceItem = Get-Item -LiteralPath $timeoutSource -ErrorAction Stop
    $timeoutSourceHash = Get-Sha256 -Path $timeoutSource
    $timeoutManifest = Get-Content -LiteralPath $sourcesManifest -Raw | ConvertFrom-Json -Depth 100
    $timeoutArchive = $timeoutManifest.components.'platform-tools-37.0.1'.archive
    $timeoutArchive.url = 'https://example.invalid/stalled-body.zip'
    $timeoutArchive.size = [long]$timeoutSourceItem.Length
    $timeoutArchive.sha256 = $timeoutSourceHash
    $timeoutManifestPath = Join-Path $timeoutFixtureRoot 'windows-tool-sources.timeout.json'
    [void](Write-JsonFixture -Path $timeoutManifestPath -Value $timeoutManifest)
    $timeoutCache = Join-Path $testRootFull 'cache/download-timeout'
    $timeoutMessage = $null
    $timeoutUnexpectedSuccess = $false
    $timeoutStopwatch = [Diagnostics.Stopwatch]::StartNew()
    try {
        & $materializer `
            -TaskRoot $testRootFull `
            -CacheRoot $timeoutCache `
            -Component platform-tools-37.0.1 `
            -SourcesManifestPath $timeoutManifestPath `
            -AcceptAndroidSdkLicense `
            -PrivateDownloadSourcePath $timeoutSource `
            -PrivateDownloadDeadlineMilliseconds 150 `
            -PrivateDownloadStallBody | Out-Null
        $timeoutUnexpectedSuccess = $true
    }
    catch {
        $timeoutMessage = $_.Exception.Message
    }
    finally {
        $timeoutStopwatch.Stop()
    }
    Assert-True -Condition (-not $timeoutUnexpectedSuccess) -Message 'stalled body unexpectedly completed'
    Assert-True -Condition ($timeoutMessage -match 'download timed out for https://example\.invalid/stalled-body\.zip after deadline 150ms') -Message "stalled body failed for the wrong reason: $timeoutMessage"
    Assert-True -Condition ($timeoutStopwatch.ElapsedMilliseconds -ge 50 -and $timeoutStopwatch.ElapsedMilliseconds -le 3000) -Message "stalled body deadline was not bounded: elapsed_ms=$($timeoutStopwatch.ElapsedMilliseconds)"
    $timeoutResidue = @(Get-ChildItem -LiteralPath $timeoutCache -Force -Recurse -ErrorAction Stop)
    Assert-True -Condition ($timeoutResidue.Count -eq 0) -Message 'stalled body left a final publication or partial download'
    Complete-Case -Name $script:CurrentCase

    $mumuRoot = Join-Path $testRootFull 'installed-mumu'
    $mumuVersion = 'fixture-version'
    $mumuShell = Join-Path $mumuRoot "nx_device/$mumuVersion/shell"
    Write-Utf8NoBom -Path (Join-Path $mumuShell 'adb.exe') -Text 'fixture adb metadata only'
    Write-Utf8NoBom -Path (Join-Path $mumuShell 'sdk/external_renderer_ipc.dll') -Text 'fixture dll metadata only'

    $script:CurrentCase = 'materializer-installed-metadata-positive'
    $mumuCache = Join-Path $testRootFull 'cache/mumu'
    $mumuResult = & $materializer -TaskRoot $testRootFull -CacheRoot $mumuCache -Component mumu-nemu-installed -MumuInstallRoot $mumuRoot -MumuVersion $mumuVersion
    Assert-True -Condition ($mumuResult.state -ceq 'Ready') -Message 'installed metadata materialization was not Ready'
    $mumuProvenance = Get-Content -LiteralPath $mumuResult.provenance_path -Raw | ConvertFrom-Json -Depth 100
    Assert-True -Condition ($mumuProvenance.restrictions.binaries_executed -eq $false) -Message 'installed metadata path incorrectly reported binary execution'
    Assert-True -Condition ($mumuProvenance.restrictions.installed_mumu_nemu_files_copied -eq $false) -Message 'installed MuMu/Nemu files were incorrectly reported as copied'
    Assert-True -Condition ($mumuProvenance.restrictions.downloaded_or_caller_supplied_files_materialized -eq $false) -Message 'metadata-only path incorrectly reported materialized files'
    foreach ($record in @($mumuProvenance.components.'mumu-nemu-installed'.adb, $mumuProvenance.components.'mumu-nemu-installed'.capture_dll)) {
        Assert-True -Condition (-not [string]::IsNullOrWhiteSpace([string]$record.source)) -Message 'installed file source was not recorded'
        Assert-True -Condition ($record.version -ceq $mumuVersion) -Message 'installed file version identity was not recorded'
        Assert-True -Condition (-not [string]::IsNullOrWhiteSpace([string]$record.expected_name)) -Message 'installed file expected name was not recorded'
        Assert-True -Condition ([long]$record.size_bytes -gt 0) -Message 'installed file size was not recorded'
        Assert-True -Condition ([string]$record.sha256 -cmatch '^[0-9a-f]{64}$') -Message 'installed file SHA-256 was not recorded'
        Assert-True -Condition (-not [string]::IsNullOrWhiteSpace([string]$record.license_provenance_note)) -Message 'installed file license/provenance note was not recorded'
        Assert-True -Condition ($null -eq $record.cache_path) -Message 'installed-only file must not claim a cache path'
    }
    Complete-Case -Name $script:CurrentCase

    $script:CurrentCase = 'materializer-cleanup-classification'
    Assert-True -Condition ($mumuProvenance.cleanup.classification -ceq 'task_local_reproducible_cache') -Message 'cleanup classification mismatch'
    Assert-True -Condition ($mumuProvenance.cleanup.reproducible_from_manifest -eq $true) -Message 'cache was not classified reproducible'
    Complete-Case -Name $script:CurrentCase

    Remove-Item -LiteralPath (Join-Path $mumuShell 'sdk/external_renderer_ipc.dll') -Force
    Invoke-FailCase -Name 'materializer-missing-installed-file' -MessagePattern 'does not exist' -Action {
        & $materializer -TaskRoot $testRootFull -CacheRoot (Join-Path $testRootFull 'cache/missing-mumu') -Component mumu-nemu-installed -MumuInstallRoot $mumuRoot -MumuVersion $mumuVersion
    }

    Invoke-FailCase -Name 'materializer-forbidden-global-path' -MessagePattern 'D-drive|strict child' -Action {
        & $materializer -TaskRoot $testRootFull -CacheRoot 'C:\issue194-forbidden-cache' -Component mumu-nemu-installed -MumuInstallRoot $mumuRoot -MumuVersion $mumuVersion
    }

    $script:Completed = $true
    [pscustomobject]@{
        status = 'PASS'
        cases = $script:CaseCount
        downloaded_or_vendor_binaries_executed = $false
        download_timeout_child_processes_started = 0
        live_github_requests = 0
        test_root_cleaned = $true
    } | ConvertTo-Json -Compress
}
catch {
    $firstRed = [ordered]@{
        status = 'FAIL'
        case = $script:CurrentCase
        message = $_.Exception.Message
        generated_at_utc = [DateTimeOffset]::UtcNow.ToString('O')
    }
    Write-Utf8NoBom -Path (Join-Path $testRootFull 'first-red.json') -Text (($firstRed | ConvertTo-Json -Depth 5) + "`n")
    throw
}
finally {
    foreach ($name in @(
        'ACTINGCOMMAND_FAKE_GH_ROOT',
        'ACTINGCOMMAND_FAKE_GH_MODE',
        'ACTINGCOMMAND_FAKE_GH_SOURCE_SHA',
        'ACTINGCOMMAND_FAKE_GH_TREE_SHA',
        'ACTINGCOMMAND_FAKE_GH_REPOSITORY'
    )) {
        [Environment]::SetEnvironmentVariable($name, $null, 'Process')
    }
    if ($script:Completed -and (Test-Path -LiteralPath $testRootFull)) {
        Remove-Item -LiteralPath $testRootFull -Recurse -Force -ErrorAction Stop
    }
}
