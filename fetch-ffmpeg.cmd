@echo off
REM ---------------------------------------------------------------------------
REM Fetches the ffmpeg/ffprobe build Phone Sync prefers into tools\ffmpeg\bin.
REM
REM Why a bundled copy rather than whatever is on PATH: the release builds lag
REM behind on HEIF features Apple already ships. A photo whose primary item is a
REM `tmap` (tone-map) derived image — what an iPhone writes for HDR stills, and
REM increasingly the default — makes ffmpeg 7.1.1 fail to open the file at all
REM ("Derived Image item of type tmap is not implemented"), so no thumbnail and
REM no preview. A git build opens it and exposes the tile grid normally.
REM
REM This is optional. start-phone-sync.bat uses these binaries when present and
REM silently falls back to PATH otherwise; the fallback still handles every other
REM format in the library.
REM ---------------------------------------------------------------------------

setlocal
set TOOLS=%~dp0tools
set ARCHIVE=%TOOLS%\ffmpeg-git-essentials.7z
set SEVENZIP=C:\Program Files\7-Zip\7z.exe

if not exist "%SEVENZIP%" (
  echo Need 7-Zip at "%SEVENZIP%" to unpack the ffmpeg archive.
  exit /b 1
)

if not exist "%TOOLS%" mkdir "%TOOLS%"
echo Downloading ffmpeg git build...
curl -sL -o "%ARCHIVE%" "https://www.gyan.dev/ffmpeg/builds/ffmpeg-git-essentials.7z" || exit /b 1

echo Unpacking...
"%SEVENZIP%" x "%ARCHIVE%" -o"%TOOLS%\unpacked" -y >nul || exit /b 1

if not exist "%TOOLS%\ffmpeg\bin" mkdir "%TOOLS%\ffmpeg\bin"
for /r "%TOOLS%\unpacked" %%F in (ffmpeg.exe ffprobe.exe) do copy /y "%%F" "%TOOLS%\ffmpeg\bin\" >nul

echo Installed:
"%TOOLS%\ffmpeg\bin\ffmpeg.exe" -version 2>nul | findstr /b "ffmpeg version"
exit /b 0
