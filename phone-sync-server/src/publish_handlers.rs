//! HTTP surface for publishing to the media site (task-1569).
//!
//! Two clearly separated halves, and the separation is the security model:
//!
//! * The `/api/publish*` routes sit behind the same auth middleware as the rest
//!   of the private API. They are how Jason curates.
//! * The `/public/*` routes are anonymous, and they can only ever see the
//!   publish index. There is no code path from here into `Storage`, into the
//!   media root, or into any record that has not been explicitly published.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::handlers;
use crate::publish::{PublicItem, PublishFields, PublishedItem, Variant};
use crate::state::AppState;

/// Published derivatives are addressed by a random id and never change once
/// written, so they can be cached hard — by the browser and by Cloudflare, which
/// is what keeps this box out of the path of a popular photograph.
const PUBLIC_CACHE: &str = "public, max-age=31536000, immutable";

/// The feed is the one public response that changes when something is published,
/// so it is cached briefly rather than not at all.
const FEED_CACHE: &str = "public, max-age=30, stale-while-revalidate=300";

/// How many items a feed page carries when the caller does not say.
const DEFAULT_FEED_LIMIT: usize = 40;
/// Ceiling on a caller-requested page size.
const MAX_FEED_LIMIT: usize = 120;

/// Paging and filtering for the public feed.
#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    /// Offset into the published set; the previous page's `next_cursor`.
    pub cursor: Option<usize>,
    pub limit: Option<usize>,
    /// "photo", "video" or "featured". Anything else returns everything.
    pub kind: Option<String>,
}

/// One page of the public feed.
#[derive(Debug, Serialize)]
pub struct FeedResponse {
    pub items: Vec<PublicItem>,
    /// Offset to pass as the next `cursor`, or null at the end of the stream.
    pub next_cursor: Option<usize>,
    /// Total matching the current filter, so the client can show progress.
    pub total: usize,
    /// Totals across the whole published set, for the site's counts.
    pub photos: usize,
    pub videos: usize,
}

/// Everything published, with private ids, so the gallery can show which items
/// are already up. Authenticated.
pub async fn list_published(State(state): State<AppState>) -> Json<Vec<PublishedItem>> {
    Json(state.publish.all())
}

/// Publishes one library item to the media site. Authenticated.
///
/// Synchronous by design: rendering a photo's derivatives takes a second or two
/// and transcoding a video takes up to a minute, and a job queue with status
/// polling is more moving parts than a one-person publish flow needs. The
/// gallery issues these one at a time with a per-tile spinner.
/// @param sha256 - the private content id of the record to publish
pub async fn publish_media(State(state): State<AppState>, Path(sha256): Path<String>, body: Option<Json<PublishFields>>) -> Result<(StatusCode, Json<PublishedItem>), ApiError> {
    let record = state
        .storage
        .get_by_id(&sha256)
        .ok_or_else(|| ApiError::BadRequest("no such media id".into()))?;
    let fields = body.map(|Json(f)| f).unwrap_or_default();

    // ffmpeg and image decoding are blocking work; running them on the async
    // runtime's worker would stall every other request on this server for the
    // length of a transcode.
    let publish = state.publish.clone();
    let storage = state.storage.clone();
    let tools = state.config.media_tools();
    let item = tokio::task::spawn_blocking(move || publish.publish(&storage, &tools, &record, &fields))
        .await
        .map_err(|e| ApiError::Internal(format!("publish task failed: {e}")))?
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(item)))
}

/// Edits a published item's title, caption or featured flag. Authenticated.
/// @param public_id - the item to edit
pub async fn update_published(State(state): State<AppState>, Path(public_id): Path<String>, Json(fields): Json<PublishFields>) -> Result<Json<PublishedItem>, ApiError> {
    state
        .publish
        .update(&public_id, &fields)
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("no such published item".into()))
}

/// Unpublishes an item and deletes the derivatives that were rendered for it.
/// The original photo or video in the private library is untouched.
/// Authenticated.
/// @param public_id - the item to unpublish
pub async fn unpublish(State(state): State<AppState>, Path(public_id): Path<String>) -> Result<StatusCode, ApiError> {
    let removed = state.publish.remove(&public_id).map_err(|e| ApiError::Internal(e.to_string()))?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("no such published item".into()))
    }
}

/// One page of published items, newest capture first. Public.
pub async fn public_feed(State(state): State<AppState>, Query(query): Query<FeedQuery>) -> Result<Response, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_FEED_LIMIT).clamp(1, MAX_FEED_LIMIT);
    let cursor = query.cursor.unwrap_or(0);
    let (items, total) = state.publish.page(cursor, limit, query.kind.as_deref());
    let (photos, videos) = state.publish.counts();

    let next = cursor + items.len();
    let body = FeedResponse {
        items,
        next_cursor: (next < total).then_some(next),
        total,
        photos,
        videos,
    };
    json_response(&body, FEED_CACHE)
}

/// One published item by public id, for a deep-linked detail page. Public.
/// @param public_id - the id from the URL
pub async fn public_item(State(state): State<AppState>, Path(public_id): Path<String>) -> Result<Response, ApiError> {
    let item = state
        .publish
        .by_public_id(&public_id)
        .ok_or_else(|| ApiError::NotFound("no such item".into()))?;
    json_response(&PublicItem::from(&item), FEED_CACHE)
}

/// Serves one rendition of one published item. Public.
///
/// The only filesystem access the public surface has. `variant` is parsed into a
/// closed enum before it is used, so the file name is always one of five
/// constants and a caller-supplied path fragment can never reach the disk; the
/// store then confirms the resolved path is inside the publish root.
/// @param public_id - the id from the URL
/// @param variant - the requested rendition name
pub async fn public_asset(State(state): State<AppState>, Path((public_id, variant)): Path<(String, String)>, headers: HeaderMap) -> Result<Response, ApiError> {
    let variant = Variant::parse(&variant).ok_or_else(|| ApiError::NotFound("no such variant".into()))?;
    let path = state
        .publish
        .variant_path(&public_id, variant)
        .ok_or_else(|| ApiError::NotFound("no such item".into()))?;
    let etag = handlers::quoted_etag(&public_id, variant.key());
    handlers::serve_file_range(&path, variant.content_type(), &etag, PUBLIC_CACHE, &headers).await
}

/// Serializes a public JSON body with an explicit caching policy.
///
/// `Json` would answer with no `Cache-Control` at all, which means Cloudflare
/// revalidates the feed on every visitor.
/// @param body - the value to serialize
/// @param cache_control - the caching policy to attach
fn json_response<T: Serialize>(body: &T, cache_control: &str) -> Result<Response, ApiError> {
    let bytes = serde_json::to_vec(body).map_err(|e| ApiError::Internal(e.to_string()))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, cache_control)
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiError::Internal(e.to_string()))
}
