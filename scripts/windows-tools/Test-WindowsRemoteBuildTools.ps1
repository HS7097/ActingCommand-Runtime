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

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Text
    )
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function New-ArtifactFixture {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $Mode,
        [Parameter(Mandatory)][string] $ArtifactName,
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][string] $CommitSha,
        [Parameter(Mandatory)][string] $TreeSha,
        [Parameter(Mandatory)][string] $CargoLockSha256,
        [Parameter(Mandatory)][bool] $CorruptPayload
    )
    $directory = Join-Path $Root "artifacts/$Mode/$ArtifactName"
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $payloads = @(
        @{ name = 'actingcommand-actingd.exe'; content = 'synthetic actingd payload' },
        @{ name = 'actingctl.exe'; content = 'synthetic actingctl payload' }
    )
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
        Add-Content -LiteralPath (Join-Path $directory 'actingctl.exe') -Value 'corrupt' -NoNewline
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
$artifactName = "actingcommand-runtime-$sourceSha"
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
        $artifactRecord = [pscustomobject][ordered]@{ name = $artifactName; expired = $false }
        [object[]]$artifacts = if ($mode -ceq 'missing-artifact') {
            @()
        } else {
            @($artifactRecord)
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
    $toolsSplit = '\$toolFiles\s*=\s*@\(\s*''actinglab\.exe'',\s*''actingcommand-vision-provider-check\.exe'',\s*''actingcommand-device-test\.exe''\s*\)'
    Assert-True -Condition ([regex]::IsMatch($workflowText, $runtimeSplit)) -Message 'workflow Runtime artifact split is not exact'
    Assert-True -Condition ([regex]::IsMatch($workflowText, $toolsSplit)) -Message 'workflow Tools artifact split is not exact'
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
    Complete-Case -Name $script:CurrentCase

    $fixtureRoot = Join-Path $testRootFull 'fake-gh'
    New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
    $repository = 'HS7097/ActingCommand-Runtime'
    $sourceSha = '0123456789abcdef0123456789abcdef01234567'
    $treeSha = '89abcdef0123456789abcdef0123456789abcdef'
    $artifactName = "actingcommand-runtime-$sourceSha"
    Write-Utf8NoBom -Path (Join-Path $fixtureRoot 'Cargo.lock') -Text "fixture-lock`n"
    $lockSha = Get-Sha256 -Path (Join-Path $fixtureRoot 'Cargo.lock')
    New-ArtifactFixture -Root $fixtureRoot -Mode 'success' -ArtifactName $artifactName -Repository $repository -CommitSha $sourceSha -TreeSha $treeSha -CargoLockSha256 $lockSha -CorruptPayload $false
    New-ArtifactFixture -Root $fixtureRoot -Mode 'wrong-hash' -ArtifactName $artifactName -Repository $repository -CommitSha $sourceSha -TreeSha $treeSha -CargoLockSha256 $lockSha -CorruptPayload $true
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
        provider = 'provider.dll'; runtime = 'onnxruntime.dll'; detector = 'detector.onnx'
        recognizer = 'recognizer.onnx'; dictionary = 'ppocrv6_dict.txt'
    }
    foreach ($entry in $providerFiles.GetEnumerator()) {
        Write-Utf8NoBom -Path (Join-Path $providerRoot $entry.Value) -Text "fixture-$($entry.Key)"
    }
    $providerManifest = [ordered]@{
        schema_version = 'actingcommand.vision_provider_artifacts.v0.3'
        fastdeploy_ppocr = [ordered]@{
            provider_library_path = $providerFiles.provider
            provider_library_sha256 = Get-Sha256 (Join-Path $providerRoot $providerFiles.provider)
            runtime_library_path = $providerFiles.runtime
            runtime_library_paths = @($providerFiles.runtime)
            runtime_library_sha256 = Get-Sha256 (Join-Path $providerRoot $providerFiles.runtime)
            detector_model_path = $providerFiles.detector
            recognizer_model_path = $providerFiles.recognizer
            dictionary_path = $providerFiles.dictionary
            classifier_model_path = $null
            model_ref = 'PP-OCRv6_medium'
            model_sha256 = ('0' * 64)
            detector_model_sha256 = Get-Sha256 (Join-Path $providerRoot $providerFiles.detector)
            recognizer_model_sha256 = Get-Sha256 (Join-Path $providerRoot $providerFiles.recognizer)
            dictionary_sha256 = Get-Sha256 (Join-Path $providerRoot $providerFiles.dictionary)
            classifier_model_sha256 = $null
            execution_provider = 'cuda'
            cuda_device = [ordered]@{ ordinal = 0; expected_stable_identity = 'cuda-uuid:fixture' }
            strict_no_fallback = $true
        }
    }
    $providerManifestPath = Join-Path $providerRoot 'artifacts.json'
    Write-Utf8NoBom -Path $providerManifestPath -Text (($providerManifest | ConvertTo-Json -Depth 20) + "`n")
    $providerManifestSha = Get-Sha256 $providerManifestPath

    Invoke-FailCase -Name 'materializer-backend-mismatch' -MessagePattern 'backend must match' -Action {
        & $materializer -TaskRoot $testRootFull -CacheRoot (Join-Path $testRootFull 'cache/backend-mismatch') -Component provider-v0.3 -OcrBackend cpu -ProviderArtifactManifestPath $providerManifestPath -ProviderArtifactManifestSha256 $providerManifestSha
    }
    $providerManifest.fastdeploy_ppocr.provider_library_sha256 = ('f' * 64)
    Write-Utf8NoBom -Path $providerManifestPath -Text (($providerManifest | ConvertTo-Json -Depth 20) + "`n")
    $providerManifestSha = Get-Sha256 $providerManifestPath
    Invoke-FailCase -Name 'materializer-provider-hash-mismatch' -MessagePattern 'provider library does not match' -Action {
        & $materializer -TaskRoot $testRootFull -CacheRoot (Join-Path $testRootFull 'cache/provider-hash') -Component provider-v0.3 -OcrBackend cuda -CudaDeviceOrdinal 0 -CudaStableIdentity 'cuda-uuid:fixture' -ProviderArtifactManifestPath $providerManifestPath -ProviderArtifactManifestSha256 $providerManifestSha
    }
    Invoke-FailCase -Name 'materializer-forbidden-global-path' -MessagePattern 'D-drive|strict child' -Action {
        & $materializer -TaskRoot $testRootFull -CacheRoot 'C:\issue194-forbidden-cache' -Component mumu-nemu-installed -MumuInstallRoot $mumuRoot -MumuVersion $mumuVersion
    }

    $script:Completed = $true
    [pscustomobject]@{
        status = 'PASS'
        cases = $script:CaseCount
        downloaded_or_vendor_binaries_executed = $false
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
