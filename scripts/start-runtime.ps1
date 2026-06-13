param(
    [string]$Python = "python",
    [string]$HostName = "127.0.0.1",
    [int]$HttpPort = 8765,
    [int]$WsPort = 8766,
    [switch]$Foreground
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$Runtime = Join-Path $Root "runtime\alice_runtime_orchestrator.py"

if (-not (Test-Path -LiteralPath $Runtime -PathType Leaf)) {
    throw "Runtime entry not found: $Runtime"
}

$PythonCommand = Get-Command $Python -ErrorAction Stop
if ($PythonCommand.Source -like "*\Microsoft\WindowsApps\python.exe" -or $PythonCommand.Source -like "*\Microsoft\WindowsApps\python3.exe") {
    throw "A real Python interpreter was not found. Install Python and rerun this script, or pass -Python with an explicit python.exe path."
}

& $Python -c "import websockets" 2>$null
if ($LASTEXITCODE -ne 0) {
    throw "Missing runtime dependency 'websockets'. Install it with: $Python -m pip install -r runtime\requirements.txt"
}

$HealthUri = "http://$HostName`:$HttpPort/health"

function Get-OrchestratorHealth {
    param([string]$Uri)

    try {
        return Invoke-RestMethod -Uri $Uri -TimeoutSec 1
    } catch {
        return $null
    }
}

$ExistingHealth = Get-OrchestratorHealth -Uri $HealthUri
if ($null -ne $ExistingHealth) {
    if ($ExistingHealth.service -eq "AliceRuntimeOrchestrator" -and $ExistingHealth.ok -eq $true) {
        Write-Host "AliceRuntimeOrchestrator is already running."
        Write-Host "PID: $($ExistingHealth.pid)"
        Write-Host "HTTP: $HealthUri"
        Write-Host "WebSocket: ws://$HostName`:$WsPort/events"
        exit 0
    }

    throw "Port $HttpPort responded, but it is not AliceRuntimeOrchestrator. Refusing to start a conflicting runtime."
}

$argsList = @(
    $Runtime,
    "--host", $HostName,
    "--http-port", [string]$HttpPort,
    "--ws-port", [string]$WsPort
)

if ($Foreground) {
    & $Python @argsList
    exit $LASTEXITCODE
}

$process = Start-Process `
    -FilePath $Python `
    -ArgumentList $argsList `
    -WorkingDirectory $Root `
    -WindowStyle Hidden `
    -PassThru

$Deadline = (Get-Date).AddSeconds(10)
while ((Get-Date) -lt $Deadline) {
    if ($process.HasExited) {
        throw "AliceRuntimeOrchestrator exited during startup with code $($process.ExitCode). Check %LOCALAPPDATA%\GachaPilot\AliceRuntimeOrchestrator\logs\orchestrator.log for details."
    }

    $StartedHealth = Get-OrchestratorHealth -Uri $HealthUri
    if ($null -ne $StartedHealth -and $StartedHealth.service -eq "AliceRuntimeOrchestrator" -and $StartedHealth.ok -eq $true) {
        Write-Host "AliceRuntimeOrchestrator started."
        Write-Host "PID: $($StartedHealth.pid)"
        Write-Host "HTTP: $HealthUri"
        Write-Host "WebSocket: ws://$HostName`:$WsPort/events"
        exit 0
    }

    Start-Sleep -Milliseconds 250
}

if (-not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
}

throw "AliceRuntimeOrchestrator did not become healthy within 10 seconds. Startup was aborted."
