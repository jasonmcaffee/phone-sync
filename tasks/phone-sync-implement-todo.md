# Phone Sync — Implementation TODO

## Backend (Rust / axum)
- [x] Install Rust toolchain (background)
- [x] Scaffold `phone-sync-server` cargo project + deps
- [x] Config module (env/toml: port, data-dir, jwt secret, seeded user/pass hash)
- [x] Models (login req/resp, upload metadata, manifest, stored record)
- [x] Auth: Argon2 password verify + HS256 JWT sign/verify (1y exp), Bearer middleware
- [x] Storage service: content-addressed FS write (sha256), metadata index, idempotency
- [x] Handlers: /health, /auth/login, /media/manifest, /media/upload (multipart streamed), /media/{id}
- [x] Wire router + body limits + tracing
- [x] Integration tests (login, auth guard, upload, dedup, manifest, photo+video) — 6/6 pass
- [x] cargo test + run locally, smoke test with curl — verified end to end (LAN IP 192.168.0.26)

## iOS app (SwiftUI)
- [x] Generate Xcode project (phone-sync-app) targeting simulator — builds clean
- [x] Info.plist: photo usage, background modes, BGTask ids, ATS dev exception
- [x] Models: Asset, SyncState, AuthToken, ServerConfig, dtos
- [x] KeychainService (token + server url)
- [x] ApiClient (login, manifest, upload multipart)
- [x] AuthService + SignInView + SignInViewModel
- [x] PhotoLibraryService (auth, fetch PHAssets, thumbnails, change observer, export data)
- [x] SyncStateStore (local persistence of per-asset state)
- [x] SyncEngine (diff, queue, upload, state transitions, manual + auto triggers)
- [x] Background scheduler (BGProcessingTask registration/scheduling)
- [x] Views: MediaGridView, MediaCellView (badge), MediaDetailView (photo/video), SettingsView
- [x] Wire app entry, root navigation (signed-in vs sign-in)

## Verification
- [x] Build backend, run locally on LAN IP
- [x] Build iOS app for simulator, boot simulator, seed photos (5 photos + 1 video)
- [x] Sign in, view grid, tap Sync now, confirm badges + files on server (12/12 synced; files on disk)
- [x] iOS tests: 5 unit (SyncDiff) + 4 XCUITest (sign-in ok/bad, manual sync, tap-to-expand) — all pass
- [x] Re-verify against original task instructions + TDD

## Docs
- [x] README with run instructions (backend + app), server URL config, LAN IP tip
