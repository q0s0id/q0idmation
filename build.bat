@echo off
setlocal

rem q0editor one-click build wrapper.
rem modes: debug (default), release, check, verify, run

set "SCRIPT_DIR=%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT_DIR%build.ps1" %*
set "EXIT_CODE=%ERRORLEVEL%"

echo.
if "%EXIT_CODE%"=="0" (
    echo [ok] vse norm.
) else (
    echo [fail] chto-to ne tak. see out\last-build.txt
)
echo.
echo examples:
echo   build.bat
echo   build.bat release
echo   build.bat verify
echo   build.bat run
echo.
pause
exit /b %EXIT_CODE%
