@echo off
setlocal
set "SCRIPT_DIR=%~dp0"
set "REPO_ROOT=%SCRIPT_DIR%..\.."

if exist "%REPO_ROOT%\target\release\actinglab.exe" (
  "%REPO_ROOT%\target\release\actinglab.exe" %*
  exit /b %ERRORLEVEL%
)

if exist "%REPO_ROOT%\target\debug\actinglab.exe" (
  "%REPO_ROOT%\target\debug\actinglab.exe" %*
  exit /b %ERRORLEVEL%
)

cargo run -q -p actingcommand-actinglab --manifest-path "%REPO_ROOT%\Cargo.toml" -- %*
exit /b %ERRORLEVEL%
