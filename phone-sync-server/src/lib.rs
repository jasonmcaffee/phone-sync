//! Phone Sync backend library: modules and router construction, shared by the
//! binary (`main.rs`) and integration tests.

pub mod auth;
pub mod config;
pub mod error;
pub mod handlers;
pub mod models;
pub mod state;
pub mod storage;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
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
        .route("/api/media", get(handlers::list_media))
        .route("/media/:id", get(handlers::get_media))
        .route("/media/:id/thumb", get(handlers::get_thumb))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth));

    // Public routes (the gallery page shell loads, then authenticates via JS).
    Router::new()
        .route("/health", get(handlers::health))
        .route("/", get(handlers::gallery))
        .route("/auth/login", post(handlers::login))
        .merge(protected)
        .layer(DefaultBodyLimit::max(max))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
