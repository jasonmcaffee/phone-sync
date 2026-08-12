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
    let storage = Storage::open(
        cfg.data_dir.clone(),
        cfg.media_root.clone(),
        cfg.media_folder_suffix.clone(),
    )?;
    tracing::info!(
        "data dir {} | media root {} (filed as <year>/<yyyymm>-{})",
        cfg.data_dir.display(),
        cfg.media_root.display(),
        cfg.media_folder_suffix
    );
    let state = AppState {
        config: Arc::new(cfg),
        storage: Arc::new(storage),
    };

    // Background: pre-generate thumbnails for the whole library so the web UI is
    // fast even for years of existing photos/videos. Skips items already cached,
    // so it's cheap on subsequent restarts.
    {
        let storage = state.storage.clone();
        let ffmpeg = state.config.ffmpeg_path.clone();
        tokio::spawn(async move {
            let records = storage.all_records();
            let mut made = 0usize;
            for record in records {
                if storage.has_thumbnail(&record.sha256) {
                    continue;
                }
                let (s, f) = (storage.clone(), ffmpeg.clone());
                if tokio::task::spawn_blocking(move || s.thumbnail_bytes(&record, &f))
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    made += 1;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            if made > 0 {
                tracing::info!("thumbnail pre-generation complete: {made} generated");
            }
        });
    }

    let bind_addr = state.config.bind_addr.clone();
    let app = build_app(state);

    let listener = bind_with_retry(&bind_addr).await?;
    tracing::info!("phone-sync-server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Binds the listener, retrying briefly while the address is still in use.
///
/// A service-manager "restart" starts the replacement before Windows has
/// finished releasing the old process's socket, which otherwise kills the new
/// process outright with `os error 10048` and leaves the backup server down.
/// @param bind_addr - the address to listen on
async fn bind_with_retry(bind_addr: &str) -> anyhow::Result<tokio::net::TcpListener> {
    const ATTEMPTS: u32 = 30;
    for attempt in 1..=ATTEMPTS {
        match tokio::net::TcpListener::bind(bind_addr).await {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempt < ATTEMPTS => {
                tracing::warn!("{bind_addr} still in use (attempt {attempt}/{ATTEMPTS}), retrying...");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!("the loop either binds or returns the final error")
}
