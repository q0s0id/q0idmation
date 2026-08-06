@echo off
rem Wrapper around build.ps1. Double-click this file to build both installers.
rem PowerShell does the actual work and reports any missing build tools.

set "SCRIPT_DIR=%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%build.ps1" %*
set "EXIT_CODE=%ERRORLEVEL%"

echo.
if "%EXIT_CODE%"=="0" (
    echo Build succeeded. Installers are in installer\dist\.
) else (
    echo Build failed with exit code %EXIT_CODE%.
)
echo.
pause
exit /b %EXIT_CODE%
