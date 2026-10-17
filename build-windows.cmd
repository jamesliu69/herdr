@echo off
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-windows.ps1"
set "BUILD_EXIT=%ERRORLEVEL%"
echo.
if not "%BUILD_EXIT%"=="0" (
  echo Build failed with exit code %BUILD_EXIT%.
) else (
  echo Build succeeded.
)
echo.
pause
exit /b %BUILD_EXIT%
