@echo off
REM Type-checks the crate (lib, bin and tests) with the MSVC toolchain on PATH.
REM `ring` compiles C, so without vcvars64 the build dies in stddef.h — the same
REM reason build-windows.cmd exists. Unlike that script this never touches the
REM release binary, so it can be run while the service is up.
setlocal
set VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat
if not exist "%VCVARS%" (
  echo Could not find vcvars64.bat at "%VCVARS%".
  exit /b 1
)
call "%VCVARS%" >nul
cd /d "%~dp0"
cargo check --all-targets %*
exit /b %ERRORLEVEL%
