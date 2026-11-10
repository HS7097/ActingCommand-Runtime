# SPDX-License-Identifier: AGPL-3.0-only
#Requires -Version 7.2

<#
.SYNOPSIS
Fills or verifies one vision root for the CPU execution provider from windows-tool-sources.v1.json.

.DESCRIPTION
Workflow #428 CI-OCR-1 (contracts/v012-register.md, section CI-OCR-1). This is the one
implementation a resource repository's CI fetches at its Tools pin's commit and calls to give a
hosted runner a vision root for the CPU execution provider. It places, in the layout of
contracts/vision-model-folders.md:

  ort/onnxruntime.dll, ort/onnxruntime_providers_shared.dll
      the pinned_files of onnxruntime-directml-1.24.4 at their vision_root_path, extracted from the
      package that entry pins;
  ort/licenses/onnxruntime/LICENSE, ort/licenses/onnxruntime/ThirdPartyNotices.txt
      the same package's license files, as the entry's placement_note asks;
  models/PP-OCRv6_medium/det.onnx, rec.onnx, keys.txt
      the detector, recognizer and dictionary of ppocrv6-medium-onnx, in the flat layout.

Every address, size and SHA-256 comes from the registry. A file that exists with its pinned size
and SHA-256 is kept and reported "verified". Any other file is fetched to a temporary name beside
it, checked for size and SHA-256 and moved into place ("downloaded"); a file that fails its check is
removed, never left in place. Each fetch has -Attempts attempts. The license files have no pin of
their own: they are checked against the entries of the pinned package, so every run fetches that
package once.

It writes no configuration file and starts no program: the caller writes its configuration and runs
its own self-check. On success it prints one JSON line on stdout (each file with its path, size,
SHA-256 and state, and the registry's own SHA-256) and exits 0. Diagnostics go to stderr. Every
failure exits 1 with a message on stderr that names the file and the reason.

.PARAMETER Destination
The vision root to fill or verify. It is created when it does not exist.

.PARAMETER Registry
The tool registry; by default windows-tool-sources.v1.json beside this script.

.PARAMETER ExecutionProvider
The execution provider the vision root is for. Only cpu is taken.

.PARAMETER Attempts
Attempts per fetched file, at least 1 (default 3).
#>

[CmdletBinding()]
param(
    [string] $Destination,

    [string] $Registry = (Join-Path $PSScriptRoot 'windows-tool-sources.v1.json'),

    [string] $ExecutionProvider,

    [int] $Attempts = 3,

    # Test only: every address is read from this directory (the address's path below it) instead
    # of the network. Refused unless every address the selected registry entries name is on the
    # reserved host example.invalid, so it cannot stand in for a real registry's sources.
    [Parameter(DontShow)]
    [string] $TestSourceDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$script:RegistrySchema = 'actingcommand.windows_tool_sources.v1'
$script:RuntimeComponent = 'onnxruntime-directml-1.24.4'
$script:ModelComponent = 'ppocrv6-medium-onnx'
# contracts/vision-model-folders.md: ort\onnxruntime.dll is required; the package's license files
# go to ort\licenses\onnxruntime\.
$script:RequiredRuntimeFile = 'ort/onnxruntime.dll'
$script:LicenseDirectory = 'ort/licenses/onnxruntime'
$script:LicenseFiles = @('LICENSE', 'ThirdPartyNotices.txt')
# The OCR model folder, named by the model_ref recognition packs use, in the flat layout.
$script:ModelDirectory = 'models/PP-OCRv6_medium'
$script:ModelFiles = [ordered]@{
    'PP-OCRv6_medium_det_onnx' = 'det.onnx'
    'PP-OCRv6_medium_rec_onnx' = 'rec.onnx'
    'ppocrv6_dict' = 'keys.txt'
}
$script:TestHost = 'example.invalid'
$script:AttemptDeadline = [TimeSpan]::FromMinutes(5)
$script:RetryDelaySeconds = 5
$script:SourceRoot = $null
$script:Http = $null

function Stop-VisionRoot {
    param([Parameter(Mandatory)][string] $Message)
    throw [InvalidOperationException]::new($Message)
}

function Write-Diagnostic {
    param([Parameter(Mandatory)][string] $Message)
    [Console]::Error.WriteLine("Materialize-VisionRoot: $Message")
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string] $Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Field {
    param(
        [Parameter(Mandatory)][AllowNull()] $Object,
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $Label
    )
    if ($Object -isnot [Management.Automation.PSCustomObject]) {
        Stop-VisionRoot "$Label is not a JSON object"
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property -or $property.Name -cne $Name -or $null -eq $property.Value) {
        Stop-VisionRoot "$Label has no $Name"
    }
    $property.Value
}

function Get-TextField {
    param($Object, [string] $Name, [string] $Label)
    $value = Get-Field -Object $Object -Name $Name -Label $Label
    if ($value -isnot [string] -or [string]::IsNullOrWhiteSpace($value)) {
        Stop-VisionRoot "${Label}: $Name must be a non-empty string"
    }
    $value
}

function Get-SizeField {
    param($Object, [string] $Name, [string] $Label)
    $value = Get-Field -Object $Object -Name $Name -Label $Label
    if (($value -isnot [long] -and $value -isnot [int]) -or [long]$value -le 0) {
        Stop-VisionRoot "${Label}: $Name must be a positive whole number"
    }
    [long]$value
}

function Get-Sha256Field {
    param($Object, [string] $Name, [string] $Label)
    $value = Get-TextField -Object $Object -Name $Name -Label $Label
    if ($value -cnotmatch '\A[0-9a-f]{64}\z') {
        Stop-VisionRoot "${Label}: $Name must be 64 lowercase hexadecimal digits"
    }
    $value
}

function Get-AddressField {
    param($Object, [string] $Name, [string] $Label)
    $value = Get-TextField -Object $Object -Name $Name -Label $Label
    $uri = $null
    if (-not [Uri]::TryCreate($value, [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -cne 'https') {
        Stop-VisionRoot "${Label}: $Name must be an absolute https address, not $value"
    }
    $value
}

function Get-ChildPath {
    param([string] $Root, [string] $RelativePath, [string] $Label)
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or
        [IO.Path]::IsPathRooted($RelativePath) -or
        $RelativePath.Contains(':') -or
        $RelativePath.Contains('\')) {
        Stop-VisionRoot "${Label}: '$RelativePath' is not a relative path with / separators"
    }
    $segments = $RelativePath.Split('/')
    foreach ($segment in $segments) {
        if ($segment -ceq '' -or $segment -ceq '.' -or $segment -ceq '..') {
            Stop-VisionRoot "${Label}: '$RelativePath' has an empty or parent segment"
        }
    }
    [IO.Path]::Combine([string[]](@($Root) + $segments))
}

function Get-Component {
    param($Components, [string] $Id, [string] $RegistryPath)
    $property = $Components.PSObject.Properties[$Id]
    if ($null -eq $property -or $property.Name -cne $Id) {
        Stop-VisionRoot "registry $RegistryPath has no component '$Id'"
    }
    if ($property.Value -isnot [Management.Automation.PSCustomObject]) {
        Stop-VisionRoot "registry $RegistryPath component '$Id' is not a JSON object"
    }
    $property.Value
}

# 'missing', 'verified', or the reason the file differs from its pin. A directory or a link where a
# file belongs is never removed: it fails the run.
function Test-PlacedFile {
    param([string] $Path, [string] $Label, [long] $Size, [string] $Sha256)
    if (-not (Test-Path -LiteralPath $Path)) {
        return 'missing'
    }
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer) {
        Stop-VisionRoot "${Label}: $Path is a directory, not a file"
    }
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Stop-VisionRoot "${Label}: $Path is a link, not a regular file"
    }
    if ($item.Length -ne $Size) {
        return "it holds $($item.Length) bytes, the pin says $Size"
    }
    $actual = Get-Sha256 -Path $Path
    if ($actual -cne $Sha256) {
        return "its SHA-256 is $actual, the pin says $Sha256"
    }
    'verified'
}

# Creates the target's folders and requires every folder between the target and the root to be a
# regular directory.
function Initialize-TargetFolder {
    param([string] $Root, [string] $Target, [string] $Label)
    $parent = [IO.Path]::GetDirectoryName($Target)
    if (-not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $cursor = $parent
    while ($cursor.Length -gt $Root.Length) {
        $item = Get-Item -LiteralPath $cursor -Force
        if (-not $item.PSIsContainer -or
            (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
            Stop-VisionRoot "${Label}: $cursor must be a regular directory"
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}

function Get-FailureReason {
    param([Parameter(Mandatory)] $ErrorRecord)
    for ($cursor = $ErrorRecord.Exception; $null -ne $cursor; $cursor = $cursor.InnerException) {
        if ($cursor -is [OperationCanceledException]) {
            return "no complete answer within $([int]$script:AttemptDeadline.TotalMinutes) minutes"
        }
    }
    $base = $ErrorRecord.Exception.GetBaseException()
    if ([string]::IsNullOrWhiteSpace($base.Message)) { $ErrorRecord.Exception.Message } else { $base.Message }
}

function Open-TestSource {
    param([string] $Url)
    $uri = [Uri]$Url
    $relative = [Uri]::UnescapeDataString($uri.AbsolutePath.TrimStart('/'))
    $local = Get-ChildPath -Root $script:SourceRoot -RelativePath $relative -Label "test source of $Url"
    if (-not (Test-Path -LiteralPath $local -PathType Leaf)) {
        throw "the test source has no file $local"
    }
    [IO.File]::OpenRead($local)
}

# Streams one address into a new file at Path, at most Size bytes and exactly Size bytes in the end.
function Copy-Source {
    param([string] $Url, [string] $Path, [long] $Size)
    $cancellation = [Threading.CancellationTokenSource]::new($script:AttemptDeadline)
    $response = $null
    $stream = $null
    $output = $null
    try {
        if ($null -ne $script:SourceRoot) {
            $stream = Open-TestSource -Url $Url
        } else {
            $response = $script:Http.GetAsync(
                [Uri]$Url,
                [Net.Http.HttpCompletionOption]::ResponseHeadersRead,
                $cancellation.Token
            ).GetAwaiter().GetResult()
            $status = [int]$response.StatusCode
            if ($status -ne 200) {
                throw "HTTP $status"
            }
            $declared = $response.Content.Headers.ContentLength
            if ($null -ne $declared -and $declared -ne $Size) {
                throw "Content-Length is $declared, the pin says $Size"
            }
            $stream = $response.Content.ReadAsStreamAsync($cancellation.Token).GetAwaiter().GetResult()
        }
        $output = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $buffer = [byte[]]::new(1048576)
        [long] $written = 0
        while ($true) {
            $read = $stream.ReadAsync($buffer, 0, $buffer.Length, $cancellation.Token).GetAwaiter().GetResult()
            if ($read -eq 0) { break }
            $written += $read
            if ($written -gt $Size) {
                throw "the answer is longer than the pinned $Size bytes"
            }
            $output.Write($buffer, 0, $read)
        }
        $output.Flush($true)
        if ($written -ne $Size) {
            throw "the answer holds $written bytes, the pin says $Size"
        }
    }
    finally {
        if ($null -ne $output) { $output.Dispose() }
        if ($null -ne $stream) { $stream.Dispose() }
        if ($null -ne $response) { $response.Dispose() }
        $cancellation.Dispose()
    }
}

# Fetches one file to a temporary name beside Target, checks its size and SHA-256 and moves it to
# Target, which must not exist. A fetched file that fails its check is removed; after the last
# failed attempt the run fails.
function Receive-File {
    param([string] $Label, [string] $Url, [long] $Size, [string] $Sha256, [string] $Target)
    $lastReason = $null
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        if ($attempt -gt 1 -and $null -eq $script:SourceRoot) {
            Start-Sleep -Seconds ($script:RetryDelaySeconds * ($attempt - 1))
        }
        $partial = "$Target.download-$([Guid]::NewGuid().ToString('N'))"
        try {
            Copy-Source -Url $Url -Path $partial -Size $Size
            $actual = Get-Sha256 -Path $partial
            if ($actual -cne $Sha256) {
                throw "its SHA-256 is $actual, the pin says $Sha256"
            }
            [IO.File]::Move($partial, $Target)
            Write-Diagnostic "${Label}: fetched $Url, $Size bytes, attempt $attempt of $Attempts"
            return
        }
        catch {
            $lastReason = Get-FailureReason -ErrorRecord $_
            Write-Diagnostic "${Label}: attempt $attempt of $Attempts failed ($Url): $lastReason"
        }
        finally {
            if (Test-Path -LiteralPath $partial) {
                Remove-Item -LiteralPath $partial -Force
            }
        }
    }
    Stop-VisionRoot "${Label}: $lastReason ($Url); all $Attempts attempt(s) failed"
}

function Get-PackageEntry {
    param($Zip, [string] $Name, [string] $Label)
    $found = @($Zip.Entries | Where-Object {
        $_.FullName.Replace('\', '/').Equals($Name, [StringComparison]::OrdinalIgnoreCase)
    })
    if ($found.Count -ne 1) {
        Stop-VisionRoot "${Label}: the package holds $($found.Count) entries named $Name, expected exactly one"
    }
    $entry = $found[0]
    if ($entry.FullName.Replace('\', '/') -cne $Name) {
        Stop-VisionRoot "${Label}: the package names the entry $($entry.FullName), expected $Name"
    }
    if ((($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) {
        Stop-VisionRoot "${Label}: package entry $Name is a symbolic link"
    }
    $entry
}

# Writes one package entry to a new file at Path, exactly Size bytes.
function Expand-PackageEntry {
    param($Entry, [string] $Path, [long] $Size)
    $source = $Entry.Open()
    $output = $null
    try {
        $output = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        $buffer = [byte[]]::new(1048576)
        [long] $written = 0
        while ($true) {
            $read = $source.Read($buffer, 0, $buffer.Length)
            if ($read -eq 0) { break }
            $written += $read
            if ($written -gt $Size) {
                throw "the entry is longer than $Size bytes"
            }
            $output.Write($buffer, 0, $read)
        }
        $output.Flush($true)
        if ($written -ne $Size) {
            throw "the entry holds $written bytes, expected $Size"
        }
    }
    finally {
        if ($null -ne $output) { $output.Dispose() }
        $source.Dispose()
    }
}

function Invoke-VisionRoot {
    if ([string]::IsNullOrEmpty($ExecutionProvider)) {
        Stop-VisionRoot 'ExecutionProvider is required; only cpu is taken'
    }
    if ($ExecutionProvider -cne 'cpu') {
        Stop-VisionRoot "execution provider '$ExecutionProvider' is refused: this script places a vision root for cpu only (the DirectML files are placed elsewhere)"
    }
    if ($Attempts -lt 1) {
        Stop-VisionRoot "Attempts must be at least 1, not $Attempts"
    }
    if ([string]::IsNullOrWhiteSpace($Destination)) {
        Stop-VisionRoot 'Destination is required'
    }
    if ([string]::IsNullOrWhiteSpace($Registry)) {
        Stop-VisionRoot 'Registry must name a file'
    }

    # The registry: its own SHA-256, then the two components a CPU vision root needs.
    $registryPath = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Registry))
    if (-not (Test-Path -LiteralPath $registryPath -PathType Leaf)) {
        Stop-VisionRoot "registry $registryPath does not exist"
    }
    $registryBytes = [IO.File]::ReadAllBytes($registryPath)
    $registrySha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($registryBytes)).ToLowerInvariant()
    try {
        $document = [Text.UTF8Encoding]::new($false, $true).GetString($registryBytes) | ConvertFrom-Json -Depth 100
    }
    catch {
        Stop-VisionRoot "registry $registryPath is not UTF-8 JSON: $($_.Exception.Message)"
    }
    $registryLabel = "registry $registryPath"
    $schema = Get-TextField -Object $document -Name 'schema_version' -Label $registryLabel
    if ($schema -cne $script:RegistrySchema) {
        Stop-VisionRoot "${registryLabel}: schema_version is $schema, expected $($script:RegistrySchema)"
    }
    $components = Get-Field -Object $document -Name 'components' -Label $registryLabel
    if ($components -isnot [Management.Automation.PSCustomObject]) {
        Stop-VisionRoot "${registryLabel}: components is not a JSON object"
    }
    Write-Diagnostic "registry $registryPath, SHA-256 $registrySha256"

    $files = [Collections.Generic.List[object]]::new()

    # ONNX Runtime: the pinned files at their vision_root_path and the package's license files.
    $runtime = Get-Component -Components $components -Id $script:RuntimeComponent -RegistryPath $registryPath
    $runtimeLabel = "component $($script:RuntimeComponent)"
    $kind = Get-TextField -Object $runtime -Name 'kind' -Label $runtimeLabel
    if ($kind -cne 'download_zip') {
        Stop-VisionRoot "${runtimeLabel}: kind is $kind, expected download_zip"
    }
    $archive = Get-Field -Object $runtime -Name 'archive' -Label $runtimeLabel
    $package = [pscustomobject]@{
        Url = Get-AddressField -Object $archive -Name 'url' -Label "$runtimeLabel archive"
        Size = Get-SizeField -Object $archive -Name 'size' -Label "$runtimeLabel archive"
        Sha256 = Get-Sha256Field -Object $archive -Name 'sha256' -Label "$runtimeLabel archive"
        MaximumBytes = Get-SizeField -Object $runtime -Name 'max_extract_bytes' -Label $runtimeLabel
        MaximumFiles = Get-SizeField -Object $runtime -Name 'max_extract_file_count' -Label $runtimeLabel
    }
    $allowlist = @(Get-Field -Object $runtime -Name 'extract_allowlist' -Label $runtimeLabel)
    $pins = @(Get-Field -Object $runtime -Name 'pinned_files' -Label $runtimeLabel)
    if ($pins.Count -eq 0) {
        Stop-VisionRoot "${runtimeLabel}: pinned_files is empty"
    }
    foreach ($pin in $pins) {
        $entryName = Get-TextField -Object $pin -Name 'path' -Label "$runtimeLabel pinned file"
        $pinLabel = "$runtimeLabel pinned file $entryName"
        $visionPath = Get-TextField -Object $pin -Name 'vision_root_path' -Label $pinLabel
        if (-not $visionPath.StartsWith('ort/', [StringComparison]::Ordinal)) {
            Stop-VisionRoot "${pinLabel}: vision_root_path $visionPath is not under ort/"
        }
        if ($allowlist -cnotcontains $entryName) {
            Stop-VisionRoot "${pinLabel}: the entry is not in extract_allowlist"
        }
        $files.Add([pscustomobject]@{
            Path = $visionPath
            Component = $script:RuntimeComponent
            Source = 'package_pinned'
            Entry = $entryName
            Url = $null
            Size = Get-SizeField -Object $pin -Name 'size' -Label $pinLabel
            Sha256 = Get-Sha256Field -Object $pin -Name 'sha256' -Label $pinLabel
            State = $null
        })
    }
    if (@($files | Where-Object { $_.Path -ceq $script:RequiredRuntimeFile }).Count -ne 1) {
        Stop-VisionRoot "${runtimeLabel}: pinned_files must place $($script:RequiredRuntimeFile) once"
    }
    foreach ($license in $script:LicenseFiles) {
        if ($allowlist -cnotcontains $license) {
            Stop-VisionRoot "${runtimeLabel}: the license file $license is not in extract_allowlist"
        }
        $files.Add([pscustomobject]@{
            Path = "$($script:LicenseDirectory)/$license"
            Component = $script:RuntimeComponent
            Source = 'package_license'
            Entry = $license
            Url = $null
            Size = $null
            Sha256 = $null
            State = $null
        })
    }

    # The OCR model: detector, recognizer and dictionary in the flat layout.
    $model = Get-Component -Components $components -Id $script:ModelComponent -RegistryPath $registryPath
    $modelLabel = "component $($script:ModelComponent)"
    $kind = Get-TextField -Object $model -Name 'kind' -Label $modelLabel
    if ($kind -cne 'download_files') {
        Stop-VisionRoot "${modelLabel}: kind is $kind, expected download_files"
    }
    if ($null -ne $model.PSObject.Properties['compatibility']) {
        $compatibility = Get-Field -Object $model -Name 'compatibility' -Label $modelLabel
        $state = Get-TextField -Object $compatibility -Name 'state' -Label "$modelLabel compatibility"
        if ($state -cne 'Ready') {
            $reason = if ($null -ne $compatibility.PSObject.Properties['reason']) { [string]$compatibility.reason } else { 'no reason given' }
            Stop-VisionRoot "${modelLabel}: the registry marks it $state, not Ready ($reason)"
        }
    }
    $artifacts = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
    foreach ($artifact in @(Get-Field -Object $model -Name 'artifacts' -Label $modelLabel)) {
        $id = Get-TextField -Object $artifact -Name 'id' -Label "$modelLabel artifact"
        if (@($script:ModelFiles.Keys) -cnotcontains $id) {
            Stop-VisionRoot "${modelLabel}: artifact '$id' has no place in a vision root"
        }
        if ($artifacts.ContainsKey($id)) {
            Stop-VisionRoot "${modelLabel}: artifact '$id' is listed twice"
        }
        $artifacts.Add($id, $artifact)
    }
    foreach ($id in $script:ModelFiles.Keys) {
        if (-not $artifacts.ContainsKey($id)) {
            Stop-VisionRoot "${modelLabel}: the registry has no artifact '$id'"
        }
        $artifactLabel = "$modelLabel artifact $id"
        $files.Add([pscustomobject]@{
            Path = "$($script:ModelDirectory)/$($script:ModelFiles[$id])"
            Component = $script:ModelComponent
            Source = 'download'
            Entry = $null
            Url = Get-AddressField -Object $artifacts[$id] -Name 'url' -Label $artifactLabel
            Size = Get-SizeField -Object $artifacts[$id] -Name 'size' -Label $artifactLabel
            Sha256 = Get-Sha256Field -Object $artifacts[$id] -Name 'sha256' -Label $artifactLabel
            State = $null
        })
    }
    $distinctPaths = @($files | ForEach-Object { $_.Path.ToLowerInvariant() } | Sort-Object -Unique)
    if ($distinctPaths.Count -ne $files.Count) {
        Stop-VisionRoot "${registryLabel}: two files would be placed at the same path"
    }

    # The test-only source: refused for any address off the reserved test host.
    $addresses = @($package.Url) + @($files | Where-Object { $null -ne $_.Url } | ForEach-Object { $_.Url })
    if (-not [string]::IsNullOrEmpty($TestSourceDirectory)) {
        foreach ($address in $addresses) {
            if (([Uri]$address).Host -cne $script:TestHost) {
                Stop-VisionRoot "TestSourceDirectory is for tests only and is refused: the registry names $address, which is not on $($script:TestHost)"
            }
        }
        $sourceRoot = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($TestSourceDirectory))
        if (-not (Test-Path -LiteralPath $sourceRoot -PathType Container)) {
            Stop-VisionRoot "TestSourceDirectory $sourceRoot is not a directory"
        }
        $script:SourceRoot = $sourceRoot.TrimEnd('\')
        Write-Diagnostic "test source $($script:SourceRoot) stands in for $($script:TestHost)"
    } else {
        $handler = [Net.Http.HttpClientHandler]::new()
        $handler.AllowAutoRedirect = $true
        $script:Http = [Net.Http.HttpClient]::new($handler, $true)
        $script:Http.Timeout = [Threading.Timeout]::InfiniteTimeSpan
        $script:Http.DefaultRequestHeaders.UserAgent.ParseAdd('ActingCommand-Materialize-VisionRoot/1.0')
    }

    # The destination.
    $root = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)).TrimEnd('\')
    if (Test-Path -LiteralPath $root) {
        $rootItem = Get-Item -LiteralPath $root -Force
        if (-not $rootItem.PSIsContainer) {
            Stop-VisionRoot "Destination $root exists and is not a directory"
        }
        if (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Stop-VisionRoot "Destination $root is a link, not a regular directory"
        }
    } else {
        New-Item -ItemType Directory -Path $root -Force | Out-Null
    }
    Write-Diagnostic "vision root $root, execution provider cpu, $Attempts attempt(s) per fetched file"
    foreach ($file in $files) {
        $file | Add-Member -NotePropertyName Target -NotePropertyValue (Get-ChildPath -Root $root -RelativePath $file.Path -Label $file.Path)
        Initialize-TargetFolder -Root $root -Target $file.Target -Label $file.Path
    }

    # Files with a pin of their own: keep what verifies, remove what does not.
    foreach ($file in $files | Where-Object { $_.Source -cne 'package_license' }) {
        $state = Test-PlacedFile -Path $file.Target -Label $file.Path -Size $file.Size -Sha256 $file.Sha256
        if ($state -ceq 'verified') {
            $file.State = 'verified'
            Write-Diagnostic "$($file.Path): verified, $($file.Size) bytes"
        } elseif ($state -ceq 'missing') {
            Write-Diagnostic "$($file.Path): missing"
        } else {
            Write-Diagnostic "$($file.Path): $state; it is removed and fetched again"
            Remove-Item -LiteralPath $file.Target -Force
        }
    }

    # The ONNX Runtime package: pinned files that did not verify, and the license files, which are
    # checked against the package's entries.
    $packagePath = Join-Path $root ('.package-' + [Guid]::NewGuid().ToString('N'))
    $zip = $null
    try {
        Receive-File -Label "package of $($script:RuntimeComponent)" -Url $package.Url -Size $package.Size -Sha256 $package.Sha256 -Target $packagePath
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($packagePath)
        [long] $extractedBytes = 0
        [long] $extractedFiles = 0
        foreach ($file in $files | Where-Object { $_.Source -like 'package_*' }) {
            if ($file.State -ceq 'verified') { continue }
            $entry = Get-PackageEntry -Zip $zip -Name $file.Entry -Label $file.Path
            $entrySize = [long]$entry.Length
            if ($file.Source -ceq 'package_pinned' -and $entrySize -ne $file.Size) {
                Stop-VisionRoot "$($file.Path): package entry $($file.Entry) holds $entrySize bytes, the pin says $($file.Size)"
            }
            if ($entrySize -le 0) {
                Stop-VisionRoot "$($file.Path): package entry $($file.Entry) is empty"
            }
            $extractedBytes += $entrySize
            $extractedFiles++
            if ($extractedBytes -gt $package.MaximumBytes -or $extractedFiles -gt $package.MaximumFiles) {
                Stop-VisionRoot "$($file.Path): extracting it exceeds the registry's bound of $($package.MaximumFiles) files and $($package.MaximumBytes) bytes"
            }
            $partial = "$($file.Target).extract-$([Guid]::NewGuid().ToString('N'))"
            try {
                try {
                    Expand-PackageEntry -Entry $entry -Path $partial -Size $entrySize
                }
                catch {
                    Stop-VisionRoot "$($file.Path): package entry $($file.Entry) cannot be extracted: $(Get-FailureReason -ErrorRecord $_)"
                }
                $actual = Get-Sha256 -Path $partial
                if ($file.Source -ceq 'package_pinned') {
                    if ($actual -cne $file.Sha256) {
                        Stop-VisionRoot "$($file.Path): package entry $($file.Entry) has SHA-256 $actual, the pin says $($file.Sha256)"
                    }
                } else {
                    $file.Size = $entrySize
                    $file.Sha256 = $actual
                    $state = Test-PlacedFile -Path $file.Target -Label $file.Path -Size $file.Size -Sha256 $file.Sha256
                    if ($state -ceq 'verified') {
                        $file.State = 'verified'
                        Write-Diagnostic "$($file.Path): verified against the package, $($file.Size) bytes"
                        continue
                    }
                    if ($state -cne 'missing') {
                        Write-Diagnostic "$($file.Path): differs from the package ($state); it is replaced"
                        Remove-Item -LiteralPath $file.Target -Force
                    }
                }
                [IO.File]::Move($partial, $file.Target)
                $file.State = 'downloaded'
                Write-Diagnostic "$($file.Path): placed from the package, $entrySize bytes"
            }
            finally {
                if (Test-Path -LiteralPath $partial) {
                    Remove-Item -LiteralPath $partial -Force
                }
            }
        }
    }
    finally {
        if ($null -ne $zip) { $zip.Dispose() }
        if (Test-Path -LiteralPath $packagePath) {
            Remove-Item -LiteralPath $packagePath -Force
        }
    }

    # The model files that did not verify.
    foreach ($file in $files | Where-Object { $_.Source -ceq 'download' -and $_.State -cne 'verified' }) {
        Receive-File -Label $file.Path -Url $file.Url -Size $file.Size -Sha256 $file.Sha256 -Target $file.Target
        $file.State = 'downloaded'
    }

    # Every file in place and verified, or the run fails.
    foreach ($file in $files) {
        if ($file.State -cne 'verified' -and $file.State -cne 'downloaded') {
            Stop-VisionRoot "$($file.Path): was neither verified nor placed"
        }
        $state = Test-PlacedFile -Path $file.Target -Label $file.Path -Size $file.Size -Sha256 $file.Sha256
        if ($state -cne 'verified') {
            Stop-VisionRoot "$($file.Path): after placement $state"
        }
    }

    [ordered]@{
        execution_provider = 'cpu'
        destination = $root
        registry = [ordered]@{ path = $registryPath; sha256 = $registrySha256 }
        files = @($files | ForEach-Object {
            [ordered]@{
                path = $_.Path
                size = [long]$_.Size
                sha256 = $_.Sha256
                state = $_.State
                component = $_.Component
            }
        })
    } | ConvertTo-Json -Depth 6 -Compress
}

$exitCode = 1
try {
    $line = Invoke-VisionRoot
    Write-Output $line
    $exitCode = 0
}
catch {
    [Console]::Error.WriteLine("Materialize-VisionRoot: FAILED: $($_.Exception.Message)")
}
finally {
    if ($null -ne $script:Http) { $script:Http.Dispose() }
}
exit $exitCode
