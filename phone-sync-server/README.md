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
| `PHONE_SYNC_FFMPEG` | `ffmpeg` | Path to the ffmpeg binary (renders HEIC/video) |
| `PHONE_SYNC_FFPROBE` | `ffprobe` | Path to ffprobe — reports a HEIC's tile-grid layout |
| `PHONE_SYNC_THUMB_MAX_DIM` | `512` | Longest edge of a grid thumbnail |
| `PHONE_SYNC_PREVIEW_MAX_DIM` | `2048` | Longest edge of a full-screen preview |
| `PHONE_SYNC_PAGE_SIZE` | `120` | Default `/api/media` page size |
| `PHONE_SYNC_MAX_PAGE_SIZE` | `500` | Cap on a caller-requested page size |
| `PHONE_SYNC_THUMB_WORKERS` | half the cores (1-8) | Concurrency of the startup thumbnail backfill |

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
<data dir>/thumbs/<sha256>.jpg      # generated grid-thumbnail cache
<data dir>/previews/<sha256>.jpg    # generated full-screen preview cache
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

Sign in with the same credentials. The grid is grouped by month, pages in as you
scroll (120 items at a time — the library here is 2,700 items and 71 GB, so
sending it all at once is not an option), and every tile has a real thumbnail.
Click any item to maximize it; videos play inline and seek.

### Decoding an iPhone photo

An iPhone HEIC is not one image. A 4032x3024 shot is stored as a **tile grid** of
48 separate 512x512 HEVC streams, alongside a grayscale HDR **gain map** and a
10-bit HDR rendition — and the orientation lives in an `irot` item property, not
in EXIF and not in any stream. Handing the file to `ffmpeg -i photo.heic out.jpg`
gets you the *gain map*: a black-and-white ghost of the photo, lying on its side.

So the server does three things instead (`imaging.rs`, `heif.rs`,
`orientation.rs`):

1. asks `ffprobe -show_stream_groups` for the primary group — the one with
   `disposition=default` — and the exact pixel offset of each of its tiles;
2. reassembles them with `xstack`, then crops the padded canvas back to the
   visible size (the tiles cover 4096x3072 for a 4032x3024 photo);
3. reads `irot`/`imir` straight out of the ISOBMFF box tree and applies the
   matching rotation, since stitching bypasses ffmpeg's auto-rotate.

JPEG EXIF orientation (tag 0x0112) is read the same way for the same reason.
Videos need none of this — ffmpeg honours their display matrix — so they just get
a frame grabbed ~1s in.

Results are cached as `thumbs/<sha>.jpg` (512 px) and `previews/<sha>.jpg`
(2048 px). On startup a background pool generates any that are missing; the whole
library of 2,547 took about five minutes on eight workers, and a restart after
that costs nothing.

### ffmpeg requirements

**ffmpeg and ffprobe must be installed**, and a *recent* build is worth having.
Release 7.1.1 cannot open a HEIC whose primary item is a `tmap` (tone-map)
derived image — what an iPhone writes for HDR stills, and increasingly the
default — failing with *"Derived Image item of type tmap is not implemented"*, so
such a photo gets no thumbnail and no preview at all. A git build opens it fine.

`..etch-ffmpeg.cmd` downloads a git build into `tools/ffmpeg/bin`, and
`start-phone-sync.bat` prefers it when present, falling back to `PATH` otherwise.
Point `PHONE_SYNC_FFMPEG` / `PHONE_SYNC_FFPROBE` somewhere else if you'd rather.

### Serving video

Media is **streamed** from disk in bounded chunks, honouring `Range` (including
the open-ended `bytes=N-` and suffix `bytes=-N` forms). Nothing is buffered whole,
which is what makes a 4 GB clip playable at all: the two-byte probe a browser
opens with returns in ~1 ms, as does a seek 3.6 GB into the file.

`.MOV` is served as `video/mp4`. The container is ISO-BMFF either way — the bytes
are untouched — but `canPlayType("video/quicktime")` reports nothing, so any
`<source type=…>` check would rule the file out before trying it. (Chromium does
decode it under either type when set as a `<video src>`, because it sniffs the
container; the label is for the declarative paths.)

Whether an **HEVC** clip plays is still down to the viewer's codecs: Safari and
iOS always, Chrome/Edge where the OS supplies an HEVC decoder, and the gallery
says so plainly with a download link when a player gives up. 274 of the 328 clips
in this library are HEVC.

### Caching

`/media/{id}`, `/thumb` and `/preview` are all addressed by content hash, so they
are served `Cache-Control: private, max-age=31536000, immutable` with an `ETag`,
and answer a conditional re-request with `304`. Each rendition carries its own tag
so a cache can never serve a thumbnail in place of the original.

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
| GET | `/api/media?offset=&limit=` | token | `{items[], count, offset, limit}` — one page, newest first |
| GET | `/media/{id}` | token | Stream stored bytes; supports `Range` (video seeking) |
| GET | `/media/{id}/thumb` | token | Cached 512 px JPEG thumbnail (any format) |
| POST | `/media/{id}/thumb` | token | Store a client-generated JPEG thumbnail |
| GET | `/media/{id}/preview` | token | Cached 2048 px JPEG — how HEIC is shown full-screen |

**Auth:** protected routes accept the token as an `Authorization: Bearer <jwt>`
header (iOS app, JSON calls) **or** a `?token=<jwt>` query parameter (so browser
`<img>`/`<video>` tags in the gallery can authenticate).

## Tests

```bash
cargo test   # 21 unit + 23 integration tests: auth, upload, dedup, chunking,
             # range/suffix/unsatisfiable ranges, ETag conditionals, paging,
             # HEIC tile-grid and orientation handling
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
