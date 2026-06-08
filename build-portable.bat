@echo off
setlocal
echo === [1/2] Sourcing vcvars64 ===
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if errorlevel 1 (echo vcvars64 FAILED & exit /b 1)
echo === [2/2] Tauri build (no-bundle) ===
call npm run tauri -- build --no-bundle
if errorlevel 1 (echo tauri build FAILED & exit /b 1)
echo.
echo === DONE ===
dir src-tauri\target\release\cc-traffic-light.exe
