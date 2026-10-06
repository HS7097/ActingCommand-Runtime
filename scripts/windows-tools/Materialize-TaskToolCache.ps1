# SPDX-License-Identifier: AGPL-3.0-only

[CmdletBinding()]
param(
    [string] $TaskRoot = (Get-Location).Path,

    [Parameter(Mandatory)]
    [string] $CacheRoot,

    [Parameter(Mandatory)]
    [string[]] $Component,

    [string] $SourcesManifestPath = (Join-Path $PSScriptRoot 'windows-tool-sources.v1.json'),

    [switch] $AcceptAndroidSdkLicense,

    [ValidateSet('cpu', 'cuda')]
    [string] $OcrBackend,

    [Nullable[int]] $CudaDeviceOrdinal,

    [string] $CudaStableIdentity,

    [string] $MumuInstallRoot,

    [string] $MumuVersion,

    [Parameter(DontShow)]
    [string] $PrivateDownloadSourcePath,

    [Parameter(DontShow)]
    [Nullable[int]] $PrivateDownloadDeadlineMilliseconds,

    [Parameter(DontShow)]
    [switch] $PrivateDownloadStallBody
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Stop-Materialization {
    param([Parameter(Mandatory)][string] $Message)
    throw [InvalidOperationException]::new($Message)
}

function Assert-LowerSha256 {
    param(
        [Parameter(Mandatory)][string] $Value,
        [Parameter(Mandatory)][string] $Label
    )
    if ($Value -cnotmatch '^[0-9a-f]{64}$') {
        Stop-Materialization "$Label must be exactly 64 lowercase hexadecimal characters"
    }
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string] $Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Add-BoundedInt64 {
    param(
        [Parameter(Mandatory)][long] $Left,
        [Parameter(Mandatory)][long] $Right,
        [Parameter(Mandatory)][string] $Label
    )
    if ($Left -lt 0 -or $Right -lt 0 -or $Left -gt ([long]::MaxValue - $Right)) {
        Stop-Materialization "$Label exceeds the supported Int64 byte bound"
    }
    $Left + $Right
}

function Test-PathWithin {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Root
    )
    $pathFull = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    $pathFull.Equals($rootFull, [StringComparison]::OrdinalIgnoreCase) -or
        $pathFull.StartsWith($rootFull + '\', [StringComparison]::OrdinalIgnoreCase)
}

function Get-SafeChildPath {
    param(
        [Parameter(Mandatory)][string] $Root,
        [Parameter(Mandatory)][string] $RelativePath,
        [Parameter(Mandatory)][string] $Label
    )
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or
        [IO.Path]::IsPathFullyQualified($RelativePath) -or
        $RelativePath.Contains(':')) {
        Stop-Materialization "$Label must be a non-empty relative path"
    }
    $segments = $RelativePath.Replace('/', '\').Split(
        [char[]]@('\'),
        [StringSplitOptions]::RemoveEmptyEntries
    )
    if ($segments.Count -eq 0 -or $segments.Where({ $_ -eq '.' -or $_ -eq '..' }).Count -ne 0) {
        Stop-Materialization "$Label contains an unsafe path segment: $RelativePath"
    }
    $candidate = [IO.Path]::GetFullPath((Join-Path $Root ($segments -join '\')))
    if (-not (Test-PathWithin -Path $candidate -Root $Root)) {
        Stop-Materialization "$Label escapes its controlled root: $RelativePath"
    }
    $candidate
}

function Get-RegularFile {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Label
    )
    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
    if ($item.PSIsContainer -or (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        Stop-Materialization "$Label must be a regular non-reparse file: $Path"
    }
    $item.FullName
}

function Assert-TaskCacheRoot {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Boundary
    )
    if (-not [IO.Path]::IsPathFullyQualified($Boundary) -or
        -not [IO.Path]::IsPathFullyQualified($Path)) {
        Stop-Materialization 'TaskRoot and CacheRoot must be absolute task-local D-drive paths'
    }
    $taskRoot = [IO.Path]::GetFullPath($Boundary).TrimEnd('\')
    if (-not $taskRoot.StartsWith('D:\', [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $taskRoot -PathType Container)) {
        Stop-Materialization 'TaskRoot must be an existing D-drive directory'
    }
    $taskRootItem = Get-Item -LiteralPath $taskRoot -Force -ErrorAction Stop
    if (($taskRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        Stop-Materialization 'TaskRoot must not be a reparse point'
    }
    if (-not [IO.Path]::IsPathFullyQualified($Path)) {
        Stop-Materialization 'CacheRoot must be an absolute task-local D-drive path'
    }
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    if (-not $full.StartsWith('D:\', [StringComparison]::OrdinalIgnoreCase) -or $full -eq 'D:') {
        Stop-Materialization 'CacheRoot must be below the D-drive root'
    }
    if (Test-PathWithin -Path $full -Root 'D:\项目仓库') {
        Stop-Materialization 'CacheRoot must not be in the shared read-only mirror'
    }
    if (-not (Test-PathWithin -Path $full -Root $taskRoot) -or
        $full -ceq $taskRoot) {
        Stop-Materialization 'CacheRoot must be a strict child of TaskRoot'
    }
    foreach ($temporaryRoot in @($env:TEMP, $env:TMP)) {
        if (-not [string]::IsNullOrWhiteSpace($temporaryRoot) -and
            (Test-PathWithin -Path $full -Root $temporaryRoot)) {
            Stop-Materialization 'CacheRoot must not be in system TEMP or TMP'
        }
    }
    if (-not (Test-Path -LiteralPath $full)) {
        New-Item -ItemType Directory -Path $full -ErrorAction Stop | Out-Null
    }
    $resolved = (Resolve-Path -LiteralPath $full -ErrorAction Stop).ProviderPath
    if (-not $resolved.StartsWith('D:\', [StringComparison]::OrdinalIgnoreCase) -or
        (Test-PathWithin -Path $resolved -Root 'D:\项目仓库') -or
        -not (Test-PathWithin -Path $resolved -Root $taskRoot)) {
        Stop-Materialization 'resolved CacheRoot must remain on the task-local D-drive path'
    }
    $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
    if (-not $item.PSIsContainer -or (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        Stop-Materialization "CacheRoot must be a regular directory: $full"
    }
    $item.FullName.TrimEnd('\')
}

function Invoke-BoundedDownload {
    param(
        [Parameter(Mandatory)][string] $Url,
        [Parameter(Mandatory)][string] $Destination,
        [Parameter(Mandatory)][string] $ControlledRoot,
        [Parameter(Mandatory)][long] $ExpectedSize,
        [Parameter(Mandatory)][string] $ExpectedSha256,
        [TimeSpan] $Deadline = [TimeSpan]::FromMinutes(5),
        [string] $PrivateSourcePath,
        [switch] $PrivateStallBeforeBodyRead
    )
    if ($ExpectedSize -le 0) {
        Stop-Materialization "download size must be positive for $Url"
    }
    if ($Deadline -le [TimeSpan]::Zero -or $Deadline -gt [TimeSpan]::FromMinutes(5)) {
        Stop-Materialization "download deadline must be positive and no greater than five minutes for $Url"
    }
    Assert-LowerSha256 -Value $ExpectedSha256 -Label "sha256 for $Url"
    $uri = [Uri]$Url
    if ($uri.Scheme -cne 'https') {
        Stop-Materialization "downloads require HTTPS: $Url"
    }
    if (-not [string]::IsNullOrWhiteSpace($PrivateSourcePath) -and
        $uri.Host -cne 'example.invalid') {
        Stop-Materialization 'private download controls require the reserved example.invalid host'
    }
    $parent = Split-Path -Parent $Destination
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    if (Test-Path -LiteralPath $Destination) {
        Stop-Materialization "refusing to replace an existing download destination: $Destination"
    }
    $partialPath = "$Destination.partial-$([Guid]::NewGuid().ToString('N'))"
    $deadlineMilliseconds = [long][Math]::Ceiling($Deadline.TotalMilliseconds)
    $cancellation = [Threading.CancellationTokenSource]::new($Deadline)
    $handler = $null
    $client = $null
    $response = $null
    $input = $null
    $output = $null
    $published = $false
    $actualHash = $null
    try {
        if ([string]::IsNullOrWhiteSpace($PrivateSourcePath)) {
            $handler = [Net.Http.HttpClientHandler]::new()
            $client = [Net.Http.HttpClient]::new($handler)
            $client.Timeout = [Threading.Timeout]::InfiniteTimeSpan
            $response = $client.GetAsync(
                $uri,
                [Net.Http.HttpCompletionOption]::ResponseHeadersRead,
                $cancellation.Token
            ).GetAwaiter().GetResult()
            if (-not $response.IsSuccessStatusCode) {
                Stop-Materialization "download failed for $Url with HTTP $([int]$response.StatusCode)"
            }
            $declaredLength = $response.Content.Headers.ContentLength
            if ($null -ne $declaredLength -and $declaredLength -ne $ExpectedSize) {
                Stop-Materialization "Content-Length mismatch for ${Url}: expected=$ExpectedSize actual=$declaredLength"
            }
            $cancellation.Token.ThrowIfCancellationRequested()
            $input = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
            $cancellation.Token.ThrowIfCancellationRequested()
        } else {
            $source = Get-RegularFile -Path $PrivateSourcePath -Label 'private download source'
            $sourceSize = (Get-Item -LiteralPath $source -ErrorAction Stop).Length
            if ($sourceSize -ne $ExpectedSize) {
                Stop-Materialization "private download source size mismatch for ${Url}: expected=$ExpectedSize actual=$sourceSize"
            }
            $input = [IO.FileStream]::new(
                $source,
                [IO.FileMode]::Open,
                [IO.FileAccess]::Read,
                [IO.FileShare]::Read
            )
        }
        $output = [IO.File]::Open($partialPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        if ($PrivateStallBeforeBodyRead.IsPresent) {
            [Threading.Tasks.Task]::Delay(
                [Threading.Timeout]::Infinite,
                $cancellation.Token
            ).GetAwaiter().GetResult()
        }
        $buffer = [byte[]]::new(65536)
        [long]$written = 0
        while ($true) {
            $cancellation.Token.ThrowIfCancellationRequested()
            $read = $input.ReadAsync(
                $buffer,
                0,
                $buffer.Length,
                $cancellation.Token
            ).GetAwaiter().GetResult()
            if ($read -eq 0) { break }
            $written = Add-BoundedInt64 -Left $written -Right ([long]$read) -Label 'download'
            if ($written -gt $ExpectedSize) {
                Stop-Materialization "download exceeded its exact byte bound for $Url"
            }
            $output.Write($buffer, 0, $read)
        }
        $output.Flush($true)
        $output.Dispose()
        $output = $null
        if ($written -ne $ExpectedSize) {
            Stop-Materialization "download size mismatch for ${Url}: expected=$ExpectedSize actual=$written"
        }
        $cancellation.Token.ThrowIfCancellationRequested()
        $actualHash = Get-Sha256 -Path $partialPath
        if ($actualHash -cne $ExpectedSha256) {
            Stop-Materialization "SHA-256 mismatch for ${Url}: expected=$ExpectedSha256 actual=$actualHash"
        }
        $cancellation.Token.ThrowIfCancellationRequested()
        Move-Item -LiteralPath $partialPath -Destination $Destination -ErrorAction Stop
        $published = $true
    }
    catch [OperationCanceledException] {
        throw [TimeoutException]::new(
            "download timed out for $Url after deadline ${deadlineMilliseconds}ms",
            $_.Exception
        )
    }
    finally {
        if ($null -ne $output) { $output.Dispose() }
        if ($null -ne $input) { $input.Dispose() }
        if ($null -ne $response) { $response.Dispose() }
        if ($null -ne $client) { $client.Dispose() }
        if ($null -ne $handler) { $handler.Dispose() }
        $cancellation.Dispose()
        if (-not $published -and (Test-Path -LiteralPath $partialPath)) {
            Remove-Item -LiteralPath $partialPath -Force -ErrorAction Stop
        }
    }
    [ordered]@{
        url = $Url
        relative_path = [IO.Path]::GetRelativePath($ControlledRoot, $Destination).Replace('\', '/')
        size = $ExpectedSize
        sha256 = $actualHash
        executed = $false
    }
}

function Expand-BoundedZip {
    param(
        [Parameter(Mandatory)][string] $ArchivePath,
        [Parameter(Mandatory)][string] $DestinationRoot,
        [Parameter(Mandatory)][long] $MaximumBytes,
        [Parameter(Mandatory)][int] $MaximumFiles,
        [string[]] $AllowedFiles
    )
    if ($MaximumBytes -le 0 -or $MaximumFiles -le 0) {
        Stop-Materialization 'ZIP extraction byte and file bounds must be positive'
    }
    $allowed = [Collections.Generic.Dictionary[string, string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    foreach ($allowedFile in $AllowedFiles) {
        $normalized = ([string]$allowedFile).Replace('\', '/')
        Get-SafeChildPath -Root $DestinationRoot -RelativePath $normalized -Label 'ZIP allowlist entry' | Out-Null
        if ($allowed.ContainsKey($normalized)) {
            Stop-Materialization "ZIP allowlist contains a duplicate or case collision: $normalized"
        }
        $allowed.Add($normalized, $normalized)
    }
    $seen = [Collections.Generic.Dictionary[string, string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    New-Item -ItemType Directory -Path $DestinationRoot -ErrorAction Stop | Out-Null
    $archive = [IO.Compression.ZipFile]::OpenRead($ArchivePath)
    [long]$total = 0
    [int]$fileCount = 0
    try {
        foreach ($entry in $archive.Entries) {
            $unixType = (($entry.ExternalAttributes -shr 16) -band 0xF000)
            if ($unixType -eq 0xA000) {
                Stop-Materialization "ZIP contains a symbolic link: $($entry.FullName)"
            }
            if ([string]::IsNullOrEmpty($entry.FullName)) {
                if ([long]$entry.Length -ne 0) {
                    Stop-Materialization 'ZIP unnamed root entry must be empty'
                }
                continue
            }
            $target = Get-SafeChildPath -Root $DestinationRoot -RelativePath $entry.FullName -Label 'ZIP entry'
            if ([string]::IsNullOrEmpty($entry.Name)) {
                continue
            }
            $entryName = $entry.FullName.Replace('\', '/')
            if ($allowed.Count -gt 0 -and -not $allowed.ContainsKey($entryName)) {
                continue
            }
            if ($seen.ContainsKey($entryName)) {
                Stop-Materialization "ZIP contains a duplicate or case-colliding selected path: $entryName"
            }
            $seen.Add($entryName, $entryName)
            $fileCount++
            if ($fileCount -gt $MaximumFiles) {
                Stop-Materialization "ZIP extraction exceeds the bounded maximum of $MaximumFiles files"
            }
            $total = Add-BoundedInt64 -Left $total -Right ([long]$entry.Length) -Label 'ZIP extraction'
            if ($total -gt $MaximumBytes) {
                Stop-Materialization "ZIP extraction exceeds the bounded maximum of $MaximumBytes bytes"
            }
            New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
            $source = $entry.Open()
            $destination = [IO.File]::Open($target, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
            try {
                $source.CopyTo($destination)
                $destination.Flush($true)
            }
            finally {
                $destination.Dispose()
                $source.Dispose()
            }
        }
    }
    finally {
        $archive.Dispose()
    }
    if ($allowed.Count -gt 0 -and $seen.Count -ne $allowed.Count) {
        $missing = @($allowed.Keys | Where-Object { -not $seen.ContainsKey($_) } | Sort-Object)
        Stop-Materialization "ZIP is missing allowlisted files: $($missing -join ', ')"
    }
    $total
}

function Get-ExtractedFileRecords {
    param(
        [Parameter(Mandatory)][string] $DestinationRoot,
        [Parameter(Mandatory)][string] $StageRoot,
        [Parameter(Mandatory)][string] $Source,
        [Parameter(Mandatory)][string] $Version,
        [Parameter(Mandatory)][string] $LicenseProvenanceNote
    )
    @(
        Get-ChildItem -LiteralPath $DestinationRoot -Recurse -Force -File |
            Sort-Object FullName |
            ForEach-Object {
                if (($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    Stop-Materialization "extracted file must not be a reparse point: $($_.FullName)"
                }
                [ordered]@{
                    source = $Source
                    version = $Version
                    expected_name = $_.Name
                    size_bytes = [long]$_.Length
                    sha256 = (Get-Sha256 -Path $_.FullName)
                    license_provenance_note = $LicenseProvenanceNote
                    cache_path = [IO.Path]::GetRelativePath($StageRoot, $_.FullName).Replace('\', '/')
                    executed = $false
                }
            }
    )
}

function Get-InstalledFileAttestation {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Label,
        [Parameter(Mandatory)][string] $VersionIdentity,
        [Parameter(Mandatory)][string] $LicenseProvenanceNote
    )
    $file = Get-RegularFile -Path $Path -Label $Label
    $fileVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($file)
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    [ordered]@{
        source = $file
        path = $file
        version = $VersionIdentity
        expected_name = [IO.Path]::GetFileName($file)
        size_bytes = (Get-Item -LiteralPath $file).Length
        sha256 = (Get-Sha256 -Path $file)
        license_provenance_note = $LicenseProvenanceNote
        cache_path = $null
        file_version = $fileVersion.FileVersion
        product_version = $fileVersion.ProductVersion
        authenticode_status = [string]$signature.Status
        signer_subject = if ($null -eq $signature.SignerCertificate) { $null } else { $signature.SignerCertificate.Subject }
        signer_thumbprint = if ($null -eq $signature.SignerCertificate) { $null } else { $signature.SignerCertificate.Thumbprint }
        copied = $false
        executed = $false
    }
}

function Get-MumuNemuAttestation {
    if ([string]::IsNullOrWhiteSpace($MumuInstallRoot) -or [string]::IsNullOrWhiteSpace($MumuVersion)) {
        Stop-Materialization 'mumu-nemu-installed requires explicit MumuInstallRoot and MumuVersion'
    }
    if ($MumuVersion -cnotmatch '^[A-Za-z0-9._-]+$') {
        Stop-Materialization 'MumuVersion contains an unsafe character'
    }
    $resolvedRoot = (Resolve-Path -LiteralPath $MumuInstallRoot -ErrorAction Stop).ProviderPath
    $rootItem = Get-Item -LiteralPath $resolvedRoot -Force -ErrorAction Stop
    if (-not $rootItem.PSIsContainer -or (($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) {
        Stop-Materialization 'MumuInstallRoot must be a regular directory'
    }
    $root = $rootItem.FullName.TrimEnd('\')
    $versionedAdb = Get-SafeChildPath -Root $root -RelativePath "nx_device/$MumuVersion/shell/adb.exe" -Label 'versioned MuMu ADB'
    $sharedAdb = Get-SafeChildPath -Root $root -RelativePath 'nx_main/adb.exe' -Label 'shared MuMu ADB'
    $adb = if (Test-Path -LiteralPath $versionedAdb -PathType Leaf) { $versionedAdb } elseif (Test-Path -LiteralPath $sharedAdb -PathType Leaf) { $sharedAdb } else {
        Stop-Materialization 'no exact installed MuMu ADB candidate exists for the selected version'
    }
    $dll = Get-SafeChildPath -Root $root -RelativePath "nx_device/$MumuVersion/shell/sdk/external_renderer_ipc.dll" -Label 'Nemu IPC DLL'
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) {
        Stop-Materialization 'the exact installed Nemu IPC DLL does not exist for the selected version'
    }
    [ordered]@{
        install_root = $root
        version_identity = $MumuVersion
        adb = Get-InstalledFileAttestation -Path $adb -Label 'MuMu ADB' -VersionIdentity $MumuVersion -LicenseProvenanceNote 'Installed-only MuMu/Nemu file; redistribution is prohibited without separate permission.'
        capture_dll = Get-InstalledFileAttestation -Path $dll -Label 'Nemu IPC DLL' -VersionIdentity $MumuVersion -LicenseProvenanceNote 'Installed-only MuMu/Nemu file; redistribution is prohibited without separate permission.'
        discovery = 'explicit-installed-root-and-version-only'
        redistribution = 'prohibited_without_separate_permission'
        copied_vendor_files = $false
        executed_vendor_files = $false
    }
}

$manifestFile = Get-RegularFile -Path $SourcesManifestPath -Label 'tool source manifest'
$manifestHash = Get-Sha256 -Path $manifestFile
$manifest = Get-Content -LiteralPath $manifestFile -Raw | ConvertFrom-Json -Depth 100
if ($manifest.schema_version -cne 'actingcommand.windows_tool_sources.v1') {
    Stop-Materialization "unsupported source manifest schema_version: $($manifest.schema_version)"
}
$cacheRootFull = Assert-TaskCacheRoot -Path $CacheRoot -Boundary $TaskRoot
$privateControlSelected = -not [string]::IsNullOrWhiteSpace($PrivateDownloadSourcePath) -or
    $null -ne $PrivateDownloadDeadlineMilliseconds -or
    $PrivateDownloadStallBody.IsPresent
$downloadControl = @{ Deadline = [TimeSpan]::FromMinutes(5) }
if ($privateControlSelected) {
    if ([string]::IsNullOrWhiteSpace($PrivateDownloadSourcePath) -or
        $null -eq $PrivateDownloadDeadlineMilliseconds) {
        Stop-Materialization 'private download controls require source path and short deadline together'
    }
    if ([int]$PrivateDownloadDeadlineMilliseconds -le 0 -or
        [int]$PrivateDownloadDeadlineMilliseconds -gt 5000) {
        Stop-Materialization 'private download deadline must be between 1 and 5000 milliseconds'
    }
    $privateSource = Get-RegularFile -Path $PrivateDownloadSourcePath -Label 'private download source'
    if (-not (Test-PathWithin -Path $privateSource -Root $TaskRoot) -or
        (Test-PathWithin -Path $privateSource -Root $cacheRootFull)) {
        Stop-Materialization 'private download source must be task-local and outside CacheRoot'
    }
    $downloadControl = @{
        Deadline = [TimeSpan]::FromMilliseconds([int]$PrivateDownloadDeadlineMilliseconds)
        PrivateSourcePath = $privateSource
        PrivateStallBeforeBodyRead = $PrivateDownloadStallBody.IsPresent
    }
}
$selected = @($Component | Sort-Object -Unique)
if ($selected.Count -ne $Component.Count -or $selected.Count -eq 0) {
    Stop-Materialization 'Component must contain one or more unique component ids'
}
$componentTable = @{}
foreach ($property in $manifest.components.PSObject.Properties) {
    $componentTable[$property.Name] = $property.Value
}
foreach ($id in $selected) {
    if (-not $componentTable.ContainsKey($id)) {
        Stop-Materialization "unknown component id: $id"
    }
}
if ($selected -contains 'platform-tools-37.0.1' -and -not $AcceptAndroidSdkLicense.IsPresent) {
    Stop-Materialization 'platform-tools-37.0.1 requires explicit -AcceptAndroidSdkLicense'
}
$needsBackend = $selected -contains 'ppocrv6-medium-onnx'
if ($needsBackend -and [string]::IsNullOrWhiteSpace($OcrBackend)) {
    Stop-Materialization 'OCR components require an explicit -OcrBackend cpu or cuda; no automatic fallback exists'
}
if (-not [string]::IsNullOrWhiteSpace($OcrBackend) -and
    $OcrBackend -cnotin @('cpu', 'cuda')) {
    Stop-Materialization 'OcrBackend must use exact lowercase cpu or cuda'
}
if ($OcrBackend -ceq 'cuda' -and ($null -eq $CudaDeviceOrdinal -or [string]::IsNullOrWhiteSpace($CudaStableIdentity))) {
    Stop-Materialization 'CUDA selection requires an explicit ordinal and stable identity'
}
if ($OcrBackend -ceq 'cuda' -and [int]$CudaDeviceOrdinal -lt 0) {
    Stop-Materialization 'CUDA ordinal must be nonnegative'
}
if ($OcrBackend -ceq 'cpu' -and ($null -ne $CudaDeviceOrdinal -or -not [string]::IsNullOrWhiteSpace($CudaStableIdentity))) {
    Stop-Materialization 'CPU selection must not carry CUDA selector data'
}

[long]$declaredDownloadBytes = 0
foreach ($id in $selected) {
    $definition = $componentTable[$id]
    if ($definition.kind -ceq 'download_zip') {
        $declaredDownloadBytes = Add-BoundedInt64 -Left $declaredDownloadBytes -Right ([long]$definition.archive.size) -Label 'selected downloads'
    } elseif ($definition.kind -ceq 'download_files') {
        foreach ($artifact in $definition.artifacts) {
            $declaredDownloadBytes = Add-BoundedInt64 -Left $declaredDownloadBytes -Right ([long]$artifact.size) -Label 'selected downloads'
        }
    }
}
if ($declaredDownloadBytes -gt [long]$manifest.cache_layout.max_total_download_bytes) {
    Stop-Materialization 'selected components exceed the manifest total download bound'
}

$blockingPendingReasons = @()
if ($selected -contains 'ppocrv6-medium-source') {
    $blockingPendingReasons += [string]$componentTable['ppocrv6-medium-source'].compatibility.reason
}
if ($selected -contains 'ppocrv6-medium-onnx') {
    $blockingPendingReasons += [string]$componentTable['ppocrv6-medium-onnx'].compatibility.reason
}
$state = if ($blockingPendingReasons.Count -eq 0) { 'Ready' } else { 'PendingVerification' }
$suffix = if ($state -ceq 'Ready') { 'ready' } else { 'pending-verification' }
$finalLeaf = ([string]$manifest.cache_layout.directory_name) + '.' + $suffix
$finalRoot = Join-Path $cacheRootFull $finalLeaf
if (Test-Path -LiteralPath $finalRoot) {
    Stop-Materialization "refusing to replace an existing cache directory: $finalRoot"
}

$stageRoot = Join-Path $cacheRootFull ('.stage-' + [Guid]::NewGuid().ToString('N'))
$publishedRoot = $null
New-Item -ItemType Directory -Path $stageRoot -ErrorAction Stop | Out-Null
try {
    $componentResults = [ordered]@{}
    foreach ($id in $selected) {
        $definition = $componentTable[$id]
        switch ([string]$definition.kind) {
            'download_zip' {
                $archive = $definition.archive
                $archivePath = Get-SafeChildPath -Root $stageRoot -RelativePath ([string]$archive.relative_path) -Label "$id archive"
                $downloadResult = Invoke-BoundedDownload -Url ([string]$archive.url) -Destination $archivePath -ControlledRoot $stageRoot -ExpectedSize ([long]$archive.size) -ExpectedSha256 ([string]$archive.sha256) @downloadControl
                $extractRoot = Get-SafeChildPath -Root $stageRoot -RelativePath ([string]$definition.extract_relative_path) -Label "$id extraction root"
                $allowlist = if ($null -eq $definition.PSObject.Properties['extract_allowlist']) {
                    @()
                } else {
                    @($definition.extract_allowlist | ForEach-Object { [string]$_ })
                }
                $extractedBytes = Expand-BoundedZip `
                    -ArchivePath $archivePath `
                    -DestinationRoot $extractRoot `
                    -MaximumBytes ([long]$definition.max_extract_bytes) `
                    -MaximumFiles ([int]$definition.max_extract_file_count) `
                    -AllowedFiles $allowlist
                foreach ($required in $definition.required_files) {
                    $requiredPath = Get-SafeChildPath -Root $extractRoot -RelativePath ([string]$required) -Label "$id required file"
                    Get-RegularFile -Path $requiredPath -Label "$id required file" | Out-Null
                }
                $licenseNote = "$($definition.license.id); $($definition.license.url); redistribution=$($definition.license.redistribution)"
                $componentResults[$id] = [ordered]@{
                    download = [ordered]@{
                        source = [string]$downloadResult.url
                        version = [string]$definition.version
                        expected_name = [IO.Path]::GetFileName([string]$archive.relative_path)
                        size_bytes = [long]$downloadResult.size
                        sha256 = [string]$downloadResult.sha256
                        license_provenance_note = $licenseNote
                        provenance_note = [string]$archive.hash_provenance
                        cache_path = [string]$downloadResult.relative_path
                        executed = $false
                    }
                    extracted_files = Get-ExtractedFileRecords -DestinationRoot $extractRoot -StageRoot $stageRoot -Source ([string]$archive.url) -Version ([string]$definition.version) -LicenseProvenanceNote $licenseNote
                    extracted_bytes = $extractedBytes
                    executed = $false
                }
            }
            'download_files' {
                $artifacts = @()
                foreach ($artifact in $definition.artifacts) {
                    $destination = Get-SafeChildPath -Root $stageRoot -RelativePath ([string]$artifact.relative_path) -Label "$id artifact"
                    $downloadResult = Invoke-BoundedDownload -Url ([string]$artifact.url) -Destination $destination -ControlledRoot $stageRoot -ExpectedSize ([long]$artifact.size) -ExpectedSha256 ([string]$artifact.sha256) @downloadControl
                    $artifacts += [ordered]@{
                        source = [string]$downloadResult.url
                        version = [string]$definition.version
                        expected_name = [IO.Path]::GetFileName([string]$artifact.relative_path)
                        size_bytes = [long]$downloadResult.size
                        sha256 = [string]$downloadResult.sha256
                        license_provenance_note = "$($definition.license.id); $($definition.license.url)"
                        provenance_note = [string]$artifact.hash_provenance
                        cache_path = [string]$downloadResult.relative_path
                        executed = $false
                    }
                }
                $componentResults[$id] = [ordered]@{ artifacts = $artifacts; executed = $false }
            }
            'installed_metadata_only' {
                $componentResults[$id] = Get-MumuNemuAttestation
            }
            default {
                Stop-Materialization "unsupported component kind for $id`: $($definition.kind)"
            }
        }
    }

    $licenses = [ordered]@{}
    foreach ($id in $selected) {
        $definition = $componentTable[$id]
        if ($null -ne $definition.PSObject.Properties['license']) {
            $licenses[$id] = $definition.license
        }
    }
    $provenance = [ordered]@{
        schema_version = 'actingcommand.task_tool_cache_provenance.v1'
        state = $state
        generated_at_utc = [DateTimeOffset]::UtcNow.ToString('O')
        source_manifest = [ordered]@{ path = $manifestFile; sha256 = $manifestHash }
        selected_components = $selected
        explicit_backend = if ($needsBackend) { $OcrBackend } else { $null }
        automatic_fallback = $false
        licenses = $licenses
        android_sdk_license_acknowledged = $AcceptAndroidSdkLicense.IsPresent
        downloaded_bytes = $declaredDownloadBytes
        components = $componentResults
        pending_verification = @($blockingPendingReasons)
        byte_materialization_ready = ($state -ceq 'Ready')
        functional_validation_performed = $false
        cleanup = [ordered]@{
            classification = [string]$manifest.cache_layout.cleanup_classification
            timing = [string]$manifest.cache_layout.cleanup_timing
            reproducible_from_manifest = $true
            preserve = @('PROVENANCE.json', 'source URLs', 'sizes', 'hashes', 'run evidence')
        }
        restrictions = [ordered]@{
            binaries_executed = $false
            global_path_modified = $false
            system_temp_used = $false
            shared_mirror_written = $false
            installed_mumu_nemu_files_copied = $false
            downloaded_or_caller_supplied_files_materialized = ($declaredDownloadBytes -gt 0)
        }
    }
    $provenancePath = Join-Path $stageRoot 'PROVENANCE.json'
    [IO.File]::WriteAllText(
        $provenancePath,
        ($provenance | ConvertTo-Json -Depth 30) + "`n",
        [Text.UTF8Encoding]::new($false)
    )
    Move-Item -LiteralPath $stageRoot -Destination $finalRoot -ErrorAction Stop
    $publishedRoot = $finalRoot
    if ($state -ceq 'PendingVerification') {
        Stop-Materialization "PendingVerification: materialized exact bytes at $finalRoot, but functional ONNX/provider compatibility is not proven; cache is not ready"
    }
    [pscustomobject]@{
        state = $state
        cache_root = $finalRoot
        provenance_path = (Join-Path $finalRoot 'PROVENANCE.json')
    }
}
finally {
    if ($null -eq $publishedRoot -and (Test-Path -LiteralPath $stageRoot)) {
        Remove-Item -LiteralPath $stageRoot -Recurse -Force -ErrorAction Stop
    }
}
