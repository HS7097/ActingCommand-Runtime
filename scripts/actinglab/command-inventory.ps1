param(
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$cargoArgs = @(
    'run',
    '-q',
    '-p', 'actingcommand-actinglab-architecture',
    '--bin', 'actinglab-command-inventory'
)
if ($Check) {
    $cargoArgs += @('--', '--check')
}

Push-Location $repoRoot
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "actinglab command inventory failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}
