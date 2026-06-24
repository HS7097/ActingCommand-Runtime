$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..\..")
$releaseExe = Join-Path $repoRoot "target\release\actinglab.exe"
$debugExe = Join-Path $repoRoot "target\debug\actinglab.exe"

if (Test-Path -LiteralPath $releaseExe) {
    & $releaseExe @args
    exit $LASTEXITCODE
}

if (Test-Path -LiteralPath $debugExe) {
    & $debugExe @args
    exit $LASTEXITCODE
}

& cargo run -q -p actingcommand-actinglab --manifest-path (Join-Path $repoRoot "Cargo.toml") -- @args
exit $LASTEXITCODE
