# Phone Sync

Self-hosted photo/video backup: a native iOS app that uploads your camera roll to
a Rust server you own. Photos-app-style grid with per-item sync badges, full-screen
viewer with video playback, manual **Sync now**, sign-in, and opportunistic
background sync. The server also hosts a **web gallery** to browse everything from
a browser.

- **`phone-sync-server/`** — Rust (axum) backend. See its [README](phone-sync-server/README.md).
- **`phone-sync-app/`** — SwiftUI iOS app.
- **`tasks/phone-sync-tdd.md`** — technical design (architecture, trade-offs, testing).

## Quick start (development on macOS)

### 1. Run the backend
```bash
cd phone-sync-server
cargo run          # listens on 0.0.0.0:8080; seeded user "jason" / "modestMouse1!"
```
Find your Mac's LAN IP so the phone can reach it: `ipconfig getifaddr en0`.

### 2. Build & run the app
```bash
cd phone-sync-app
xcodegen generate          # regenerate PhoneSync.xcodeproj from project.yml
open PhoneSync.xcodeproj    # then Run on a simulator or device
```
Sign in as `jason` / `modestMouse1!`. The server URL defaults to a LAN address and
is editable on the sign-in screen and in Settings. In the Simulator you can also use
`http://127.0.0.1:8080` (the simulator shares the Mac's network).

Seed the simulator's photo library for testing:
```bash
xcrun simctl addmedia booted /path/to/photo.jpg /path/to/video.mp4
```

### 3. Browse the web gallery
Open `http://<host>:8080/` in a browser and sign in. You get a month-grouped grid
of every imported photo and video that pages in as you scroll; click any item to
maximize it, and videos stream and seek inline. HEIC is decoded server-side —
tile grid reassembled, orientation applied — because no browser can display it.
See [phone-sync-server/README.md](phone-sync-server/README.md#web-gallery) for how
that works and which ffmpeg build it needs.

## How syncing works (and the iOS reality)

iOS does **not** allow an app to run continuously in the background. Phone Sync uses
the same approach as iCloud/Google Photos:

- **Foreground:** a `PHPhotoLibraryChangeObserver` notices new captures and uploads them.
- **Background:** a registered `BGProcessingTask` is woken opportunistically by the OS
  (typically on Wi-Fi) to upload anything outstanding — no need to open the app.
- **Manual:** the **Sync now** button uploads everything not yet backed up.

Sync state is content-addressed (SHA-256), so uploads are idempotent and retryable;
the client diffs the local library against the server manifest and only uploads what's
missing.

## Tests

```bash
# Backend (6 integration tests: auth, upload, dedup, manifest, fetch)
cd phone-sync-server && cargo test

# iOS (5 unit + 4 UI tests). Requires the backend running on 127.0.0.1:8080.
cd phone-sync-app && xcodebuild test -project PhoneSync.xcodeproj -scheme PhoneSync \
  -sdk iphonesimulator -destination 'platform=iOS Simulator,name=iPhone 16'
```

## Production (Windows 11)

The backend is pure Rust and cross-compiles to Windows. On the Windows box it runs as
the **Phone Sync** Service Manager entry on port **7071**, launched by
[`start-phone-sync.bat`](start-phone-sync.bat), behind the reverse proxy that terminates
TLS for `phone.jasonmcaffee.com`. Point the app's server URL at
`https://phone.jasonmcaffee.com`.

Photos and videos are filed into the real photo library by capture date:

```
E:\pictures\2026\202608-phone-sync\IMG_0093.HEIC
E:\pictures\2025\202512-phone-sync\IMG_0044.HEIC
```

Only the metadata index and the thumbnail/preview caches live in the data dir
(`E:\phone-sync-data`). Build with `phone-sync-server\build-windows.cmd` — it sets up
the MSVC environment that `ring` needs, and run `fetch-ffmpeg.cmd` once to get a
git ffmpeg build (release builds can't open HDR HEICs). See
[phone-sync-server/README.md](phone-sync-server/README.md) for the full configuration,
storage layout and de-duplication rules.
