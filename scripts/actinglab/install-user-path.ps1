$ErrorActionPreference = "Stop"

$launcherDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$resolvedLauncherDir = (Resolve-Path $launcherDir).Path.TrimEnd("\")
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$parts = @()
if (-not [string]::IsNullOrWhiteSpace($userPath)) {
    $parts = $userPath.Split(";") | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
}

$exists = $false
foreach ($part in $parts) {
    if ($part.TrimEnd("\").Equals($resolvedLauncherDir, [StringComparison]::OrdinalIgnoreCase)) {
        $exists = $true
        break
    }
}

if (-not $exists) {
    $parts += $resolvedLauncherDir
    [Environment]::SetEnvironmentVariable("Path", ($parts -join ";"), "User")
}

Write-Output "actinglab launcher directory is installed in the user PATH: $resolvedLauncherDir"
Write-Output "Open a new terminal for PATH changes to take effect."
