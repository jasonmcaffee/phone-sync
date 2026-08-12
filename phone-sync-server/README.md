# Phone Sync — Backend (Rust)

Self-hosted media backup server for the Phone Sync iOS app. Accepts authenticated
photo/video uploads and stores them on the local filesystem, content-addressed by
SHA-256 (so identical bytes are stored once). Pure Rust — the same code runs on
macOS (dev) and Windows 11 (prod).

## Run (development, macOS)

```bash
cargo run
# listens on 0.0.0.0:8080 with dev defaults (user "jason" / "modestMouse1!")
```

Point the iOS app at `http://<your-mac-LAN-ip>:8080`. Find the IP with:

```bash
ipconfig getifaddr en0
```

## Configuration (environment variables)

| Var | Default | Purpose |
|---|---|---|
| `PHONE_SYNC_BIND` | `0.0.0.0:8080` | Bind address |
| `PHONE_SYNC_DATA_DIR` | `./data` | Metadata index, thumbnail cache, generated JWT secret |
| `PHONE_SYNC_MEDIA_ROOT` | `<data dir>/media` | Root of the date-organized photo/video tree |
| `PHONE_SYNC_MEDIA_FOLDER_SUFFIX` | `phone-sync` | Suffix on each month folder (`202608-phone-sync`) |
| `PHONE_SYNC_USER` | `jason` | Seeded username |
| `PHONE_SYNC_PASSWORD_HASH` | (hash of `modestMouse1!`) | Argon2 PHC hash; **set in prod** |
| `PHONE_SYNC_JWT_SECRET` | generated + persisted | HMAC secret; see below |
| `PHONE_SYNC_TOKEN_TTL_SECS` | `31536000` (1 year) | Token lifetime |
| `PHONE_SYNC_MAX_UPLOAD_BYTES` | `2147483648` (2 GB) | Per-file upload cap |
| `PHONE_SYNC_FFMPEG` | `ffmpeg` | Path to the ffmpeg binary (thumbnails for HEIC/video) |

To generate a production password hash, run the server once and copy the login flow,
or add a small helper; the dev default is fine for local use.

**JWT secret.** With `PHONE_SYNC_JWT_SECRET` unset the server generates a random
256-bit secret on first run and persists it to `<data dir>/jwt-secret`, so tokens
survive restarts without a known constant being compiled into an internet-facing
binary. Set the variable explicitly if you'd rather manage it yourself.

## Where files are stored

Media is filed by **capture date**, using the original filename, so the backup is
a normal photo library you can browse in Explorer rather than a hash tree:

```
<media root>/2026/202608-phone-sync/IMG_0093.HEIC
<media root>/2026/202608-phone-sync/IMG_0102.MOV
<media root>/2025/202512-phone-sync/IMG_0044.HEIC
<data dir>/index/manifest.json      # asset_id -> record metadata
<data dir>/thumbs/<sha256>.jpg      # generated thumbnail cache
<data dir>/jwt-secret               # generated signing secret
```

Details worth knowing:

- The client sends the capture time as RFC-3339 in **UTC**; it is converted to the
  server's local time before choosing the month, so a shot taken at 8pm on August
  31st files under August rather than slipping into September.
- An unparseable or missing capture time falls back to today's date.
- Content is still de-duplicated by sha256 — re-uploading bytes that are already
  stored reuses the existing file instead of writing a second copy.
- Two *different* photos that share a filename in the same month both survive: the
  second is written as `IMG_0001-<8 hex chars>.jpg`. Nothing is ever overwritten.
- Records written by an older build (the `media/<ab>/<sha>.<ext>` layout inside the
  data dir) keep resolving; the index records which root each item lives under.

## Web gallery

Open the server root in a browser to view everything that's been backed up:

```
http://<host>:8080/
```

Sign in with the same credentials; the page shows a grid of all imported photos
and videos, click any item to **maximize** it, and videos **play** inline (with
HTTP range/streaming support for seeking). Image thumbnails are generated and
cached server-side.

**Thumbnails (HEIC & video).** The pure-Rust image decoder handles JPEG/PNG/etc.
For HEIC stills and video frames — which it can't decode — the server shells out
to **ffmpeg**: it decodes one full frame (seeking ~1s into videos) and downscales
it, caching the result under `thumbs/<sha>.jpg`. So the web gallery shows real
previews (and video posters) for everything.

Requirements: **ffmpeg must be installed** on the server host, and for HEIC it
must be a build with **HEIF/libheif support** (e.g. the gyan.dev "full" build on
Windows). Point `PHONE_SYNC_FFMPEG` at it if it isn't on `PATH`. On startup a
background task pre-generates any missing thumbnails across the whole library, so
years of existing photos/videos get previews without opening each one.

Note that HEVC videos may still not *play* in Chrome/Firefox (a browser codec
limitation); they play in Safari and on the device.

## API

| Method | Path | Auth | Description |
|---|---|---|---|
| GET | `/` | none (page) | Web gallery single-page app |
| GET | `/health` | none | Liveness probe |
| POST | `/auth/login` | none | `{username,password}` → `{token, expires_at}` (1-year JWT) |
| GET | `/media/manifest` | token | `{asset_ids, count}` already stored |
| POST | `/media/upload` | token | multipart `metadata`(JSON)+`file`(bytes) → `{id, sha256, stored, duplicate}` (for files ≤90 MB) |
| GET | `/media/upload/status/{sha256}` | token | `{stored, received[]}` — which chunks the server already has (resume) |
| POST | `/media/upload/chunk` | token | multipart `metadata`{sha256,chunk_index}+`file`(chunk) → `{received, ok}` |
| POST | `/media/upload/complete` | token | JSON {…, sha256, total_chunks} → assembles+verifies chunks → `{id, sha256, stored, duplicate}` |
| GET | `/api/media` | token | `{items[], count}` — full listing for the gallery, newest first |
| GET | `/media/{id}` | token | Stream stored bytes; supports `Range` requests (video seeking) |
| GET | `/media/{id}/thumb` | token | Cached JPEG thumbnail (image formats only) |

**Auth:** protected routes accept the token as an `Authorization: Bearer <jwt>`
header (iOS app, JSON calls) **or** a `?token=<jwt>` query parameter (so browser
`<img>`/`<video>` tags in the gallery can authenticate).

## Tests

```bash
cargo test   # 6 integration tests covering auth, upload, dedup, manifest, fetch
```

## Production (Windows 11)

Build the release binary with `build-windows.cmd` — plain `cargo build --release`
fails because `ring` (via `jsonwebtoken`) compiles C and needs the MSVC toolchain
on `INCLUDE`/`LIB`, which the script sets up by calling `vcvars64.bat`:

```powershell
.\build-windows.cmd
```

Then run it behind the reverse proxy that terminates TLS for
`phone.jasonmcaffee.com`. On this box that is `..\start-phone-sync.bat`, launched
by the Service Manager as the **Phone Sync** service on port **7071**:

| | |
|---|---|
| Bind | `0.0.0.0:7071` |
| Media root | `E:\pictures` (→ `E:\pictures\2026\202608-phone-sync\…`) |
| Data dir | `E:\phone-sync-data` |
| Public URL | `https://phone.jasonmcaffee.com` (Cloudflare → proxy on :80 → :7071) |

One process serves the app API *and* the web gallery, so it is a single Service
Manager entry. It is registered with `startOnBoot` in both the **Balanced** and
**2 Comfy** profiles.

Large videos work around this with the chunked upload flow: the client splits
anything over 90 MB into ≤90 MB chunks (`/media/upload/chunk`), then asks the
server to assemble and verify them (`/media/upload/complete`). Chunks stage under
`<data dir>/chunks/<sha256>/` and are streamed into the final file one at a time,
so a multi-GB video is never held whole in memory; `/media/upload/status` lets an
interrupted upload resume without re-sending chunks already received.

Note that Cloudflare's proxied edge caps a single request body at 100 MB on the
free plan, so a very large video will fail there before it reaches this server.
