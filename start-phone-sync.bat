@echo off
REM ---------------------------------------------------------------------------
REM Phone Sync - production launcher for the Windows box (Service Manager entry).
REM
REM One process serves everything: the iOS app's JSON/upload API *and* the web
REM gallery UI (the gallery is a single page compiled into the binary and served
REM at "/"), so a single Service Manager entry covers both.
REM
REM Photos and videos are filed straight into the real photo library at
REM E:\pictures\<year>\<yyyymm>-phone-sync. Only the metadata index and the
REM thumbnail cache live in PHONE_SYNC_DATA_DIR.
REM
REM The JWT signing secret is NOT in this file: with PHONE_SYNC_JWT_SECRET unset
REM the server generates a random 256-bit secret on first run and persists it to
REM %PHONE_SYNC_DATA_DIR%\jwt-secret, which is outside the repo.
REM ---------------------------------------------------------------------------

setlocal

REM Service Manager passes the configured port in %PORT%; 7071 when run by hand.
if "%PORT%"=="" set PORT=7071

set PHONE_SYNC_BIND=0.0.0.0:%PORT%
set PHONE_SYNC_MEDIA_ROOT=E:\pictures
set PHONE_SYNC_MEDIA_FOLDER_SUFFIX=phone-sync
set PHONE_SYNC_DATA_DIR=E:\phone-sync-data
set PHONE_SYNC_USER=jason
set RUST_LOG=info,tower_http=info

set SERVER_DIR=%~dp0phone-sync-server
set EXE=%SERVER_DIR%\target\release\phone-sync-server.exe

if not exist "%EXE%" (
  echo Release binary missing, building it first...
  call "%SERVER_DIR%\build-windows.cmd" || exit /b 1
)

echo Starting phone-sync-server on %PHONE_SYNC_BIND%
echo   media root : %PHONE_SYNC_MEDIA_ROOT%
echo   data dir   : %PHONE_SYNC_DATA_DIR%
"%EXE%"
