param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$StateDir = Join-Path $env:LOCALAPPDATA "GachaPilot\AliceRuntimeOrchestrator"
$InfoPath = Join-Path $StateDir "runtime_info.json"

if (-not (Test-Path -LiteralPath $InfoPath -PathType Leaf)) {
    Write-Host "Runtime info not found: $InfoPath"
    exit 0
}

$info = Get-Content -Raw -LiteralPath $InfoPath | ConvertFrom-Json
$uri = "http://$($info.host):$($info.httpPort)/orchestrator/shutdown"

try {
    Invoke-RestMethod -Method Post -Uri $uri -TimeoutSec 5 | Out-Null
    Write-Host "AliceRuntimeOrchestrator shutdown requested."
    exit 0
}
catch {
    Write-Warning "Graceful shutdown failed: $($_.Exception.Message)"
    if (-not $Force) {
        Write-Warning "Use -Force to stop PID $($info.pid)."
        exit 1
    }
}

if ($info.pid) {
    Stop-Process -Id ([int]$info.pid) -Force
    Write-Host "AliceRuntimeOrchestrator process stopped: $($info.pid)"
}
