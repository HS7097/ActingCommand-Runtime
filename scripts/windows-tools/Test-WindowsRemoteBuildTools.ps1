# SPDX-License-Identifier: AGPL-3.0-only

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $TaskRoot,

    [Parameter(Mandatory)]
    [string] $TestRoot,

    [string] $VisionProviderCheckExecutable
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
        [Parameter(Mandatory)][bool] $CorruptPayload
    )
    $directory = Join-Path $Root "artifacts/$Mode/$ArtifactName"
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $payloads = if ($ArtifactKind -ceq 'Runtime') {
        @(
            @{ name = 'actingcommand-actingd.exe'; content = 'synthetic actingd payload' },
            @{ name = 'actingctl.exe'; content = 'synthetic actingctl payload' }
        )
    } else {
        @(
            @{ name = 'actinglab.exe'; content = 'synthetic actinglab payload' },
            @{ name = 'actingcommand-vision-provider-check.exe'; content = 'synthetic provider-check payload' },
            @{ name = 'actingcommand-device-test.exe'; content = 'synthetic device-test payload' },
            @{ name = 'ac_fastdeploy_ppocr.dll'; content = 'synthetic nonempty provider payload' }
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
    Write-Utf8NoBom -Path (Join-Path $directory 'BUILD-MANIFEST.json') -Text (($manifest | ConvertTo-Json -Depth 8) + "`n")
    if ($CorruptPayload) {
        $corruptName = if ($ArtifactKind -ceq 'Runtime') { 'actingctl.exe' } else { 'ac_fastdeploy_ppocr.dll' }
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
    Get-ChildItem -LiteralPath $source -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $destination $_.Name) }
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
    $runtimeSplit = '\$runtimeFiles\s*=\s*@\(\s*''actingcommand-actingd\.exe'',\s*''actingctl\.exe''\s*\)'
    $toolsSplit = '\$toolFiles\s*=\s*@\(\s*''actinglab\.exe'',\s*''actingcommand-vision-provider-check\.exe'',\s*''actingcommand-device-test\.exe'',\s*''ac_fastdeploy_ppocr\.dll''\s*\)'
    Assert-True -Condition ([regex]::IsMatch($workflowText, $runtimeSplit)) -Message 'workflow Runtime artifact split is not exact'
    Assert-True -Condition ([regex]::IsMatch($workflowText, $toolsSplit)) -Message 'workflow Tools artifact split is not exact'
    foreach ($required in @(
        '--package actingcommand-ppocr-onnx-json-provider',
        'actingcommand_ppocr_onnx_json_provider.dll',
        'ac_fastdeploy_ppocr.dll',
        'expected PP-OCR provider output is empty'
    )) {
        Assert-True -Condition $workflowText.Contains($required) -Message "workflow provider closure is missing '$required'"
    }
    foreach ($field in @(
        'repository', 'commit_sha', 'tree_sha', 'cargo_lock_sha256', 'rust_toolchain',
        'target', 'configuration', 'workflow_run_id', 'workflow_run_attempt',
        'source_artifact_name', 'files', 'path', 'size_bytes', 'sha256'
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
    Assert-True -Condition (@($sourceManifest.components.'provider-v0.3'.required_names.cpu).Count -gt 1) -Message 'CPU closure must cover multiple runtime DLLs'
    Assert-True -Condition (@($sourceManifest.components.'provider-v0.3'.required_names.cuda).Count -gt 2) -Message 'CUDA closure must cover multiple runtime DLLs'
    if (-not [string]::IsNullOrWhiteSpace($VisionProviderCheckExecutable)) {
        Assert-True -Condition (Test-Path -LiteralPath $VisionProviderCheckExecutable -PathType Leaf) -Message 'static manifest parser executable is missing'
    }
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
    Assert-True -Condition (Test-Path -LiteralPath (Join-Path $positiveOutput 'actingctl.exe') -PathType Leaf) -Message 'positive artifact payload was not published'
    Complete-Case -Name $script:CurrentCase

    $script:CurrentCase = 'artifact-tools-provider-positive-exact-selection'
    $toolsOutput = Join-Path $testRootFull 'downloads/tools-positive'
    $toolsJson = & $downloader -Repository $repository -SourceSha $sourceSha -ArtifactKind Tools -TaskRoot $testRootFull -OutputPath $toolsOutput -GhExecutable $fakeGh
    $tools = $toolsJson | ConvertFrom-Json -Depth 20
    Assert-True -Condition ($tools.status -ceq 'PASS') -Message 'Tools artifact verification did not report PASS'
    Assert-True -Condition (@($tools.verified_files).Count -eq 4) -Message 'Tools artifact verifier did not freeze exactly four payloads'
    $providerFixture = Get-Item -LiteralPath (Join-Path $toolsOutput 'ac_fastdeploy_ppocr.dll') -ErrorAction Stop
    Assert-True -Condition ($providerFixture.Length -gt 0) -Message 'Tools artifact provider payload is missing or empty'
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

    $providerRoot = Join-Path $testRootFull 'provider-fixture'
    $providerFiles = [ordered]@{
        provider = 'ac_fastdeploy_ppocr.dll'
        detector = 'models/detector.onnx'
        recognizer = 'models/recognizer.onnx'
        dictionary = 'models/ppocrv6_dict.txt'
        core = 'runtime/onnxruntime.dll'
        shared = 'runtime/onnxruntime_providers_shared.dll'
        cuda = 'runtime/onnxruntime_providers_cuda.dll'
        external = 'runtime/cublas64_12.dll'
    }
    foreach ($entry in $providerFiles.GetEnumerator()) {
        Write-Utf8NoBom -Path (Join-Path $providerRoot $entry.Value) -Text "fixture-$($entry.Key)"
    }
    $ortLicense = "$($ort.license.id); $($ort.license.url); redistribution=$($ort.license.redistribution)"

    function New-ProviderFixtureSet {
        param(
            [Parameter(Mandatory)][string] $Name,
            [Parameter(Mandatory)][string] $Backend,
            [Parameter(Mandatory)][string[]] $RuntimePaths,
            [string] $SelectedCorePath = $providerFiles.core
        )
        $dependencies = foreach ($runtimePath in $RuntimePaths) {
            $runtimeName = [IO.Path]::GetFileName($runtimePath)
            $isOrt = @($ort.extract_allowlist | ForEach-Object { [IO.Path]::GetFileName([string]$_) }) -ccontains $runtimeName
            [ordered]@{
                path = $runtimePath
                sha256 = Get-Sha256 (Join-Path $providerRoot $runtimePath)
                source = if ($isOrt) { [string]$ort.archive.url } else { "task-local-fixture:$runtimeName" }
                version = if ($isOrt) { [string]$ort.version } else { 'fixture-cuda-runtime-v1' }
                license_provenance_note = if ($isOrt) { $ortLicense } else { 'synthetic test-only external CUDA dependency; not redistributed' }
                kind = if ($isOrt) { 'onnxruntime_archive' } else { 'external_cuda' }
            }
        }
        $detectorHash = Get-Sha256 (Join-Path $providerRoot $providerFiles.detector)
        $recognizerHash = Get-Sha256 (Join-Path $providerRoot $providerFiles.recognizer)
        $dictionaryHash = Get-Sha256 (Join-Path $providerRoot $providerFiles.dictionary)
        $selectedCoreHash = if (Test-Path -LiteralPath (Join-Path $providerRoot $SelectedCorePath) -PathType Leaf) {
            Get-Sha256 (Join-Path $providerRoot $SelectedCorePath)
        } else {
            Get-Sha256 (Join-Path $providerRoot $providerFiles.core)
        }
        $ocr = [ordered]@{
            provider_library_path = $providerFiles.provider
            provider_library_sha256 = Get-Sha256 (Join-Path $providerRoot $providerFiles.provider)
            runtime_library_paths = $RuntimePaths
            runtime_library_path = $SelectedCorePath
            runtime_library_sha256 = $selectedCoreHash
            detector_model_path = $providerFiles.detector
            recognizer_model_path = $providerFiles.recognizer
            dictionary_path = $providerFiles.dictionary
            classifier_model_path = $null
            model_ref = 'PP-OCRv6_medium'
            model_sha256 = Get-PpocrModelContentSha256 -Detector $detectorHash -Recognizer $recognizerHash -Dictionary $dictionaryHash
            detector_model_sha256 = $detectorHash
            recognizer_model_sha256 = $recognizerHash
            dictionary_sha256 = $dictionaryHash
            classifier_model_sha256 = $null
            execution_provider = $Backend
            strict_no_fallback = $true
            supported_languages = @('zh_cn', 'en')
            default_timeout_ms = 1000
        }
        if ($Backend -ceq 'cuda') {
            $ocr['cuda_device'] = [ordered]@{
                ordinal = 0
                expected_stable_identity = 'cuda-uuid:fixture'
            }
        }
        $providerManifest = [ordered]@{
            schema_version = 'actingcommand.vision_provider_artifacts.v0.3'
            fastdeploy_ppocr = $ocr
            onnxruntime = $null
        }
        $dependencyManifest = [ordered]@{
            schema_version = 'actingcommand.provider_runtime_dependencies.v1'
            backend = $Backend
            closure_complete = $true
            selected_core_path = $SelectedCorePath
            dependencies = @($dependencies)
        }
        $providerManifestPath = Join-Path $providerRoot "$Name-artifacts.json"
        $dependencyManifestPath = Join-Path $providerRoot "$Name-dependencies.json"
        $providerManifestSha = Write-JsonFixture -Path $providerManifestPath -Value $providerManifest
        $dependencyManifestSha = Write-JsonFixture -Path $dependencyManifestPath -Value $dependencyManifest
        [pscustomobject]@{
            provider = $providerManifest
            dependency = $dependencyManifest
            provider_path = $providerManifestPath
            provider_sha = $providerManifestSha
            dependency_path = $dependencyManifestPath
            dependency_sha = $dependencyManifestSha
        }
    }

    function Save-ProviderFixtureSet {
        param([Parameter(Mandatory)] $Set)
        $Set.provider_sha = Write-JsonFixture -Path $Set.provider_path -Value $Set.provider
        $Set.dependency_sha = Write-JsonFixture -Path $Set.dependency_path -Value $Set.dependency
    }

    function Invoke-ProviderMaterializer {
        param(
            [Parameter(Mandatory)] $Set,
            [Parameter(Mandatory)][string] $CacheName,
            [Parameter(Mandatory)][string] $Backend,
            [string] $SourcesPath = $sourcesManifest,
            [switch] $WithCudaSelector
        )
        $arguments = @{
            TaskRoot = $testRootFull
            CacheRoot = Join-Path $testRootFull "cache/$CacheName"
            Component = 'provider-v0.3'
            OcrBackend = $Backend
            ProviderArtifactManifestPath = $Set.provider_path
            ProviderArtifactManifestSha256 = $Set.provider_sha
            ProviderDependencyManifestPath = $Set.dependency_path
            ProviderDependencyManifestSha256 = $Set.dependency_sha
            SourcesManifestPath = $SourcesPath
        }
        if ($WithCudaSelector) {
            $arguments.CudaDeviceOrdinal = 0
            $arguments.CudaStableIdentity = 'cuda-uuid:fixture'
        }
        & $materializer @arguments
    }

    $cpuSet = New-ProviderFixtureSet -Name 'cpu' -Backend cpu -RuntimePaths @(
        $providerFiles.core,
        $providerFiles.shared
    )
    $script:CurrentCase = 'materializer-provider-cpu-multi-dll-positive'
    $cpuResult = Invoke-ProviderMaterializer -Set $cpuSet -CacheName 'provider-cpu' -Backend cpu
    Assert-True -Condition ($cpuResult.state -ceq 'Ready') -Message 'CPU provider bytes were not Ready'
    $cpuProvenance = Get-Content -LiteralPath $cpuResult.provenance_path -Raw | ConvertFrom-Json -Depth 100
    $cpuProvider = $cpuProvenance.components.'provider-v0.3'
    Assert-True -Condition ($cpuProvider.byte_materialization_ready -eq $true) -Message 'CPU byte readiness was not explicit'
    Assert-True -Condition ($cpuProvider.functional_validation_performed -eq $false) -Message 'CPU fixture incorrectly claimed provider execution'
    Assert-True -Condition (@($cpuProvider.runtime_libraries).Count -eq 2) -Message 'CPU runtime closure did not preserve both DLLs'
    $cpuConfigPath = Join-Path $cpuResult.cache_root ([string]$cpuProvider.canonical_manifest.cache_path)
    $cpuConfig = Get-Content -LiteralPath $cpuConfigPath -Raw | ConvertFrom-Json -Depth 100
    Assert-True -Condition ($null -eq $cpuConfig.fastdeploy_ppocr.PSObject.Properties['cuda_device']) -Message 'CPU canonical configuration did not omit cuda_device'
    Assert-True -Condition (@($cpuConfig.fastdeploy_ppocr.runtime_library_paths).Count -eq 2) -Message 'CPU canonical runtime list was incomplete'
    Assert-True -Condition (@($cpuConfig.fastdeploy_ppocr.runtime_library_paths | Where-Object { $_ -ceq $cpuConfig.fastdeploy_ppocr.runtime_library_path }).Count -eq 1) -Message 'CPU selected core did not occur exactly once'
    if (-not [string]::IsNullOrWhiteSpace($VisionProviderCheckExecutable)) {
        & $VisionProviderCheckExecutable --manifest $cpuConfigPath --backend fastdeploy_ppocr | Out-Null
        Assert-True -Condition ($LASTEXITCODE -eq 0) -Message 'existing manifest parser rejected CPU canonical configuration'
    }
    Complete-Case -Name $script:CurrentCase

    $cudaSet = New-ProviderFixtureSet -Name 'cuda' -Backend cuda -RuntimePaths @(
        $providerFiles.core,
        $providerFiles.shared,
        $providerFiles.cuda,
        $providerFiles.external
    )
    $script:CurrentCase = 'materializer-provider-cuda-multi-dll-positive'
    $cudaResult = Invoke-ProviderMaterializer -Set $cudaSet -CacheName 'provider-cuda' -Backend cuda -WithCudaSelector
    Assert-True -Condition ($cudaResult.state -ceq 'Ready') -Message 'CUDA provider bytes were not Ready'
    $cudaProvenance = Get-Content -LiteralPath $cudaResult.provenance_path -Raw | ConvertFrom-Json -Depth 100
    $cudaProvider = $cudaProvenance.components.'provider-v0.3'
    Assert-True -Condition (@($cudaProvider.runtime_libraries).Count -eq 4) -Message 'CUDA runtime closure was incomplete'
    Assert-True -Condition (@($cudaProvider.runtime_libraries | Where-Object { $_.kind -ceq 'external_cuda' }).Count -eq 1) -Message 'CUDA external dependency provenance was not preserved'
    $cudaConfigPath = Join-Path $cudaResult.cache_root ([string]$cudaProvider.canonical_manifest.cache_path)
    $cudaConfig = Get-Content -LiteralPath $cudaConfigPath -Raw | ConvertFrom-Json -Depth 100
    Assert-True -Condition ($cudaConfig.fastdeploy_ppocr.cuda_device.ordinal -eq 0) -Message 'CUDA ordinal was not preserved'
    Assert-True -Condition ($cudaConfig.fastdeploy_ppocr.cuda_device.expected_stable_identity -ceq 'cuda-uuid:fixture') -Message 'CUDA stable identity was not preserved'
    if (-not [string]::IsNullOrWhiteSpace($VisionProviderCheckExecutable)) {
        & $VisionProviderCheckExecutable --manifest $cudaConfigPath --backend fastdeploy_ppocr | Out-Null
        Assert-True -Condition ($LASTEXITCODE -eq 0) -Message 'existing manifest parser rejected CUDA canonical configuration'
    }
    Complete-Case -Name $script:CurrentCase

    Invoke-FailCase -Name 'materializer-backend-mismatch' -MessagePattern 'backend must match' -Action {
        Invoke-ProviderMaterializer -Set $cudaSet -CacheName 'backend-mismatch' -Backend cpu | Out-Null
    }
    Invoke-FailCase -Name 'materializer-missing-cuda-selector' -MessagePattern 'requires an explicit ordinal' -Action {
        Invoke-ProviderMaterializer -Set $cudaSet -CacheName 'missing-cuda-selector' -Backend cuda | Out-Null
    }

    $fallbackSet = New-ProviderFixtureSet -Name 'fallback' -Backend cpu -RuntimePaths @($providerFiles.core, $providerFiles.shared)
    $fallbackSet.provider.fastdeploy_ppocr.strict_no_fallback = $false
    Save-ProviderFixtureSet -Set $fallbackSet
    Invoke-FailCase -Name 'materializer-undeclared-fallback' -MessagePattern 'strict_no_fallback' -Action {
        Invoke-ProviderMaterializer -Set $fallbackSet -CacheName 'fallback' -Backend cpu | Out-Null
    }

    $missingProviderSet = New-ProviderFixtureSet -Name 'missing-provider' -Backend cpu -RuntimePaths @($providerFiles.core, $providerFiles.shared)
    $missingProviderSet.provider.fastdeploy_ppocr.provider_library_path = 'missing-ac_fastdeploy_ppocr.dll'
    Save-ProviderFixtureSet -Set $missingProviderSet
    Invoke-FailCase -Name 'materializer-missing-provider-output' -MessagePattern 'Cannot find path|does not exist' -Action {
        Invoke-ProviderMaterializer -Set $missingProviderSet -CacheName 'missing-provider' -Backend cpu | Out-Null
    }

    $wrongHashSet = New-ProviderFixtureSet -Name 'wrong-runtime-hash' -Backend cpu -RuntimePaths @($providerFiles.core, $providerFiles.shared)
    $wrongHashSet.dependency.dependencies[1].sha256 = ('f' * 64)
    Save-ProviderFixtureSet -Set $wrongHashSet
    Invoke-FailCase -Name 'materializer-runtime-hash-mismatch' -MessagePattern 'dependency hash mismatch' -Action {
        Invoke-ProviderMaterializer -Set $wrongHashSet -CacheName 'runtime-hash' -Backend cpu | Out-Null
    }

    $missingCompanionSet = New-ProviderFixtureSet -Name 'missing-companion' -Backend cuda -RuntimePaths @(
        $providerFiles.core,
        $providerFiles.cuda,
        $providerFiles.external
    )
    Invoke-FailCase -Name 'materializer-missing-companion' -MessagePattern 'missing onnxruntime_providers_shared' -Action {
        Invoke-ProviderMaterializer -Set $missingCompanionSet -CacheName 'missing-companion' -Backend cuda -WithCudaSelector | Out-Null
    }

    $noExternalSet = New-ProviderFixtureSet -Name 'missing-external' -Backend cuda -RuntimePaths @(
        $providerFiles.core,
        $providerFiles.shared,
        $providerFiles.cuda
    )
    Invoke-FailCase -Name 'materializer-missing-external-cuda-provenance' -MessagePattern 'explicit task-local CUDA' -Action {
        Invoke-ProviderMaterializer -Set $noExternalSet -CacheName 'missing-external' -Backend cuda -WithCudaSelector | Out-Null
    }

    $absentCoreSet = New-ProviderFixtureSet -Name 'absent-core' -Backend cpu -RuntimePaths @($providerFiles.shared) -SelectedCorePath $providerFiles.core
    Invoke-FailCase -Name 'materializer-absent-selected-core' -MessagePattern 'selected ONNX Runtime core' -Action {
        Invoke-ProviderMaterializer -Set $absentCoreSet -CacheName 'absent-core' -Backend cpu | Out-Null
    }

    $duplicateCoreSet = New-ProviderFixtureSet -Name 'duplicate-core' -Backend cpu -RuntimePaths @($providerFiles.core, $providerFiles.core)
    Invoke-FailCase -Name 'materializer-duplicate-selected-core' -MessagePattern 'duplicate or case collision' -Action {
        Invoke-ProviderMaterializer -Set $duplicateCoreSet -CacheName 'duplicate-core' -Backend cpu | Out-Null
    }

    Write-Utf8NoBom -Path (Join-Path $testRootFull 'onnxruntime.dll') -Text 'outside-manifest-root'
    $escapeSet = New-ProviderFixtureSet -Name 'escape' -Backend cpu -RuntimePaths @('../onnxruntime.dll', $providerFiles.shared) -SelectedCorePath '../onnxruntime.dll'
    Invoke-FailCase -Name 'materializer-runtime-path-escape' -MessagePattern 'stay inside' -Action {
        Invoke-ProviderMaterializer -Set $escapeSet -CacheName 'path-escape' -Backend cpu | Out-Null
    }

    Write-Utf8NoBom -Path (Join-Path $providerRoot 'junction-target/onnxruntime.dll') -Text 'junction-core'
    Write-Utf8NoBom -Path (Join-Path $providerRoot 'junction-target/onnxruntime_providers_shared.dll') -Text 'junction-shared'
    New-Item -ItemType Junction -Path (Join-Path $providerRoot 'junction') -Target (Join-Path $providerRoot 'junction-target') | Out-Null
    $reparseSet = New-ProviderFixtureSet -Name 'reparse' -Backend cpu -RuntimePaths @(
        'junction/onnxruntime.dll',
        'junction/onnxruntime_providers_shared.dll'
    ) -SelectedCorePath 'junction/onnxruntime.dll'
    Invoke-FailCase -Name 'materializer-runtime-reparse-path' -MessagePattern 'reparse point' -Action {
        Invoke-ProviderMaterializer -Set $reparseSet -CacheName 'reparse' -Backend cpu | Out-Null
    }

    Write-Utf8NoBom -Path (Join-Path $providerRoot 'runtime/a/external.dll') -Text 'external-a'
    Write-Utf8NoBom -Path (Join-Path $providerRoot 'runtime/b/EXTERNAL.DLL') -Text 'external-b'
    $caseCollisionSet = New-ProviderFixtureSet -Name 'case-collision' -Backend cuda -RuntimePaths @(
        $providerFiles.core,
        $providerFiles.shared,
        $providerFiles.cuda,
        'runtime/a/external.dll',
        'runtime/b/EXTERNAL.DLL'
    )
    Invoke-FailCase -Name 'materializer-runtime-name-case-collision' -MessagePattern 'duplicate or case collision' -Action {
        Invoke-ProviderMaterializer -Set $caseCollisionSet -CacheName 'case-collision' -Backend cuda -WithCudaSelector | Out-Null
    }

    Write-Utf8NoBom -Path (Join-Path $providerRoot 'runtime/unexpected.dll') -Text 'unexpected-ort-name'
    $unexpectedSet = New-ProviderFixtureSet -Name 'unexpected' -Backend cuda -RuntimePaths @(
        $providerFiles.core,
        $providerFiles.shared,
        $providerFiles.cuda,
        $providerFiles.external,
        'runtime/unexpected.dll'
    )
    $unexpected = $unexpectedSet.dependency.dependencies[-1]
    $unexpected.kind = 'onnxruntime_archive'
    $unexpected.source = [string]$ort.archive.url
    $unexpected.version = [string]$ort.version
    $unexpected.license_provenance_note = $ortLicense
    Save-ProviderFixtureSet -Set $unexpectedSet
    Invoke-FailCase -Name 'materializer-unexpected-runtime-name' -MessagePattern 'unexpected ONNX Runtime' -Action {
        Invoke-ProviderMaterializer -Set $unexpectedSet -CacheName 'unexpected' -Backend cuda -WithCudaSelector | Out-Null
    }

    $lowCountManifest = Get-Content -LiteralPath $sourcesManifest -Raw | ConvertFrom-Json -Depth 100
    $lowCountManifest.components.'provider-v0.3'.max_runtime_file_count = 1
    $lowCountPath = Join-Path $providerRoot 'sources-low-count.json'
    [void](Write-JsonFixture -Path $lowCountPath -Value $lowCountManifest)
    Invoke-FailCase -Name 'materializer-runtime-count-bound' -MessagePattern 'dependency count' -Action {
        Invoke-ProviderMaterializer -Set $cpuSet -CacheName 'count-bound' -Backend cpu -SourcesPath $lowCountPath | Out-Null
    }

    $lowBytesManifest = Get-Content -LiteralPath $sourcesManifest -Raw | ConvertFrom-Json -Depth 100
    $lowBytesManifest.components.'provider-v0.3'.max_runtime_total_bytes = 1
    $lowBytesPath = Join-Path $providerRoot 'sources-low-bytes.json'
    [void](Write-JsonFixture -Path $lowBytesPath -Value $lowBytesManifest)
    Invoke-FailCase -Name 'materializer-runtime-byte-bound' -MessagePattern 'total byte bound' -Action {
        Invoke-ProviderMaterializer -Set $cpuSet -CacheName 'byte-bound' -Backend cpu -SourcesPath $lowBytesPath | Out-Null
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
