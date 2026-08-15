//! Serializable request/response and persistence types shared across handlers.

use serde::{Deserialize, Serialize};

/// Sign-in request body.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Sign-in response containing the long-lived JWT.
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    /// Unix timestamp (seconds) when the token expires.
    pub expires_at: i64,
}

/// JWT claims. `sub` is the username, `exp` the expiry (unix seconds).
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
}

/// Client-supplied metadata accompanying an upload (the JSON multipart part).
#[derive(Debug, Deserialize)]
pub struct UploadMetadata {
    /// Stable PhotoKit localIdentifier for the source asset.
    pub asset_id: String,
    pub filename: String,
    pub content_type: String,
    /// ISO-8601 capture time from the device.
    pub created_at: String,
    /// "photo" or "video".
    pub media_type: String,
    /// Client-computed sha256 (hex) of the file bytes, for integrity checking.
    pub sha256: String,
}

/// Response to a successful upload.
#[derive(Debug, Serialize)]
pub struct UploadResponse {
    /// Server record id (the sha256, which is the content address).
    pub id: String,
    pub sha256: String,
    /// True once the bytes are persisted.
    pub stored: bool,
    /// True if the content already existed (idempotent no-op write).
    pub duplicate: bool,
}

/// Which configured root a record's `rel_path` is relative to.
///
/// Records written before the date-organized layout live under
/// `<data_dir>/media/<ab>/<sha>.<ext>`; everything written since lives under the
/// configured media root (e.g. `E:\pictures`). Serde defaults to the legacy
/// variant so an index written by an older build keeps resolving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageRoot {
    /// Legacy content-addressed layout inside the data dir.
    #[default]
    DataDir,
    /// Date-organized layout under the configured media root.
    MediaRoot,
}

/// A persisted media record kept in the metadata index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRecord {
    pub asset_id: String,
    pub sha256: String,
    pub filename: String,
    pub content_type: String,
    pub media_type: String,
    pub created_at: String,
    /// Path to the bytes, relative to the root named by `storage_root`,
    /// e.g. `2026/202608-phone-sync/IMG_0001.HEIC`.
    pub rel_path: String,
    /// Which configured root `rel_path` is relative to.
    #[serde(default)]
    pub storage_root: StorageRoot,
    pub size: u64,
    /// Server-side ingest time (unix seconds).
    pub ingested_at: i64,
}

/// Response listing which asset ids the server already has, so the client
/// can compute the not-yet-synced set.
#[derive(Debug, Serialize)]
pub struct ManifestResponse {
    pub asset_ids: Vec<String>,
    pub count: usize,
}

/// A single item as presented to the web gallery / iOS Synced view.
#[derive(Debug, Serialize)]
pub struct MediaListItem {
    /// Content id (sha256) used to fetch bytes/thumbnail.
    pub id: String,
    /// The client-side asset identifier, so the app can map a server item back
    /// to a local asset (e.g. to generate a preview while it is still on-device).
    pub asset_id: String,
    pub filename: String,
    pub content_type: String,
    pub media_type: String,
    pub created_at: String,
    pub size: u64,
    /// Where the bytes live on disk, relative to the media root — surfaced so
    /// the gallery can show which month folder an item was filed into.
    pub rel_path: String,
    /// True if the server can render an image thumbnail for this item.
    pub thumbnailable: bool,
    /// True if a browser can display the *original* bytes directly. False for
    /// HEIC, which is most of this library — those must go through `/preview`.
    pub browser_displayable: bool,
    /// The MIME type `/media/:id` will serve these bytes as, which differs from
    /// the uploaded type for iPhone video (see `storage::served_content_type`).
    pub served_content_type: String,
}

/// One page of the gallery listing, newest first.
#[derive(Debug, Serialize)]
pub struct MediaListResponse {
    pub items: Vec<MediaListItem>,
    /// Total number of items in the library, not just this page.
    pub count: usize,
    /// Offset this page started at.
    pub offset: usize,
    /// Maximum number of items this page could contain.
    pub limit: usize,
}

/// Paging parameters for the gallery listing. Both are optional so an old client
/// (or a hand-typed URL) still gets a sensible first page.
#[derive(Debug, Deserialize)]
pub struct PageQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

// MARK: - Chunked upload (large videos exceeding Cloudflare's 100 MB body cap)

/// Metadata part of a chunk upload: which file (by full-content sha256) and
/// which chunk index this payload carries.
#[derive(Debug, Deserialize)]
pub struct ChunkMetadata {
    pub sha256: String,
    pub chunk_index: u32,
}

/// Acknowledgement that a single chunk was persisted.
#[derive(Debug, Serialize)]
pub struct ChunkAck {
    pub received: u32,
    pub ok: bool,
}

/// Status of a chunked upload: whether the full content is already stored, and
/// which chunk indices the server currently holds (so the client can resume
/// without re-sending chunks it already delivered).
#[derive(Debug, Serialize)]
pub struct ChunkStatusResponse {
    pub stored: bool,
    pub received: Vec<u32>,
}

/// Finalize request: assemble the previously-uploaded chunks into the file.
#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub asset_id: String,
    pub filename: String,
    pub content_type: String,
    pub created_at: String,
    pub media_type: String,
    pub sha256: String,
    pub total_chunks: u32,
}
