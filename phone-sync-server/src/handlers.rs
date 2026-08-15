//! HTTP request handlers for auth, manifest listing, and media upload/fetch.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;

use crate::error::ApiError;
use crate::imaging::MediaTools;
use crate::models::{
    ChunkAck, ChunkMetadata, ChunkStatusResponse, CompleteRequest, LoginRequest, LoginResponse,
    ManifestResponse, MediaListItem, MediaListResponse, MediaRecord, PageQuery, UploadMetadata,
    UploadResponse,
};
use crate::state::AppState;
use crate::{auth, storage};
use sha2::Digest;

/// A year of caching: every media, thumbnail and preview URL is keyed by the
/// content's sha256, so the bytes behind a given URL can never change.
const IMMUTABLE_CACHE: &str = "private, max-age=31536000, immutable";

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
pub async fn upload_status(State(state): State<AppState>, Path(sha256): Path<String>) -> Result<Json<ChunkStatusResponse>, ApiError> {
    require_content_hash(&sha256)?;
    let stored = state.storage.is_content_stored(&sha256);
    let received = if stored { Vec::new() } else { state.storage.received_chunk_indices(&sha256) };
    Ok(Json(ChunkStatusResponse { stored, received }))
}

/// Rejects a content hash that is not a plain 64-character hex sha256.
///
/// The chunk endpoints use this value to build the staging path, so anything
/// containing `..`, a separator or a drive letter would let a signed-in caller
/// choose where uploaded bytes land on disk. Validated at the edge as well as in
/// storage so a malformed hash fails as a 400 rather than a 500.
/// @param sha256 - the client-supplied content hash
fn require_content_hash(sha256: &str) -> Result<(), ApiError> {
    if crate::storage::is_valid_content_hash(sha256) {
        return Ok(());
    }
    Err(ApiError::BadRequest("sha256 must be 64 hex characters".into()))
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
    require_content_hash(&meta.sha256)?;
    state.storage.write_chunk(&meta.sha256, meta.chunk_index, &bytes).map_err(ApiError::from)?;
    Ok(Json(ChunkAck { received: meta.chunk_index, ok: true }))
}

/// Finalizes a chunked upload: assembles and verifies the staged chunks into the
/// stored file, filed by capture date like any other upload.
pub async fn upload_complete(State(state): State<AppState>, Json(req): Json<CompleteRequest>) -> Result<(StatusCode, Json<UploadResponse>), ApiError> {
    require_content_hash(&req.sha256)?;
    let (record, duplicate) = state
        .storage
        .assemble_and_store(&req.asset_id, &req.filename, &req.content_type, &req.media_type, &req.created_at, &req.sha256, req.total_chunks)
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(UploadResponse { id: record.sha256.clone(), sha256: record.sha256, stored: true, duplicate }),
    ))
}

/// Lists one page of stored media, newest first, for the web gallery.
///
/// Paged rather than whole-library: there are thousands of items, and sending
/// them all made the gallery wait on a multi-megabyte JSON body before it could
/// paint a single tile. Callers walk the library with `offset`/`limit` and stop
/// when `offset + items.len() >= count`.
pub async fn list_media(State(state): State<AppState>, Query(page): Query<PageQuery>) -> Result<Json<MediaListResponse>, ApiError> {
    let limit = page.limit.unwrap_or(state.config.default_page_size).clamp(1, state.config.max_page_size);
    let offset = page.offset.unwrap_or(0);
    let (records, count) = state.storage.records_page(offset, limit);

    let items: Vec<MediaListItem> = records
        .into_iter()
        .map(|r| MediaListItem {
            // The server thumbnails every format (ffmpeg for HEIC and video),
            // generated on demand and cached, so the grid never has a hole.
            thumbnailable: true,
            // Whether the *original* bytes can go straight into an <img>. HEIC
            // can't, so the gallery asks for /preview instead of guessing.
            browser_displayable: storage::is_browser_displayable(&r.content_type, &r.filename),
            served_content_type: storage::served_content_type(&r).to_string(),
            id: r.sha256,
            asset_id: r.asset_id,
            filename: r.filename,
            content_type: r.content_type,
            media_type: r.media_type,
            created_at: r.created_at,
            size: r.size,
            rel_path: r.rel_path,
        })
        .collect();
    Ok(Json(MediaListResponse { count, offset, limit, items }))
}

/// Stores a client-generated JPEG thumbnail for an item (request body is the
/// JPEG). Lets iOS supply previews for HEIC/video the server can't decode.
pub async fn put_thumbnail(State(state): State<AppState>, Path(id): Path<String>, body: axum::body::Bytes) -> Result<StatusCode, ApiError> {
    if state.storage.get_by_id(&id).is_none() {
        return Err(ApiError::BadRequest("no such media id".into()));
    }
    if body.is_empty() {
        return Err(ApiError::BadRequest("empty thumbnail".into()));
    }
    state.storage.store_thumbnail(&id, &body).map_err(ApiError::from)?;
    Ok(StatusCode::CREATED)
}

/// Returns the cached grid thumbnail for an item, rendering it on first request.
pub async fn get_thumb(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let max_dim = state.config.thumbnail_max_dim;
    render_cached_jpeg(state, id, headers, "thumb", move |storage, record, tools| {
        storage.thumbnail_bytes(record, tools, max_dim)
    })
    .await
}

/// Returns a full-screen JPEG rendition of an item, rendering it on first
/// request. This is what the lightbox shows for a HEIC — the format the bulk of
/// this library is in, and the one no browser can decode.
pub async fn get_preview(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let max_dim = state.config.preview_max_dim;
    render_cached_jpeg(state, id, headers, "preview", move |storage, record, tools| {
        storage.preview_bytes(record, tools, max_dim)
    })
    .await
}

/// Shared plumbing for the thumbnail and preview endpoints: resolve the record,
/// answer a conditional request from the ETag, then render off the async runtime.
///
/// Rendering is ffmpeg subprocess work that can take half a second per HEIC, so
/// it runs on the blocking pool — otherwise one slow decode stalls every other
/// request on the executor.
/// @param state - shared application state
/// @param id - content hash of the item
/// @param headers - request headers, inspected for `If-None-Match`
/// @param variant - which rendition this is, so its ETag is distinct
/// @param render - produces the JPEG bytes for the record
async fn render_cached_jpeg<F>(state: AppState, id: String, headers: HeaderMap, variant: &'static str, render: F) -> Result<Response, ApiError>
where
    F: FnOnce(&crate::storage::Storage, &MediaRecord, &MediaTools) -> Option<Vec<u8>> + Send + 'static,
{
    let record = state
        .storage
        .get_by_id(&id)
        .ok_or_else(|| ApiError::BadRequest("no such media id".into()))?;

    let etag = quoted_etag(&record.sha256, variant);
    if is_unmodified(&headers, &etag) {
        return not_modified(&etag);
    }

    let storage = state.storage.clone();
    let tools = state.config.media_tools();
    let bytes = tokio::task::spawn_blocking(move || render(&storage, &record, &tools))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("no rendition available".into()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, IMMUTABLE_CACHE)
        .header(header::ETAG, &etag)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Streams a stored media file, honouring HTTP `Range` so a browser can start
/// playing a video immediately and seek within it.
///
/// The bytes are read from the file in bounded chunks and streamed out; nothing
/// is buffered whole. That matters more here than anywhere else in the service:
/// the largest clip in the library is 4.97 GB, and the previous implementation
/// read the entire file into memory before slicing the requested range out of
/// it — for every request, including the one-byte probe Safari opens with. No
/// video could play, and a handful of requests would have taken the box down.
pub async fn get_media(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    use tokio::io::AsyncSeekExt;

    let record = state
        .storage
        .get_by_id(&id)
        .ok_or_else(|| ApiError::BadRequest("no such media id".into()))?;

    let etag = quoted_etag(&record.sha256, "orig");
    // A conditional request is only safely answerable with 304 when the client
    // isn't asking for a range it doesn't already hold.
    if headers.get(header::RANGE).is_none() && is_unmodified(&headers, &etag) {
        return not_modified(&etag);
    }

    let path = state.storage.absolute_path(&record);
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("opening media: {e}")))?;
    let total = file
        .metadata()
        .await
        .map_err(|e| ApiError::Internal(format!("stat media: {e}")))?
        .len();

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|value| parse_range(value, total))
        .unwrap_or(RangeRequest::Absent);
    if range == RangeRequest::Unsatisfiable {
        // A well-formed range that falls outside the file must be refused, not
        // silently answered with the whole thing.
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .body(Body::empty())
            .map_err(|e| ApiError::Internal(e.to_string()));
    }

    let content_type = storage::served_content_type(&record).to_string();
    let common = |builder: axum::http::response::Builder| {
        builder
            .header(header::CONTENT_TYPE, &content_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CACHE_CONTROL, IMMUTABLE_CACHE)
            .header(header::ETAG, &etag)
    };

    let (status, start, length) = match range {
        RangeRequest::Satisfiable { start, end } => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        _ => (StatusCode::OK, 0, total),
    };
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| ApiError::Internal(format!("seeking media: {e}")))?;
    }

    let stream = tokio_util::io::ReaderStream::new(tokio::io::AsyncReadExt::take(file, length));
    let mut builder = common(Response::builder().status(status)).header(header::CONTENT_LENGTH, length.to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        let end = start + length - 1;
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"));
    }
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Builds the quoted ETag for a content hash. Each rendition of the same content
/// gets its own tag, so an original and its thumbnail can never be mistaken for
/// one another by a cache.
/// @param sha256 - the content hash
/// @param variant - which rendition ("orig", "thumb", "preview")
fn quoted_etag(sha256: &str, variant: &str) -> String {
    format!("\"{sha256}-{variant}\"")
}

/// True when the client already holds this exact content, per `If-None-Match`.
/// @param headers - the request headers
/// @param etag - the tag for the content being served
fn is_unmodified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| value == "*" || value.split(',').any(|candidate| candidate.trim() == etag))
}

/// Builds the empty 304 response for a cache hit.
/// @param etag - the tag the client already holds
fn not_modified(etag: &str) -> Result<Response, ApiError> {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::CACHE_CONTROL, IMMUTABLE_CACHE)
        .header(header::ETAG, etag)
        .body(Body::empty())
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// What a client's `Range` header amounts to for a file of a known size.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum RangeRequest {
    /// No range header, or one in a form we don't implement — send the whole file.
    Absent,
    /// A range that falls within the file, as inclusive byte offsets.
    Satisfiable { start: u64, end: u64 },
    /// A well-formed range that lies outside the file — must be answered 416.
    Unsatisfiable,
}

/// Parses a single-range `Range: bytes=start-end` header.
///
/// Also handles the suffix form (`bytes=-500`, meaning the last 500 bytes),
/// which is how some players probe a file's tail for its moov atom. Anything
/// multi-range or otherwise unsupported is reported as [`RangeRequest::Absent`]
/// so the whole file is sent, which the RFC allows.
/// @param header_value - the raw `Range` header
/// @param total - size of the file in bytes
fn parse_range(header_value: &str, total: u64) -> RangeRequest {
    let Some(spec) = header_value.trim().strip_prefix("bytes=") else {
        return RangeRequest::Absent;
    };
    if spec.contains(',') {
        return RangeRequest::Absent;
    }
    let Some((start_str, end_str)) = spec.trim().split_once('-') else {
        return RangeRequest::Absent;
    };
    if total == 0 {
        return RangeRequest::Unsatisfiable;
    }

    // `bytes=-N` asks for the final N bytes rather than a range starting at 0.
    if start_str.is_empty() {
        let Ok(suffix) = end_str.parse::<u64>() else {
            return RangeRequest::Absent;
        };
        if suffix == 0 {
            return RangeRequest::Unsatisfiable;
        }
        let start = total.saturating_sub(suffix);
        return RangeRequest::Satisfiable { start, end: total - 1 };
    }

    let Ok(start) = start_str.parse::<u64>() else {
        return RangeRequest::Absent;
    };
    let end = if end_str.is_empty() {
        total - 1
    } else {
        match end_str.parse::<u64>() {
            Ok(value) => value.min(total - 1),
            Err(_) => return RangeRequest::Absent,
        }
    };
    if start > end || start >= total {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Satisfiable { start, end }
}

/// Serves the self-contained web gallery single-page app.
pub async fn gallery() -> impl IntoResponse {
    Html(include_str!("gallery.html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_ended_range_runs_to_the_end_of_the_file() {
        assert_eq!(parse_range("bytes=0-", 1000), RangeRequest::Satisfiable { start: 0, end: 999 });
        assert_eq!(parse_range("bytes=500-", 1000), RangeRequest::Satisfiable { start: 500, end: 999 });
    }

    #[test]
    fn safaris_opening_probe_is_a_two_byte_range() {
        assert_eq!(parse_range("bytes=0-1", 5_210_710_058), RangeRequest::Satisfiable { start: 0, end: 1 });
    }

    #[test]
    fn a_suffix_range_reads_the_tail() {
        assert_eq!(parse_range("bytes=-500", 1000), RangeRequest::Satisfiable { start: 500, end: 999 });
        // Asking for more tail than exists simply yields the whole file.
        assert_eq!(parse_range("bytes=-5000", 1000), RangeRequest::Satisfiable { start: 0, end: 999 });
    }

    #[test]
    fn an_end_past_the_file_is_clamped_rather_than_refused() {
        assert_eq!(parse_range("bytes=900-99999", 1000), RangeRequest::Satisfiable { start: 900, end: 999 });
    }

    #[test]
    fn a_range_beyond_the_file_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=2000-3000", 1000), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), RangeRequest::Unsatisfiable);
    }

    #[test]
    fn unsupported_forms_fall_back_to_the_whole_file() {
        assert_eq!(parse_range("items=0-1", 1000), RangeRequest::Absent);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), RangeRequest::Absent);
        assert_eq!(parse_range("bytes=abc-def", 1000), RangeRequest::Absent);
    }

    #[test]
    fn conditional_requests_match_on_the_exact_tag() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "\"abc-orig\"".parse().unwrap());
        assert!(is_unmodified(&headers, "\"abc-orig\""));
        assert!(!is_unmodified(&headers, "\"abc-thumb\""));

        headers.insert(header::IF_NONE_MATCH, "\"other\", \"abc-orig\"".parse().unwrap());
        assert!(is_unmodified(&headers, "\"abc-orig\""));
    }

    #[test]
    fn a_rendition_never_shares_a_tag_with_the_original() {
        assert_ne!(quoted_etag("abc", "orig"), quoted_etag("abc", "thumb"));
    }
}

