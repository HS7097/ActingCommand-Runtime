param()

$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Push-Location $repo
try {
    $env:ACTINGLAB_RECORD_GOLDENS = '1'
    cargo test -p actingcommand-actinglab --test golden_protocol protocol_goldens_match_current_cli -- --exact --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "golden recorder failed with exit code $LASTEXITCODE"
    }
} finally {
    Remove-Item Env:ACTINGLAB_RECORD_GOLDENS -ErrorAction SilentlyContinue
    Pop-Location
}
