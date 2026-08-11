@echo off
REM Builds the release binary on Windows.
REM
REM `ring` (pulled in by jsonwebtoken) compiles C, so cargo needs the MSVC
REM toolchain on INCLUDE/LIB. Without vcvars64 the build dies with
REM "fatal error C1083: Cannot open include file: 'stddef.h'".

setlocal
set VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat
if not exist "%VCVARS%" (
  echo Could not find vcvars64.bat at "%VCVARS%".
  echo Install the VS 2022 C++ build tools or edit this script.
  exit /b 1
)
call "%VCVARS%" >nul
cd /d "%~dp0"
cargo build --release %*
exit /b %ERRORLEVEL%
