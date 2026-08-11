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

/// A persisted media record kept in the metadata index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRecord {
    pub asset_id: String,
    pub sha256: String,
    pub filename: String,
    pub content_type: String,
    pub media_type: String,
    pub created_at: String,
    /// Relative path under the data dir where bytes are stored.
    pub rel_path: String,
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

/// A single item as presented to the web gallery.
#[derive(Debug, Serialize)]
pub struct MediaListItem {
    /// Content id (sha256) used to fetch bytes/thumbnail.
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub media_type: String,
    pub created_at: String,
    pub size: u64,
    /// True if the server can render an image thumbnail for this item.
    pub thumbnailable: bool,
}

/// The gallery listing response, newest first.
#[derive(Debug, Serialize)]
pub struct MediaListResponse {
    pub items: Vec<MediaListItem>,
    pub count: usize,
}
