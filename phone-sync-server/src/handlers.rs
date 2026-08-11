//! HTTP request handlers for auth, manifest listing, and media upload/fetch.

use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;

use crate::error::ApiError;
use crate::models::{
    ChunkAck, ChunkMetadata, ChunkStatusResponse, CompleteRequest, LoginRequest, LoginResponse,
    ManifestResponse, MediaListItem, MediaListResponse, UploadMetadata, UploadResponse,
};
use crate::state::AppState;
use crate::storage::is_thumbnailable;
use crate::auth;
use sha2::Digest;

/// Liveness probe. Always 200 when the server is up.
pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Verifies credentials against the seeded user and issues a long-lived JWT.
pub async fn login(State(state): State<AppState>, Json(req): Json<LoginRequest>) -> Result<Json<LoginResponse>, ApiError> {
    let creds_ok = req.username == state.config.username
        && auth::verify_password(&req.password, &state.config.password_hash);
    if !creds_ok {
        return Err(ApiError::Unauthorized("invalid username or password".into()));
    }
    let (token, expires_at) = auth::issue_token(
        &state.config.username,
        &state.config.jwt_secret,
        state.config.token_ttl_secs,
    );
    Ok(Json(LoginResponse { token, expires_at }))
}

/// Returns the asset ids the server already has so the client can diff and
/// upload only what is missing.
pub async fn manifest(State(state): State<AppState>) -> Result<Json<ManifestResponse>, ApiError> {
    let asset_ids = state.storage.known_asset_ids();
    let count = asset_ids.len();
    Ok(Json(ManifestResponse { asset_ids, count }))
}

/// Accepts a multipart upload (`metadata` JSON part + `file` binary part),
/// stores it idempotently, and returns the resulting record summary.
pub async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> Result<(StatusCode, Json<UploadResponse>), ApiError> {
    let mut metadata: Option<UploadMetadata> = None;
    let mut file_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("metadata") => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("reading metadata: {e}")))?;
                let parsed: UploadMetadata = serde_json::from_str(&text)
                    .map_err(|e| ApiError::BadRequest(format!("invalid metadata json: {e}")))?;
                metadata = Some(parsed);
            }
            Some("file") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("reading file bytes: {e}")))?;
                if bytes.len() > state.config.max_upload_bytes {
                    return Err(ApiError::BadRequest("file exceeds max upload size".into()));
                }
                file_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    let meta = metadata.ok_or_else(|| ApiError::BadRequest("missing metadata part".into()))?;
    let bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("missing file part".into()))?;

    // Integrity check: the client-declared hash must match the received bytes.
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if !meta.sha256.is_empty() && meta.sha256 != actual {
        return Err(ApiError::BadRequest("sha256 mismatch between metadata and file".into()));
    }

    let (record, duplicate) = state
        .storage
        .store(&meta.asset_id, &meta.filename, &meta.content_type, &meta.media_type, &meta.created_at, &bytes)
        .map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(UploadResponse {
            id: record.sha256.clone(),
            sha256: record.sha256,
            stored: true,
            duplicate,
        }),
    ))
}

/// Reports which chunks the server already holds for a content hash, and whether
/// the full content is already stored — lets the client resume/skip work.
pub async fn upload_status(State(state): State<AppState>, Path(sha256): Path<String>) -> Json<ChunkStatusResponse> {
    let stored = state.storage.is_content_stored(&sha256);
    let received = if stored { Vec::new() } else { state.storage.received_chunk_indices(&sha256) };
    Json(ChunkStatusResponse { stored, received })
}

/// Accepts one chunk of a large upload (multipart: `metadata` {sha256,
/// chunk_index} + `file`). Chunks are idempotent — re-sending one overwrites it.
pub async fn upload_chunk(State(state): State<AppState>, mut multipart: Multipart) -> Result<Json<ChunkAck>, ApiError> {
    let mut meta: Option<ChunkMetadata> = None;
    let mut bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("malformed multipart: {e}")))?
    {
        match field.name() {
            Some("metadata") => {
                let text = field.text().await.map_err(|e| ApiError::BadRequest(format!("reading metadata: {e}")))?;
                meta = Some(serde_json::from_str(&text).map_err(|e| ApiError::BadRequest(format!("invalid metadata json: {e}")))?);
            }
            Some("file") => {
                let b = field.bytes().await.map_err(|e| ApiError::BadRequest(format!("reading chunk bytes: {e}")))?;
                if b.len() > state.config.max_upload_bytes {
                    return Err(ApiError::BadRequest("chunk exceeds max upload size".into()));
                }
                bytes = Some(b.to_vec());
            }
            _ => {}
        }
    }

    let meta = meta.ok_or_else(|| ApiError::BadRequest("missing metadata part".into()))?;
    let bytes = bytes.ok_or_else(|| ApiError::BadRequest("missing file part".into()))?;
    state.storage.write_chunk(&meta.sha256, meta.chunk_index, &bytes).map_err(ApiError::from)?;
    Ok(Json(ChunkAck { received: meta.chunk_index, ok: true }))
}

/// Finalizes a chunked upload: assembles and verifies the staged chunks into the
/// stored file, filed by capture date like any other upload.
pub async fn upload_complete(State(state): State<AppState>, Json(req): Json<CompleteRequest>) -> Result<(StatusCode, Json<UploadResponse>), ApiError> {
    let (record, duplicate) = state
        .storage
        .assemble_and_store(&req.asset_id, &req.filename, &req.content_type, &req.media_type, &req.created_at, &req.sha256, req.total_chunks)
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(UploadResponse { id: record.sha256.clone(), sha256: record.sha256, stored: true, duplicate }),
    ))
}

/// Lists all stored media (newest first) for the web gallery.
pub async fn list_media(State(state): State<AppState>) -> Result<Json<MediaListResponse>, ApiError> {
    let items: Vec<MediaListItem> = state
        .storage
        .all_records()
        .into_iter()
        .map(|r| MediaListItem {
            id: r.sha256,
            filename: r.filename,
            content_type: r.content_type.clone(),
            media_type: r.media_type,
            created_at: r.created_at,
            size: r.size,
            rel_path: r.rel_path,
            thumbnailable: is_thumbnailable(&r.content_type, ""),
        })
        .collect();
    let count = items.len();
    Ok(Json(MediaListResponse { items, count }))
}

/// Returns a cached JPEG thumbnail for an image item. For items that can't be
/// decoded server-side (HEIC, video), returns 415 so the client falls back.
pub async fn get_thumb(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response, ApiError> {
    let record = state
        .storage
        .get_by_id(&id)
        .ok_or_else(|| ApiError::BadRequest("no such media id".into()))?;
    match state.storage.thumbnail_bytes(&record) {
        Some(bytes) => Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "private, max-age=86400")
            .body(Body::from(bytes))
            .map_err(|e| ApiError::Internal(e.to_string()))?),
        None => Err(ApiError::BadRequest("no thumbnail for this media type".into())),
    }
}

/// Streams a stored media file's bytes, honoring HTTP `Range` requests so the
/// web gallery can seek/stream video. Without a Range header the whole file is
/// returned with `Accept-Ranges: bytes` advertised.
pub async fn get_media(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let record = state
        .storage
        .get_by_id(&id)
        .ok_or_else(|| ApiError::BadRequest("no such media id".into()))?;
    let path = state.storage.absolute_path(&record);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("reading media: {e}")))?;
    let total = bytes.len() as u64;

    // Serve a partial range if requested (e.g. video seeking).
    if let Some((start, end)) = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| parse_range(r, total))
    {
        let slice = bytes[start as usize..=end as usize].to_vec();
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, record.content_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .header(header::CONTENT_LENGTH, (end - start + 1).to_string())
            .body(Body::from(slice))
            .map_err(|e| ApiError::Internal(e.to_string()));
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, record.content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, total.to_string())
        .body(Body::from(bytes))
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Parses a single-range `Range: bytes=start-end` header into inclusive byte
/// offsets clamped to the file size. Returns None if unparseable/unsatisfiable.
fn parse_range(header_value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header_value.strip_prefix("bytes=")?;
    let (start_str, end_str) = spec.split_once('-')?;
    let start: u64 = if start_str.is_empty() { 0 } else { start_str.parse().ok()? };
    let end: u64 = if end_str.is_empty() { total.saturating_sub(1) } else { end_str.parse().ok()? };
    if total == 0 || start > end || start >= total {
        return None;
    }
    Some((start, end.min(total - 1)))
}

/// Serves the self-contained web gallery single-page app.
pub async fn gallery() -> impl IntoResponse {
    Html(include_str!("gallery.html"))
}
