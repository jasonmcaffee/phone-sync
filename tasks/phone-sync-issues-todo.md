# Phone Sync — issues TODO

## Backend (Rust)
- [x] Chunked upload: `/media/upload/status/{sha256}`, `/media/upload/chunk`, `/media/upload/complete`
- [x] Storage: write chunk, list received chunks, streaming assemble + verify sha + store, cleanup
- [x] Integration tests for chunked flow (resume, dedup, verify) — 15/15 pass

## iOS app
- [x] ApiClient: uploadStatus / uploadChunk / uploadComplete
- [x] SyncEngine: chunk files > 90MB; only attempt not-synced items
- [x] Circuit breaker: stop batch on transport/server error; exponential backoff retry
- [x] UI: fix "N/N synced" truncation (move out of cramped toolbar)
- [x] UI: green checkmark bottom-right for synced items
- [x] UI: segmented filter tab (All / Unsynced / Synced)

## Verify
- [x] cargo test (backend)
- [x] iOS build + simulator sanity (grid filter, badges), unit tests
- [x] commit + push
