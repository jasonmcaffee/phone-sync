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
| `PHONE_SYNC_DATA_DIR` | `./data` | Where media + index are stored |
| `PHONE_SYNC_USER` | `jason` | Seeded username |
| `PHONE_SYNC_PASSWORD_HASH` | (hash of `modestMouse1!`) | Argon2 PHC hash; **set in prod** |
| `PHONE_SYNC_JWT_SECRET` | `dev-insecure-change-me` | HMAC secret; **set in prod** |
| `PHONE_SYNC_TOKEN_TTL_SECS` | `31536000` (1 year) | Token lifetime |
| `PHONE_SYNC_MAX_UPLOAD_BYTES` | `2147483648` (2 GB) | Per-file upload cap |

To generate a production password hash, run the server once and copy the login flow,
or add a small helper; the dev default is fine for local use.

## Web gallery

Open the server root in a browser to view everything that's been backed up:

```
http://<host>:8080/
```

Sign in with the same credentials; the page shows a grid of all imported photos
and videos, click any item to **maximize** it, and videos **play** inline (with
HTTP range/streaming support for seeking). Image thumbnails are generated and
cached server-side.

**HEIC note:** iPhone originals are often HEIC/HEVC, which neither the pure-Rust
image decoder nor most browsers (Chrome/Firefox) can display. Those items show a
labeled fallback tile with a download link and render natively in Safari. Adding
server-side HEIC→JPEG transcoding (via libheif) is a future enhancement.

## API

| Method | Path | Auth | Description |
|---|---|---|---|
| GET | `/` | none (page) | Web gallery single-page app |
| GET | `/health` | none | Liveness probe |
| POST | `/auth/login` | none | `{username,password}` → `{token, expires_at}` (1-year JWT) |
| GET | `/media/manifest` | token | `{asset_ids, count}` already stored |
| POST | `/media/upload` | token | multipart `metadata`(JSON)+`file`(bytes) → `{id, sha256, stored, duplicate}` |
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

Build a release binary and run it behind a reverse proxy terminating TLS for
`phone.jasonmcaffee.com`:

```powershell
cargo build --release
$env:PHONE_SYNC_JWT_SECRET="<long-random-secret>"
$env:PHONE_SYNC_DATA_DIR="D:\phone-sync-data"
.\target\release\phone-sync-server.exe
```

Data layout under the data dir:

```
media/<ab>/<sha256>.<ext>   # content-addressed bytes
index/manifest.json         # asset_id -> record metadata
```
