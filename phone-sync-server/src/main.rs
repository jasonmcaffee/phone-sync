//! Phone Sync backend entry point: loads config, opens storage, serves the app.

use std::sync::Arc;

use phone_sync_server::state::AppState;
use phone_sync_server::storage::Storage;
use phone_sync_server::{build_app, config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let cfg = config::load();
    let storage = Storage::open(cfg.data_dir.clone())?;
    let state = AppState {
        config: Arc::new(cfg),
        storage: Arc::new(storage),
    };

    let bind_addr = state.config.bind_addr.clone();
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("phone-sync-server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
