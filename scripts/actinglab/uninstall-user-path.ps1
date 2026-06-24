$ErrorActionPreference = "Stop"

$launcherDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$resolvedLauncherDir = (Resolve-Path $launcherDir).Path.TrimEnd("\")
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ([string]::IsNullOrWhiteSpace($userPath)) {
    Write-Output "User PATH is empty; nothing to remove."
    exit 0
}

$parts = $userPath.Split(";") | Where-Object {
    -not [string]::IsNullOrWhiteSpace($_) -and
    -not $_.TrimEnd("\").Equals($resolvedLauncherDir, [StringComparison]::OrdinalIgnoreCase)
}
[Environment]::SetEnvironmentVariable("Path", ($parts -join ";"), "User")

Write-Output "actinglab launcher directory removed from the user PATH: $resolvedLauncherDir"
