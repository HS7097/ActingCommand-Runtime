# SPDX-License-Identifier: AGPL-3.0-only
#Requires -Version 7.2

# Workflow #428 CI-OCR-1: logic test of Materialize-VisionRoot.ps1 without the network. A made-up
# registry names made-up files on the reserved host example.invalid; the script's test-only source
# serves them from a local directory. The script runs as a child pwsh process so that its stdout,
# stderr and exit code are read as a caller reads them. Everything lives under -TestRoot, which
# must not exist; the test creates it and removes it at the end.

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $TestRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$script:CaseCount = 0
$script:CurrentCase = $null

function Assert-True {
    param(
        [Parameter(Mandatory)][bool] $Condition,
        [Parameter(Mandatory)][string] $Message
    )
    if (-not $Condition) { throw "case '$($script:CurrentCase)': $Message" }
}

function Complete-Case {
    $script:CaseCount++
    Write-Output "PASS $($script:CurrentCase)"
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string] $Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-TextSha256 {
    param([Parameter(Mandatory)][string] $Text)
    [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Text))
    ).ToLowerInvariant()
}

function Write-Text {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Text
    )
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function New-Package {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][System.Collections.Specialized.OrderedDictionary] $Entries
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    $file = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite)
    $archive = [IO.Compression.ZipArchive]::new($file, [IO.Compression.ZipArchiveMode]::Create, $false)
    try {
        foreach ($name in $Entries.Keys) {
            $entry = $archive.CreateEntry([string]$name, [IO.Compression.CompressionLevel]::Optimal)
            $bytes = [Text.Encoding]::UTF8.GetBytes([string]$Entries[$name])
            $output = $entry.Open()
            try { $output.Write($bytes, 0, $bytes.Length) } finally { $output.Dispose() }
        }
    }
    finally {
        $archive.Dispose()
        $file.Dispose()
    }
}

# The registry pins every file with the bytes of $script:PackageEntries and $script:ModelSources;
# -Package names the package (below the test source) that its archive entry serves.
function Write-Registry {
    param(
        [Parameter(Mandatory)][string] $Path,
        [string] $DropComponent,
        [hashtable] $ModelSha256Override = @{},
        [string] $Package = 'packages/runtime.nupkg'
    )
    $packageFile = Join-Path $script:SourceRoot $Package
    $runtimePins = @(
        foreach ($name in @('onnxruntime.dll', 'onnxruntime_providers_shared.dll')) {
            $text = $script:PackageEntries["runtimes/win-x64/native/$name"]
            [ordered]@{
                path = "runtimes/win-x64/native/$name"
                vision_root_path = "ort/$name"
                size = [Text.Encoding]::UTF8.GetByteCount($text)
                sha256 = Get-TextSha256 -Text $text
            }
        }
    )
    $licensePins = @(
        foreach ($name in @('LICENSE', 'ThirdPartyNotices.txt')) {
            $text = $script:PackageEntries[$name]
            [ordered]@{
                path = $name
                vision_root_path = "ort/licenses/onnxruntime/$name"
                size = [Text.Encoding]::UTF8.GetByteCount($text)
                sha256 = Get-TextSha256 -Text $text
            }
        }
    )
    $artifacts = @(
        foreach ($id in $script:ModelSources.Keys) {
            $source = $script:ModelSources[$id]
            [ordered]@{
                id = $id
                url = "https://example.invalid/$($source.path)"
                relative_path = "test/$($source.path)"
                size = [Text.Encoding]::UTF8.GetByteCount($source.text)
                sha256 = if ($ModelSha256Override.ContainsKey($id)) { $ModelSha256Override[$id] } else { Get-TextSha256 -Text $source.text }
            }
        }
    )
    $components = [ordered]@{
        'onnxruntime-directml-1.24.4' = [ordered]@{
            kind = 'download_zip'
            version = 'test'
            license = [ordered]@{ id = 'MIT'; url = 'https://example.invalid/license'; redistribution = 'task_local_cache_only' }
            archive = [ordered]@{
                url = "https://example.invalid/$Package"
                relative_path = 'downloads/runtime.nupkg'
                size = (Get-Item -LiteralPath $packageFile).Length
                sha256 = Get-Sha256 -Path $packageFile
            }
            extract_relative_path = 'runtime'
            max_extract_bytes = 1048576
            max_extract_file_count = 4
            extract_allowlist = @(
                'runtimes/win-x64/native/onnxruntime.dll',
                'runtimes/win-x64/native/onnxruntime_providers_shared.dll',
                'LICENSE',
                'ThirdPartyNotices.txt'
            )
            required_files = @(
                'runtimes/win-x64/native/onnxruntime.dll',
                'runtimes/win-x64/native/onnxruntime_providers_shared.dll',
                'LICENSE',
                'ThirdPartyNotices.txt'
            )
            pinned_files = $runtimePins
            pinned_license_files = $licensePins
        }
        'directml-1.15.4' = [ordered]@{
            kind = 'download_zip'
            version = 'test'
            archive = [ordered]@{
                url = 'https://example.invalid/packages/directml.nupkg'
                relative_path = 'downloads/directml.nupkg'
                size = 1
                sha256 = '0' * 64
            }
            pinned_files = @(
                [ordered]@{ path = 'bin/x64-win/DirectML.dll'; vision_root_path = 'ort/DirectML.dll'; size = 1; sha256 = '0' * 64 }
            )
        }
        'ppocrv6-medium-onnx' = [ordered]@{
            kind = 'download_files'
            version = 'test'
            compatibility = [ordered]@{ state = 'Ready'; reason = 'made-up test entry' }
            artifacts = $artifacts
        }
    }
    if (-not [string]::IsNullOrEmpty($DropComponent)) {
        $components.Remove($DropComponent)
    }
    $registry = [ordered]@{
        schema_version = 'actingcommand.windows_tool_sources.v1'
        components = $components
    }
    Write-Text -Path $Path -Text (($registry | ConvertTo-Json -Depth 20) + "`n")
}

function Write-ModelSources {
    param([hashtable] $TextOverride = @{})
    foreach ($id in $script:ModelSources.Keys) {
        $source = $script:ModelSources[$id]
        $text = if ($TextOverride.ContainsKey($id)) { $TextOverride[$id] } else { $source.text }
        Write-Text -Path (Join-Path $script:SourceRoot $source.path) -Text $text
    }
}

function Invoke-Materializer {
    param([Parameter(Mandatory)][string[]] $Arguments)
    $start = [Diagnostics.ProcessStartInfo]::new([Environment]::ProcessPath)
    foreach ($argument in @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', $script:Materializer) + $Arguments) {
        $start.ArgumentList.Add($argument)
    }
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        if (-not $process.WaitForExit(300000)) {
            $process.Kill($true)
            throw "case '$($script:CurrentCase)': Materialize-VisionRoot.ps1 did not exit within 300 s"
        }
        $process.WaitForExit()
        [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdoutTask.GetAwaiter().GetResult()
            Stderr = $stderrTask.GetAwaiter().GetResult()
        }
    }
    finally {
        $process.Dispose()
    }
}

function Get-OutputLines {
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Text)
    @($Text -split "`r?`n" | Where-Object { $_ -ne '' })
}

function Get-RelativeFiles {
    param([Parameter(Mandatory)][string] $Root)
    @(Get-ChildItem -LiteralPath $Root -Recurse -Force -File |
        ForEach-Object { [IO.Path]::GetRelativePath($Root, $_.FullName).Replace('\', '/') } |
        Sort-Object)
}

# A successful run's report: exit 0, one JSON line on stdout, Count files.
function Get-ReportedFiles {
    param(
        [Parameter(Mandatory)] $Run,
        [Parameter(Mandatory)][int] $Count
    )
    Assert-True ($Run.ExitCode -eq 0) "exit code $($Run.ExitCode); stderr: $($Run.Stderr)"
    $lines = @(Get-OutputLines -Text $Run.Stdout)
    Assert-True ($lines.Count -eq 1) "stdout holds $($lines.Count) lines, expected one JSON line: $($Run.Stdout)"
    $reported = @(($lines[0] | ConvertFrom-Json -Depth 20).files)
    Assert-True ($reported.Count -eq $Count) "the report lists $($reported.Count) files, expected $Count"
    $reported
}

function Get-WriteTimes {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string[]] $Paths
    )
    $times = @{}
    foreach ($path in $Paths) {
        $times[$path] = (Get-Item -LiteralPath (Join-Path $Root $path)).LastWriteTimeUtc
    }
    $times
}

function Assert-Unchanged {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][hashtable] $Before,
        [Parameter(Mandatory)][string[]] $Paths
    )
    foreach ($path in $Paths) {
        $item = Get-Item -LiteralPath (Join-Path $Root $path)
        Assert-True ($item.LastWriteTimeUtc -eq $Before[$path]) "$path was written again"
    }
}

$testRootFull = [IO.Path]::GetFullPath($TestRoot).TrimEnd('\')
if (Test-Path -LiteralPath $testRootFull) {
    throw "TestRoot $testRootFull must not exist"
}
if (-not (Test-Path -LiteralPath (Split-Path -Parent $testRootFull) -PathType Container)) {
    throw "the parent of TestRoot $testRootFull must exist"
}
New-Item -ItemType Directory -Path $testRootFull | Out-Null

try {
    $script:Materializer = Join-Path $PSScriptRoot 'Materialize-VisionRoot.ps1'
    $script:SourceRoot = Join-Path $testRootFull 'source'
    $script:PackagePath = Join-Path $script:SourceRoot 'packages/runtime.nupkg'
    $script:PackageEntries = [ordered]@{
        'runtimes/win-x64/native/onnxruntime.dll' = 'made-up onnxruntime library'
        'runtimes/win-x64/native/onnxruntime_providers_shared.dll' = 'made-up providers library'
        'runtimes/win-x64/native/onnxruntime.lib' = 'made-up entry that is not allow-listed'
        'LICENSE' = 'made-up license text'
        'ThirdPartyNotices.txt' = 'made-up third party notices'
    }
    $script:ModelSources = [ordered]@{
        'PP-OCRv6_medium_det_onnx' = @{ path = 'models/det/inference.onnx'; text = 'made-up detector model'; target = 'models/PP-OCRv6_medium/det.onnx' }
        'PP-OCRv6_medium_rec_onnx' = @{ path = 'models/rec/inference.onnx'; text = 'made-up recognizer model'; target = 'models/PP-OCRv6_medium/rec.onnx' }
        'ppocrv6_dict' = @{ path = 'dict/ppocrv6_dict.txt'; text = "a`nb`nc`n"; target = 'models/PP-OCRv6_medium/keys.txt' }
    }
    New-Package -Path $script:PackagePath -Entries $script:PackageEntries
    # A package whose LICENSE differs from its pin in content, not in length.
    $otherLicensePackage = 'packages/runtime-other-license.nupkg'
    $otherLicenseEntries = [ordered]@{}
    foreach ($name in $script:PackageEntries.Keys) { $otherLicenseEntries[$name] = $script:PackageEntries[$name] }
    $otherLicenseEntries['LICENSE'] = $script:PackageEntries['LICENSE'].ToUpperInvariant()
    New-Package -Path (Join-Path $script:SourceRoot $otherLicensePackage) -Entries $otherLicenseEntries
    Write-ModelSources
    $registry = Join-Path $testRootFull 'registry/windows-tool-sources.v1.json'
    Write-Registry -Path $registry
    $registrySha256 = Get-Sha256 -Path $registry

    $expected = [ordered]@{
        'ort/onnxruntime.dll' = $script:PackageEntries['runtimes/win-x64/native/onnxruntime.dll']
        'ort/onnxruntime_providers_shared.dll' = $script:PackageEntries['runtimes/win-x64/native/onnxruntime_providers_shared.dll']
        'ort/licenses/onnxruntime/LICENSE' = $script:PackageEntries['LICENSE']
        'ort/licenses/onnxruntime/ThirdPartyNotices.txt' = $script:PackageEntries['ThirdPartyNotices.txt']
    }
    foreach ($id in $script:ModelSources.Keys) {
        $expected[$script:ModelSources[$id].target] = $script:ModelSources[$id].text
    }
    $expectedPaths = @($expected.Keys | Sort-Object)
    $vision = Join-Path $testRootFull 'vision'
    $baseArguments = @('-Registry', $registry, '-ExecutionProvider', 'cpu', '-TestSourceDirectory', $script:SourceRoot)

    # A fresh fill: every file placed, each with its pinned bytes, nothing else.
    $script:CurrentCase = 'fresh-fill'
    $run = Invoke-Materializer -Arguments (@('-Destination', $vision) + $baseArguments)
    Assert-True ($run.ExitCode -eq 0) "exit code $($run.ExitCode); stderr: $($run.Stderr)"
    $lines = @(Get-OutputLines -Text $run.Stdout)
    Assert-True ($lines.Count -eq 1) "stdout holds $($lines.Count) lines, expected one JSON line: $($run.Stdout)"
    $report = $lines[0] | ConvertFrom-Json -Depth 20
    Assert-True ($report.execution_provider -ceq 'cpu') 'the report does not name the cpu execution provider'
    Assert-True ($report.registry.sha256 -ceq $registrySha256) 'the report does not carry the registry SHA-256'
    $reported = @($report.files)
    Assert-True ($reported.Count -eq $expectedPaths.Count) "the report lists $($reported.Count) files, expected $($expectedPaths.Count)"
    foreach ($record in $reported) {
        $path = [string]$record.path
        Assert-True ($expected.Contains($path)) "the report lists an unexpected file $path"
        Assert-True ($record.state -ceq 'downloaded') "$path is $($record.state), expected downloaded"
        $onDisk = Join-Path $vision $path
        Assert-True (Test-Path -LiteralPath $onDisk -PathType Leaf) "$path was not placed"
        Assert-True ((Get-Sha256 -Path $onDisk) -ceq (Get-TextSha256 -Text $expected[$path])) "$path does not hold its made-up bytes"
        Assert-True ($record.sha256 -ceq (Get-Sha256 -Path $onDisk)) "the report's SHA-256 of $path differs from the file"
        Assert-True ([long]$record.size -eq (Get-Item -LiteralPath $onDisk).Length) "the report's size of $path differs from the file"
    }
    $placed = @(Get-RelativeFiles -Root $vision)
    Assert-True (($placed -join '|') -ceq ($expectedPaths -join '|')) "the vision root holds [$($placed -join ', ')]; no DirectML file, package, extra entry or temporary file may remain"
    Complete-Case

    # A second run on the complete root fetches nothing at all. Its test source is an empty
    # directory: any fetch, of the package or of a model file, finds no file there and fails the
    # run, so exit 0 proves that nothing was fetched. Every file is verified and none is replaced.
    $script:CurrentCase = 'second-run-fetches-nothing'
    $emptySource = Join-Path $testRootFull 'source-empty'
    New-Item -ItemType Directory -Path $emptySource | Out-Null
    $before = Get-WriteTimes -Root $vision -Paths $expectedPaths
    $run = Invoke-Materializer -Arguments @('-Destination', $vision, '-Registry', $registry, '-ExecutionProvider', 'cpu', '-Attempts', '1', '-TestSourceDirectory', $emptySource)
    $reported = @(Get-ReportedFiles -Run $run -Count $expectedPaths.Count)
    foreach ($record in $reported) {
        Assert-True ($record.state -ceq 'verified') "$($record.path) is $($record.state), expected verified"
    }
    Assert-Unchanged -Root $vision -Before $before -Paths $expectedPaths
    Assert-True ($run.Stderr.Contains('package of onnxruntime-directml-1.24.4: not fetched, every file it pins verified')) "the run does not say the package was not fetched: $($run.Stderr)"
    Assert-True (-not ($run.Stderr -match ': fetched |: needed for |attempt \d+ of \d+ failed')) "the run fetched or tried to fetch: $($run.Stderr)"
    Assert-True ((@(Get-RelativeFiles -Root $vision) -join '|') -ceq ($expectedPaths -join '|')) 'the second run changed the set of files'
    Complete-Case

    # One license file missing, then one changed: the package is fetched and only that file is
    # placed from it. The test source holds the package alone, so a fetch of any model file would
    # fail the run; every other file stays verified and unwritten.
    $script:CurrentCase = 'one-license-file-fetched'
    $packageOnlySource = Join-Path $testRootFull 'source-package-only'
    New-Item -ItemType Directory -Path (Join-Path $packageOnlySource 'packages') -Force | Out-Null
    Copy-Item -LiteralPath $script:PackagePath -Destination (Join-Path $packageOnlySource 'packages/runtime.nupkg')
    foreach ($change in @(
            @{ path = 'ort/licenses/onnxruntime/LICENSE'; how = 'missing' },
            @{ path = 'ort/licenses/onnxruntime/ThirdPartyNotices.txt'; how = 'changed' })) {
        $licensePath = Join-Path $vision $change.path
        if ($change.how -ceq 'missing') {
            Remove-Item -LiteralPath $licensePath -Force
        } else {
            Write-Text -Path $licensePath -Text 'changed notices'
        }
        $others = @($expectedPaths | Where-Object { $_ -cne $change.path })
        $before = Get-WriteTimes -Root $vision -Paths $others
        $run = Invoke-Materializer -Arguments @('-Destination', $vision, '-Registry', $registry, '-ExecutionProvider', 'cpu', '-Attempts', '1', '-TestSourceDirectory', $packageOnlySource)
        $reported = @(Get-ReportedFiles -Run $run -Count $expectedPaths.Count)
        foreach ($record in $reported) {
            $expectedState = if ([string]$record.path -ceq $change.path) { 'downloaded' } else { 'verified' }
            Assert-True ($record.state -ceq $expectedState) "$($change.how) $($change.path): $($record.path) is $($record.state), expected $expectedState"
        }
        Assert-Unchanged -Root $vision -Before $before -Paths $others
        Assert-True ((Get-Sha256 -Path $licensePath) -ceq (Get-TextSha256 -Text $expected[$change.path])) "$($change.path) does not hold its pinned bytes"
        Assert-True ($run.Stderr -match "(?m)package of onnxruntime-directml-1\.24\.4: needed for $([regex]::Escape($change.path))\r?$") "the package was not needed for $($change.path) alone: $($run.Stderr)"
        Assert-True ($run.Stderr.Contains('package of onnxruntime-directml-1.24.4: fetched https://example.invalid/packages/runtime.nupkg')) "the package was not fetched: $($run.Stderr)"
        Assert-True ((@(Get-RelativeFiles -Root $vision) -join '|') -ceq ($expectedPaths -join '|')) "$($change.how) $($change.path): the set of files changed"
    }
    Complete-Case

    # A license file in the package that differs from its pin (same length, other bytes): the run
    # fails, names the file and both digests, and the file is not left in place.
    $script:CurrentCase = 'license-differs-from-pin-fails'
    $otherLicenseRegistry = Join-Path $testRootFull 'registry/other-license.json'
    Write-Registry -Path $otherLicenseRegistry -Package $otherLicensePackage
    $otherLicenseVision = Join-Path $testRootFull 'vision-other-license'
    $run = Invoke-Materializer -Arguments @('-Destination', $otherLicenseVision, '-Registry', $otherLicenseRegistry, '-ExecutionProvider', 'cpu', '-TestSourceDirectory', $script:SourceRoot)
    Assert-True ($run.ExitCode -ne 0) 'a license file that differs from its pin exited 0'
    Assert-True (@(Get-OutputLines -Text $run.Stdout).Count -eq 0) "a failed run printed on stdout: $($run.Stdout)"
    $licensePin = Get-TextSha256 -Text $script:PackageEntries['LICENSE']
    Assert-True ($run.Stderr -match "FAILED: ort/licenses/onnxruntime/LICENSE: package entry LICENSE has SHA-256 [0-9a-f]{64}, the pin says $licensePin") "the failure does not name the license file and its pin: $($run.Stderr)"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $otherLicenseVision 'ort/licenses/onnxruntime/LICENSE'))) 'a license file that differs from its pin was left in place'
    $leftovers = @(Get-RelativeFiles -Root $otherLicenseVision | Where-Object { $_ -match '\.download-|\.extract-|\.package-' })
    Assert-True ($leftovers.Count -eq 0) "temporary files were left: $($leftovers -join ', ')"
    Complete-Case

    # A wrong hash: the detector's pin does not match what is served. An old detector already in
    # place fails its check too. Neither is left in place, and the run fails after its attempts.
    $script:CurrentCase = 'wrong-hash-fails-after-attempts'
    Write-ModelSources
    $wrongRegistry = Join-Path $testRootFull 'registry/wrong-hash.json'
    Write-Registry -Path $wrongRegistry -ModelSha256Override @{ 'PP-OCRv6_medium_det_onnx' = (Get-TextSha256 -Text 'MADE-UP DETECTOR MODEL') }
    $wrongVision = Join-Path $testRootFull 'vision-wrong-hash'
    Write-Text -Path (Join-Path $wrongVision 'models/PP-OCRv6_medium/det.onnx') -Text 'an old detector'
    $run = Invoke-Materializer -Arguments @('-Destination', $wrongVision, '-Registry', $wrongRegistry, '-ExecutionProvider', 'cpu', '-Attempts', '2', '-TestSourceDirectory', $script:SourceRoot)
    Assert-True ($run.ExitCode -ne 0) 'a wrong hash exited 0'
    Assert-True (@(Get-OutputLines -Text $run.Stdout).Count -eq 0) "a failed run printed on stdout: $($run.Stdout)"
    Assert-True ($run.Stderr -match 'FAILED: models/PP-OCRv6_medium/det\.onnx: its SHA-256 is [0-9a-f]{64}, the pin says [0-9a-f]{64}') "the failure does not name the detector and its SHA-256: $($run.Stderr)"
    Assert-True ($run.Stderr.Contains('all 2 attempt(s) failed')) "the failure does not say the attempts ran out: $($run.Stderr)"
    Assert-True ($run.Stderr.Contains('attempt 1 of 2 failed') -and $run.Stderr.Contains('attempt 2 of 2 failed')) "not every attempt was made: $($run.Stderr)"
    Assert-True (-not $run.Stderr.Contains('attempt 3 of')) 'more attempts than asked were made'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $wrongVision 'models/PP-OCRv6_medium/det.onnx'))) 'a detector that failed its check was left in place'
    $leftovers = @(Get-RelativeFiles -Root $wrongVision | Where-Object { $_ -match '\.download-|\.extract-|\.package-' })
    Assert-True ($leftovers.Count -eq 0) "temporary files were left: $($leftovers -join ', ')"
    Complete-Case

    # A missing registry entry: refused before the destination is touched.
    $script:CurrentCase = 'missing-registry-entry'
    $missingRegistry = Join-Path $testRootFull 'registry/missing-entry.json'
    Write-Registry -Path $missingRegistry -DropComponent 'ppocrv6-medium-onnx'
    $missingVision = Join-Path $testRootFull 'vision-missing-entry'
    $run = Invoke-Materializer -Arguments @('-Destination', $missingVision, '-Registry', $missingRegistry, '-ExecutionProvider', 'cpu', '-TestSourceDirectory', $script:SourceRoot)
    Assert-True ($run.ExitCode -ne 0) 'a registry without the model entry exited 0'
    Assert-True (@(Get-OutputLines -Text $run.Stdout).Count -eq 0) "a failed run printed on stdout: $($run.Stdout)"
    Assert-True ($run.Stderr.Contains("has no component 'ppocrv6-medium-onnx'")) "the failure does not name the missing entry: $($run.Stderr)"
    Assert-True (-not (Test-Path -LiteralPath $missingVision)) 'the destination was created for a registry without the entry'
    Complete-Case

    # A provider other than cpu: refused by name before anything is read or written.
    $script:CurrentCase = 'provider-other-than-cpu'
    $directmlVision = Join-Path $testRootFull 'vision-directml'
    $run = Invoke-Materializer -Arguments @('-Destination', $directmlVision, '-Registry', $registry, '-ExecutionProvider', 'directml', '-TestSourceDirectory', $script:SourceRoot)
    Assert-True ($run.ExitCode -ne 0) 'execution provider directml exited 0'
    Assert-True (@(Get-OutputLines -Text $run.Stdout).Count -eq 0) "a failed run printed on stdout: $($run.Stdout)"
    Assert-True ($run.Stderr.Contains("execution provider 'directml' is refused")) "the refusal does not name directml: $($run.Stderr)"
    Assert-True (-not (Test-Path -LiteralPath $directmlVision)) 'the destination was created for directml'
    Complete-Case

    [pscustomobject]@{
        status = 'PASS'
        cases = $script:CaseCount
        network_requests = 0
    } | ConvertTo-Json -Compress
}
finally {
    if (Test-Path -LiteralPath $testRootFull) {
        Remove-Item -LiteralPath $testRootFull -Recurse -Force
    }
}
