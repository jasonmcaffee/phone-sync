//! Phone Sync backend library: modules and router construction, shared by the
//! binary (`main.rs`) and integration tests.

pub mod auth;
pub mod config;
pub mod error;
pub mod handlers;
pub mod heif;
pub mod imaging;
pub mod models;
pub mod orientation;
pub mod publish;
pub mod publish_handlers;
pub mod serve;
pub mod state;
pub mod storage;
pub mod transcode;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, patch, post};
use axum::{middleware, Router};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use state::AppState;

/// Constructs the fully-wired application router from shared state. Exposed so
/// integration tests can spawn the same app the binary serves.
pub fn build_app(state: AppState) -> Router {
    let max = state.config.max_upload_bytes;

    // Routes requiring a valid token (Bearer header or ?token= query).
    let protected = Router::new()
        .route("/media/manifest", get(handlers::manifest))
        .route("/media/upload", post(handlers::upload))
        .route("/media/upload/status/:sha256", get(handlers::upload_status))
        .route("/media/upload/chunk", post(handlers::upload_chunk))
        .route("/media/upload/complete", post(handlers::upload_complete))
        .route("/media/verify", post(handlers::verify))
        .route("/api/media", get(handlers::list_media))
        .route("/media/:id", get(handlers::get_media))
        .route("/media/:id/thumb", get(handlers::get_thumb).post(handlers::put_thumbnail))
        .route("/media/:id/preview", get(handlers::get_preview))
        // Curating the public media site (task-1569). Publishing is a private
        // action; only the /public/* routes below are anonymous.
        .route("/api/publish", get(publish_handlers::list_published))
        .route("/api/publish/:sha256", post(publish_handlers::publish_media))
        .route(
            "/api/publish/item/:public_id",
            patch(publish_handlers::update_published).delete(publish_handlers::unpublish),
        )
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));

    // Public routes (the gallery page shell loads, then authenticates via JS).
    Router::new()
        .route("/health", get(handlers::health))
        .route("/", get(handlers::gallery))
        // The gallery's own assets. Public alongside the shell it belongs to —
        // they carry no library data, only the page's presentation.
        .route("/gallery.css", get(handlers::gallery_css))
        .route("/gallery.js", get(handlers::gallery_js))
        .route("/fonts/:name", get(handlers::gallery_font))
        .route("/auth/login", post(handlers::login))
        // The public face of media.jasonmcaffee.com. These see the publish index
        // and nothing else — there is no path from here into the private library.
        .route("/public/feed", get(publish_handlers::public_feed))
        .route("/public/item/:public_id", get(publish_handlers::public_item))
        .route("/public/asset/:public_id/:variant", get(publish_handlers::public_asset))
        .merge(protected)
        .layer(DefaultBodyLimit::max(max))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
