# SPDX-License-Identifier: AGPL-3.0-only

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
    [string]$Repository,

    [Parameter(Mandatory)]
    [ValidateScript({
        if ($_ -cnotmatch '^[0-9a-f]{40}$') {
            throw 'SourceSha must be exactly 40 lowercase hexadecimal characters.'
        }
        $true
    })]
    [string]$SourceSha,

    [Parameter(Mandatory)]
    [ValidateScript({
        if ($_ -cnotin @('Runtime', 'Tools')) {
            throw "ArtifactKind must be exactly 'Runtime' or 'Tools'."
        }
        $true
    })]
    [string]$ArtifactKind,

    [Parameter(Mandatory)]
    [string]$OutputPath,

    [Nullable[long]]$RunId,

    [string]$TaskRoot = (Get-Location).Path,

    [string]$GhExecutable = 'gh',

    [ValidateRange(1, 600)]
    [int]$GhTimeoutSeconds = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workflowName = 'Windows exact-SHA build'
$targetTriple = 'x86_64-pc-windows-msvc'
$configuration = 'release'
$artifactName = 'actingcommand-{0}-{1}' -f $ArtifactKind.ToLowerInvariant(), $SourceSha
$expectedFiles = if ($ArtifactKind -ceq 'Runtime') {
    @('actingcommand-actingd.exe', 'actingctl.exe')
}
else {
    @(
        'actinglab.exe',
        'actingcommand-vision-provider-check.exe',
        'actingcommand-device-test.exe'
    )
}

function Get-CanonicalPath {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Label
    )

    try {
        return [IO.Path]::GetFullPath($Path)
    }
    catch {
        throw "$Label is not a valid filesystem path: $($_.Exception.Message)"
    }
}

function Assert-DDriveTaskPath {
    param(
        [Parameter(Mandatory)]
        [string]$Candidate,

        [Parameter(Mandatory)]
        [string]$Root
    )

    if ([IO.Path]::GetPathRoot($Root) -cne 'D:\') {
        throw "TaskRoot must be on drive D: '$Root'."
    }
    if ([IO.Path]::GetPathRoot($Candidate) -cne 'D:\') {
        throw "OutputPath must be on drive D: '$Candidate'."
    }

    $rootPrefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) +
        [IO.Path]::DirectorySeparatorChar
    if (-not $Candidate.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "OutputPath must be a strict child of TaskRoot. Root='$Root'; output='$Candidate'."
    }
}

function Assert-NoReparsePoint {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Boundary
    )

    $current = Get-CanonicalPath -Path $Path -Label 'Path'
    $boundaryPath = Get-CanonicalPath -Path $Boundary -Label 'Boundary'
    $boundaryPrefix = $boundaryPath.TrimEnd([IO.Path]::DirectorySeparatorChar) +
        [IO.Path]::DirectorySeparatorChar

    while ($true) {
        if (
            $current -cne $boundaryPath -and
            -not $current.StartsWith($boundaryPrefix, [StringComparison]::OrdinalIgnoreCase)
        ) {
            throw "Path escaped its task boundary while checking reparse points: '$current'."
        }

        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Task path contains a reparse point: '$current'."
            }
        }

        if ($current -ceq $boundaryPath) {
            break
        }
        $parent = [IO.Directory]::GetParent($current)
        if ($null -eq $parent) {
            throw "Could not reach task boundary '$boundaryPath' from '$Path'."
        }
        $current = $parent.FullName
    }
}

function Resolve-GhApplication {
    param([Parameter(Mandatory)][string]$Command)

    if ([IO.Path]::IsPathRooted($Command)) {
        $item = Get-Item -LiteralPath $Command -Force -ErrorAction Stop
        if ($item.PSIsContainer) {
            throw "GhExecutable is a directory, not an executable: '$Command'."
        }
        return $item.FullName
    }

    $resolved = @(Get-Command $Command -CommandType Application -ErrorAction Stop)
    if ($resolved.Count -ne 1) {
        throw "GhExecutable must resolve to exactly one application; found $($resolved.Count)."
    }
    return $resolved[0].Source
}

function Invoke-GhProcess {
    param(
        [Parameter(Mandatory)]
        [string[]]$Arguments,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $script:GhPath
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw "failed to start '$($script:GhPath)'"
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($GhTimeoutSeconds * 1000)) {
            try {
                $process.Kill($true)
            }
            catch {
                $process.Kill()
            }
            throw "timed out after $GhTimeoutSeconds seconds"
        }
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) {
            throw "exit code $($process.ExitCode); stderr: $($stderr.Trim())"
        }
        return [pscustomobject]@{
            StdOut = $stdout
            StdErr = $stderr
        }
    }
    catch {
        throw "GitHub CLI $Context failed: $($_.Exception.Message)"
    }
    finally {
        $process.Dispose()
    }
}

function Invoke-GhJson {
    param(
        [Parameter(Mandatory)]
        [string[]]$Arguments,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $result = Invoke-GhProcess -Arguments $Arguments -Context $Context
    if ([string]::IsNullOrWhiteSpace($result.StdOut)) {
        throw "GitHub CLI $Context returned empty JSON output."
    }
    try {
        return $result.StdOut | ConvertFrom-Json -Depth 100
    }
    catch {
        throw "GitHub CLI $Context returned invalid JSON: $($_.Exception.Message)"
    }
}

function Assert-Property {
    param(
        [Parameter(Mandatory)]
        [object]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($Object.PSObject.Properties.Name -cnotcontains $Name) {
        throw "$Context is missing required property '$Name'."
    }
    return $Object.$Name
}

function Assert-ExactString {
    param(
        [Parameter(Mandatory)]
        [object]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Expected,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $actual = [string](Assert-Property -Object $Object -Name $Name -Context $Context)
    if ($actual -cne $Expected) {
        throw "$Context property '$Name' mismatch. Expected '$Expected'; received '$actual'."
    }
}

function Get-Sha256Hex {
    param([Parameter(Mandatory)][byte[]]$Bytes)

    $algorithm = [Security.Cryptography.SHA256]::Create()
    try {
        $hash = $algorithm.ComputeHash($Bytes)
        return ([BitConverter]::ToString($hash)).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $algorithm.Dispose()
    }
}

function Get-RunRecord {
    if ($null -ne $RunId) {
        if ([long]$RunId -lt 1) {
            throw 'RunId must be a positive GitHub Actions run database ID.'
        }
        return Invoke-GhJson -Context "run $RunId lookup" -Arguments @(
            'run', 'view', ([string][long]$RunId),
            '--repo', $Repository,
            '--json', 'databaseId,headSha,status,conclusion,workflowName,attempt,url'
        )
    }

    $runs = @(
        Invoke-GhJson -Context 'exact-SHA workflow run listing' -Arguments @(
            'run', 'list',
            '--repo', $Repository,
            '--workflow', $workflowName,
            '--commit', $SourceSha,
            '--limit', '1000',
            '--json', 'databaseId,headSha,status,conclusion,workflowName,attempt,url'
        )
    )
    if ($runs.Count -eq 1000) {
        throw 'Exact-SHA workflow run listing reached its bounded limit; selection is not provably complete.'
    }

    $matches = @(
        $runs | Where-Object {
            [string]$_.headSha -ceq $SourceSha -and
            [string]$_.workflowName -ceq $workflowName -and
            [string]$_.status -ceq 'completed' -and
            [string]$_.conclusion -ceq 'success'
        }
    )
    if ($matches.Count -ne 1) {
        throw "Expected exactly one completed successful '$workflowName' run at '$SourceSha'; found $($matches.Count). Supply -RunId to select one exact run."
    }
    return $matches[0]
}

$taskRootPath = Get-CanonicalPath -Path $TaskRoot -Label 'TaskRoot'
$outputFullPath = Get-CanonicalPath -Path $OutputPath -Label 'OutputPath'
if (-not (Test-Path -LiteralPath $taskRootPath -PathType Container)) {
    throw "TaskRoot does not exist or is not a directory: '$taskRootPath'."
}
Assert-DDriveTaskPath -Candidate $outputFullPath -Root $taskRootPath
Assert-NoReparsePoint -Path $taskRootPath -Boundary $taskRootPath
if (Test-Path -LiteralPath $outputFullPath) {
    throw "OutputPath already exists; overwrite is prohibited: '$outputFullPath'."
}

$outputParent = Split-Path -Parent $outputFullPath
if ([string]::IsNullOrWhiteSpace($outputParent)) {
    throw "OutputPath has no parent directory: '$outputFullPath'."
}
if (-not (Test-Path -LiteralPath $outputParent)) {
    [void](New-Item -ItemType Directory -Path $outputParent -ErrorAction Stop)
}
Assert-NoReparsePoint -Path $outputParent -Boundary $taskRootPath

$leafName = Split-Path -Leaf $outputFullPath
if ([string]::IsNullOrWhiteSpace($leafName)) {
    throw "OutputPath has no leaf name: '$outputFullPath'."
}
$stagePath = Join-Path $outputParent ".$leafName.download-$([Guid]::NewGuid().ToString('N'))"
Assert-DDriveTaskPath -Candidate $stagePath -Root $taskRootPath
if (Test-Path -LiteralPath $stagePath) {
    throw "Generated staging path already exists: '$stagePath'."
}

$script:GhPath = Resolve-GhApplication -Command $GhExecutable
$run = Get-RunRecord
Assert-ExactString -Object $run -Name 'headSha' -Expected $SourceSha -Context 'workflow run'
Assert-ExactString -Object $run -Name 'workflowName' -Expected $workflowName -Context 'workflow run'
Assert-ExactString -Object $run -Name 'status' -Expected 'completed' -Context 'workflow run'
Assert-ExactString -Object $run -Name 'conclusion' -Expected 'success' -Context 'workflow run'
$selectedRunId = [long](Assert-Property -Object $run -Name 'databaseId' -Context 'workflow run')
$selectedAttempt = [long](Assert-Property -Object $run -Name 'attempt' -Context 'workflow run')
if ($selectedRunId -lt 1 -or $selectedAttempt -lt 1) {
    throw 'Workflow run ID and attempt must both be positive integers.'
}
if ($null -ne $RunId -and $selectedRunId -ne [long]$RunId) {
    throw "Workflow run ID mismatch. Requested '$RunId'; received '$selectedRunId'."
}

$commit = Invoke-GhJson -Context 'exact commit lookup' -Arguments @(
    'api', "repos/$Repository/git/commits/$SourceSha"
)
Assert-ExactString -Object $commit -Name 'sha' -Expected $SourceSha -Context 'commit'
$commitTree = Assert-Property -Object $commit -Name 'tree' -Context 'commit'
$expectedTreeSha = [string](Assert-Property -Object $commitTree -Name 'sha' -Context 'commit tree')
if ($expectedTreeSha -cnotmatch '^[0-9a-f]{40}$') {
    throw "Commit API returned an invalid tree SHA: '$expectedTreeSha'."
}

$lockContent = Invoke-GhJson -Context 'Cargo.lock lookup' -Arguments @(
    'api', "repos/$Repository/contents/Cargo.lock?ref=$SourceSha"
)
Assert-ExactString -Object $lockContent -Name 'encoding' -Expected 'base64' -Context 'Cargo.lock content'
$encodedLock = [string](Assert-Property -Object $lockContent -Name 'content' -Context 'Cargo.lock content')
try {
    $lockBytes = [Convert]::FromBase64String(($encodedLock -replace '\s', ''))
}
catch {
    throw "Cargo.lock API content is not valid base64: $($_.Exception.Message)"
}
$expectedLockSha256 = Get-Sha256Hex -Bytes $lockBytes

$artifactPage = Invoke-GhJson -Context 'workflow artifact listing' -Arguments @(
    'api', "repos/$Repository/actions/runs/$selectedRunId/artifacts?per_page=100"
)
$artifactTotal = [long](Assert-Property -Object $artifactPage -Name 'total_count' -Context 'artifact listing')
$artifactRecords = Assert-Property -Object $artifactPage -Name 'artifacts' -Context 'artifact listing'
[object[]]$artifacts = @()
if ($null -ne $artifactRecords) {
    $artifacts = @($artifactRecords)
}
if ($artifactTotal -ne $artifacts.Count) {
    throw "Artifact listing is incomplete. API total is $artifactTotal; fetched $($artifacts.Count)."
}
$artifactMatches = @(
    $artifacts | Where-Object {
        [string]$_.name -ceq $artifactName -and -not [bool]$_.expired
    }
)
if ($artifactMatches.Count -ne 1) {
    throw "Expected exactly one non-expired artifact named '$artifactName' in run '$selectedRunId'; found $($artifactMatches.Count)."
}

$completed = $false
try {
    [void](New-Item -ItemType Directory -Path $stagePath -ErrorAction Stop)
    [void](Invoke-GhProcess -Context 'exact artifact download' -Arguments @(
        'run', 'download', ([string]$selectedRunId),
        '--repo', $Repository,
        '--name', $artifactName,
        '--dir', $stagePath
    ))

    Assert-NoReparsePoint -Path $stagePath -Boundary $taskRootPath
    $downloadedItems = @(Get-ChildItem -LiteralPath $stagePath -Recurse -Force -ErrorAction Stop)
    $reparseItems = @(
        $downloadedItems | Where-Object {
            ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
        }
    )
    if ($reparseItems.Count -ne 0) {
        throw "Downloaded artifact contains a reparse point: '$($reparseItems[0].FullName)'."
    }
    $directories = @($downloadedItems | Where-Object { $_.PSIsContainer })
    if ($directories.Count -ne 0) {
        throw "Downloaded artifact contains unexpected directories; first: '$($directories[0].FullName)'."
    }

    $manifestFiles = @(
        $downloadedItems | Where-Object {
            -not $_.PSIsContainer -and $_.Name -ieq 'BUILD-MANIFEST.json'
        }
    )
    if ($manifestFiles.Count -ne 1) {
        throw "Artifact must contain exactly one BUILD-MANIFEST.json; found $($manifestFiles.Count)."
    }
    $expectedManifestPath = Join-Path $stagePath 'BUILD-MANIFEST.json'
    if ($manifestFiles[0].FullName -cne $expectedManifestPath) {
        throw 'BUILD-MANIFEST.json must be at the artifact root with exact casing.'
    }

    try {
        $manifest = Get-Content -LiteralPath $expectedManifestPath -Raw -Encoding UTF8 |
            ConvertFrom-Json -Depth 100
    }
    catch {
        throw "BUILD-MANIFEST.json is not valid UTF-8 JSON: $($_.Exception.Message)"
    }

    Assert-ExactString -Object $manifest -Name 'repository' -Expected $Repository -Context 'manifest'
    Assert-ExactString -Object $manifest -Name 'commit_sha' -Expected $SourceSha -Context 'manifest'
    Assert-ExactString -Object $manifest -Name 'tree_sha' -Expected $expectedTreeSha -Context 'manifest'
    Assert-ExactString -Object $manifest -Name 'cargo_lock_sha256' -Expected $expectedLockSha256 -Context 'manifest'
    Assert-ExactString -Object $manifest -Name 'target' -Expected $targetTriple -Context 'manifest'
    Assert-ExactString -Object $manifest -Name 'configuration' -Expected $configuration -Context 'manifest'
    Assert-ExactString -Object $manifest -Name 'source_artifact_name' -Expected $artifactName -Context 'manifest'

    $rustToolchain = [string](Assert-Property -Object $manifest -Name 'rust_toolchain' -Context 'manifest')
    if ([string]::IsNullOrWhiteSpace($rustToolchain)) {
        throw 'Manifest rust_toolchain must be a nonempty exact tuple value.'
    }
    $manifestRunId = [long](Assert-Property -Object $manifest -Name 'workflow_run_id' -Context 'manifest')
    $manifestAttempt = [long](Assert-Property -Object $manifest -Name 'workflow_run_attempt' -Context 'manifest')
    if ($manifestRunId -ne $selectedRunId -or $manifestAttempt -ne $selectedAttempt) {
        throw "Manifest run binding mismatch. Expected run/attempt '$selectedRunId/$selectedAttempt'; received '$manifestRunId/$manifestAttempt'."
    }

    $declaredFiles = @(Assert-Property -Object $manifest -Name 'files' -Context 'manifest')
    if ($declaredFiles.Count -eq 0) {
        throw 'Manifest files must contain at least one payload record.'
    }
    $declaredByPath = [Collections.Generic.Dictionary[string, object]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    foreach ($record in $declaredFiles) {
        $relativePath = [string](Assert-Property -Object $record -Name 'path' -Context 'manifest file')
        if (
            [string]::IsNullOrWhiteSpace($relativePath) -or
            $relativePath.Contains('\') -or
            [IO.Path]::IsPathRooted($relativePath) -or
            @($relativePath.Split('/')) -contains '..' -or
            @($relativePath.Split('/')) -contains '.' -or
            $relativePath -ceq 'BUILD-MANIFEST.json'
        ) {
            throw "Manifest contains a non-canonical or reserved file path: '$relativePath'."
        }
        if ($declaredByPath.ContainsKey($relativePath)) {
            throw "Manifest contains a duplicate or case-colliding file path: '$relativePath'."
        }
        $sizeBytes = [long](Assert-Property -Object $record -Name 'size_bytes' -Context "manifest file '$relativePath'")
        if ($sizeBytes -lt 0) {
            throw "Manifest file '$relativePath' has a negative size."
        }
        $sha256 = [string](Assert-Property -Object $record -Name 'sha256' -Context "manifest file '$relativePath'")
        if ($sha256 -cnotmatch '^[0-9a-f]{64}$') {
            throw "Manifest file '$relativePath' has an invalid lowercase SHA-256."
        }
        $declaredByPath.Add($relativePath, $record)
    }

    if ($declaredByPath.Count -ne $expectedFiles.Count) {
        throw "Manifest payload count mismatch for '$ArtifactKind'. Expected $($expectedFiles.Count); received $($declaredByPath.Count)."
    }
    foreach ($expectedFile in $expectedFiles) {
        if (-not $declaredByPath.ContainsKey($expectedFile)) {
            throw "Manifest is missing required '$ArtifactKind' payload '$expectedFile'."
        }
        $declaredCase = [string]$declaredByPath[$expectedFile].path
        if ($declaredCase -cne $expectedFile) {
            throw "Manifest payload casing mismatch. Expected '$expectedFile'; received '$declaredCase'."
        }
    }

    $actualFiles = @($downloadedItems | Where-Object { -not $_.PSIsContainer })
    if ($actualFiles.Count -ne ($declaredByPath.Count + 1)) {
        throw "Artifact file count mismatch. Expected $($declaredByPath.Count + 1) including the manifest; received $($actualFiles.Count)."
    }
    $actualPayloadPaths = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    foreach ($file in $actualFiles) {
        if ($file.FullName -ceq $expectedManifestPath) {
            continue
        }
        $relativePath = [IO.Path]::GetRelativePath($stagePath, $file.FullName).Replace('\', '/')
        if ($relativePath.StartsWith('../', [StringComparison]::Ordinal)) {
            throw "Downloaded file escaped the staging root: '$($file.FullName)'."
        }
        if (-not $actualPayloadPaths.Add($relativePath)) {
            throw "Downloaded artifact contains duplicate or case-colliding path '$relativePath'."
        }
        if (-not $declaredByPath.ContainsKey($relativePath)) {
            throw "Downloaded artifact contains undeclared file '$relativePath'."
        }

        $record = $declaredByPath[$relativePath]
        if ([long]$file.Length -ne [long]$record.size_bytes) {
            throw "File size mismatch for '$relativePath'. Expected '$($record.size_bytes)'; received '$($file.Length)'."
        }
        $actualSha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        if ($actualSha256 -cne [string]$record.sha256) {
            throw "File SHA-256 mismatch for '$relativePath'. Expected '$($record.sha256)'; received '$actualSha256'."
        }
    }
    foreach ($declaredPath in $declaredByPath.Keys) {
        if (-not $actualPayloadPaths.Contains($declaredPath)) {
            throw "Downloaded artifact is missing declared file '$declaredPath'."
        }
    }

    if (Test-Path -LiteralPath $outputFullPath) {
        throw "OutputPath appeared during verification; overwrite is prohibited: '$outputFullPath'."
    }
    Move-Item -LiteralPath $stagePath -Destination $outputFullPath -ErrorAction Stop
    $completed = $true

    [pscustomobject]@{
        status = 'PASS'
        repository = $Repository
        commit_sha = $SourceSha
        tree_sha = $expectedTreeSha
        cargo_lock_sha256 = $expectedLockSha256
        rust_toolchain = $rustToolchain
        target = $targetTriple
        configuration = $configuration
        workflow_name = $workflowName
        workflow_run_id = $selectedRunId
        workflow_run_attempt = $selectedAttempt
        artifact_name = $artifactName
        artifact_kind = $ArtifactKind
        output_path = $outputFullPath
        verified_files = @($expectedFiles)
    } | ConvertTo-Json -Depth 5
}
finally {
    if (-not $completed -and (Test-Path -LiteralPath $stagePath)) {
        Remove-Item -LiteralPath $stagePath -Recurse -Force -ErrorAction Stop
    }
}
