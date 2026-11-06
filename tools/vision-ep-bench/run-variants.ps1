# SPDX-License-Identifier: AGPL-3.0-only
<#
.SYNOPSIS
Workflow #407 GPU-0a (experiment branch exp/407-gpu0-bench only, never merged): runs the
execution-provider variants of design model-gpu0-404 section 1.2 with ac-vision-ep-bench, one
process each (ONNX Runtime loads one onnxruntime.dll per process), and collects their reports.

.DESCRIPTION
The variants, in this order (cpu-1t first: every later variant compares with it):
  cpu-1t        --ep cpu                                   (production today, the baseline)
  cpu-4t        --ep cpu --cpu-threads 4
  cuda-default  --ep cuda --device <CudaDevice>            (production CUDA path as it is)
  cuda-tuned    --ep cuda --device <CudaDevice> --cuda-conv-search heuristic --cuda-tf32 off
  dml-3090ti    --ep directml --device <DmlNvidiaDevice>
  dml-radeon    --ep directml --device <DmlRadeonDevice>   (proves the adapter works only)

Each variant writes <OutDir>\<label>.json (the bench's report) and <OutDir>\<label>.log (its
stderr). The script also writes <OutDir>\adapters.json (the DXGI list, to check the DirectML
indices) and <OutDir>\index.json (every variant's exit status and report status). OutDir must not
exist: nothing is ever overwritten. A variant that fails does not stop the others; the script
exits 1 when any variant failed. A requested variant whose root or device is missing is refused
before anything runs.

Workflow #407 GPU-0b: -Mode template runs the bench's template mode (--mode template) with a
workload that carries template_search (workload-gpu0b.json). Reps then defaults to 1 (repetitions
per frame), no models folder or baseline is passed, and today's direct enumeration, which does not
depend on the execution provider, runs in cpu-1t only (--direct-enumeration off elsewhere).

.EXAMPLE
.\run-variants.ps1 -Exe .\ac-vision-ep-bench.exe -Workload .\workload-gpu0a.json `
  -OutDir .\out\round1-idle -CpuVisionRoot .\rt-cpu -CudaVisionRoot .\rt-cuda12 `
  -DmlVisionRoot .\rt-dml -ModelsDir .\models -CudaDevice 0 -DmlNvidiaDevice 0 -DmlRadeonDevice 1

.EXAMPLE
.\run-variants.ps1 -Mode template -Exe .\ac-vision-ep-bench.exe -Workload .\workload-gpu0b.json `
  -OutDir .\out\round1-idle-template -CpuVisionRoot .\rt-cpu -CudaVisionRoot .\rt-cuda12 `
  -DmlVisionRoot .\rt-dml -CudaDevice 0 -DmlNvidiaDevice 0 -DmlRadeonDevice 1
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Exe,
    [Parameter(Mandatory = $true)][string] $Workload,
    [Parameter(Mandatory = $true)][string] $OutDir,
    [Parameter(Mandatory = $true)][string] $CpuVisionRoot,
    [string] $CudaVisionRoot = '',
    [string] $DmlVisionRoot = '',
    [string] $ModelsDir = '',
    [int] $CudaDevice = -1,
    [int] $DmlNvidiaDevice = -1,
    [int] $DmlRadeonDevice = -1,
    [int] $Warmup = 3,
    [int] $Reps = 20,
    [string[]] $Variants = @('cpu-1t', 'cpu-4t', 'cuda-default', 'cuda-tuned', 'dml-3090ti', 'dml-radeon'),
    [ValidateSet('ocr', 'template')][string] $Mode = 'ocr'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if ($Mode -eq 'template' -and -not $PSBoundParameters.ContainsKey('Reps')) {
    $Reps = 1
}

$table = [ordered]@{
    'cpu-1t'       = @{ Root = $CpuVisionRoot; Device = -2; Extra = @('--ep', 'cpu') }
    'cpu-4t'       = @{ Root = $CpuVisionRoot; Device = -2; Extra = @('--ep', 'cpu', '--cpu-threads', '4') }
    'cuda-default' = @{ Root = $CudaVisionRoot; Device = $CudaDevice; Extra = @('--ep', 'cuda') }
    'cuda-tuned'   = @{ Root = $CudaVisionRoot; Device = $CudaDevice; Extra = @('--ep', 'cuda', '--cuda-conv-search', 'heuristic', '--cuda-tf32', 'off') }
    'dml-3090ti'   = @{ Root = $DmlVisionRoot; Device = $DmlNvidiaDevice; Extra = @('--ep', 'directml') }
    'dml-radeon'   = @{ Root = $DmlVisionRoot; Device = $DmlRadeonDevice; Extra = @('--ep', 'directml') }
}

foreach ($label in $Variants) {
    if (-not $table.Contains($label)) {
        throw "unknown variant '$label'; known: $($table.Keys -join ', ')"
    }
    $entry = $table[$label]
    if ([string]::IsNullOrWhiteSpace($entry.Root)) {
        throw "variant '$label' needs its vision root parameter"
    }
    if ($entry.Device -eq -1) {
        throw "variant '$label' needs its device parameter (-CudaDevice, -DmlNvidiaDevice or -DmlRadeonDevice)"
    }
}
if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
    throw "the bench executable is missing: $Exe"
}
if (-not (Test-Path -LiteralPath $Workload -PathType Leaf)) {
    throw "the workload is missing: $Workload"
}
if (Test-Path -LiteralPath $OutDir) {
    throw "OutDir exists: $OutDir; give a new folder (nothing is overwritten)"
}
New-Item -ItemType Directory -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$Workload = (Resolve-Path -LiteralPath $Workload).Path

function Invoke-Bench {
    param([string[]] $Arguments, [string] $LogPath)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Exe @Arguments 2>&1 | ForEach-Object { "$_" } | Tee-Object -FilePath $LogPath | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
}

$adapters = Join-Path $OutDir 'adapters.json'
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$listed = & $Exe --list-adapters 2> (Join-Path $OutDir 'adapters.log')
$listExit = $LASTEXITCODE
$ErrorActionPreference = $previous
if ($listExit -ne 0) {
    throw "ac-vision-ep-bench --list-adapters failed with exit code $listExit (see adapters.log)"
}
[IO.File]::WriteAllText($adapters, ($listed -join "`n"), [Text.UTF8Encoding]::new($false))
Write-Host "DXGI adapters (the DirectML --device is the index):"
foreach ($adapter in ($listed -join "`n" | ConvertFrom-Json)) {
    Write-Host ("  {0}: {1} vendor {2} device {3} software={4}" -f $adapter.index, $adapter.description, $adapter.vendor_id, $adapter.device_id, $adapter.software)
}

$baseline = Join-Path $OutDir 'cpu-1t.json'
$results = @()
foreach ($label in $Variants) {
    $entry = $table[$label]
    $out = Join-Path $OutDir "$label.json"
    $log = Join-Path $OutDir "$label.log"
    $arguments = @('--mode', $Mode, '--vision-root', $entry.Root) + $entry.Extra
    if ($entry.Device -ge 0) {
        $arguments += @('--device', [string]$entry.Device)
    }
    if ($Mode -eq 'template') {
        $arguments += @('--direct-enumeration', $(if ($label -eq 'cpu-1t') { 'on' } else { 'off' }))
    } elseif (-not [string]::IsNullOrWhiteSpace($ModelsDir)) {
        $arguments += @('--models-dir', $ModelsDir)
    }
    $arguments += @('--workload', $Workload, '--warmup', [string]$Warmup, '--reps', [string]$Reps, '--out', $out, '--label', $label)
    $comparedWith = $null
    if ($Mode -eq 'ocr' -and $label -ne 'cpu-1t' -and (Test-Path -LiteralPath $baseline -PathType Leaf)) {
        $baselineStatus = (Get-Content -LiteralPath $baseline -Raw | ConvertFrom-Json).status
        if ($baselineStatus -eq 'ok') {
            $arguments += @('--baseline', $baseline)
            $comparedWith = 'cpu-1t'
        }
    }
    Write-Host "=== $label"
    $started = Get-Date
    $exitCode = Invoke-Bench -Arguments $arguments -LogPath $log
    $status = 'no report'
    $errorCode = $null
    $gate = $null
    if (Test-Path -LiteralPath $out -PathType Leaf) {
        $report = Get-Content -LiteralPath $out -Raw | ConvertFrom-Json
        $status = $report.status
        if ($null -ne $report.error) {
            $errorCode = $report.error.code
        }
        if ($null -ne $report.consistency) {
            $gate = $report.consistency.gate.passed
        }
    }
    $results += [ordered]@{
        label = $label
        exit_code = $exitCode
        status = $status
        error_code = $errorCode
        compared_with = $comparedWith
        consistency_gate_passed = $gate
        report = $out
        log = $log
        started = $started.ToString('o')
        seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
    }
    Write-Host ("=== {0}: exit {1}, status {2}{3}" -f $label, $exitCode, $status, $(if ($errorCode) { ", $errorCode" } else { '' }))
}

$index = [ordered]@{
    schema = 'actingcommand.vision-ep-bench.index/1'
    mode = $Mode
    exe = $Exe
    workload = $Workload
    warmup = $Warmup
    reps = $Reps
    variants = $results
}
[IO.File]::WriteAllText((Join-Path $OutDir 'index.json'), ($index | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
$failed = @($results | Where-Object { $_.exit_code -ne 0 })
if ($failed.Count -ne 0) {
    Write-Error ("variants failed: " + (($failed | ForEach-Object { "$($_.label) (exit $($_.exit_code), $($_.error_code))" }) -join '; ')) -ErrorAction Continue
    exit 1
}
Write-Host "all $($results.Count) variants finished; index $(Join-Path $OutDir 'index.json')"
