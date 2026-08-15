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

    tokio::spawn(backfill_thumbnails(state.clone()));

    let bind_addr = state.config.bind_addr.clone();
    let app = build_app(state);

    let listener = bind_with_retry(&bind_addr).await?;
    tracing::info!("phone-sync-server listening on {bind_addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Generates every missing grid thumbnail in the background so the gallery is
/// instant even on the first visit after a restart.
///
/// This matters at the library's real size: 2,700 items of which the vast
/// majority are HEIC, each needing ffmpeg to reassemble a 48-tile grid at around
/// half a second apiece. Run one at a time that is over twenty minutes of an
/// empty-looking grid, so a small pool of workers shares the queue — bounded, so
/// a backfill can't monopolise a machine that is also running everything else.
/// Items already cached are skipped, making a restart nearly free.
/// @param state - shared application state
async fn backfill_thumbnails(state: AppState) {
    let pending: Vec<_> = state
        .storage
        .all_records()
        .into_iter()
        .filter(|record| !state.storage.has_thumbnail(&record.sha256))
        .collect();
    if pending.is_empty() {
        return;
    }

    let workers = state.config.thumbnail_workers.max(1);
    let total = pending.len();
    tracing::info!("thumbnail backfill: {total} missing, {workers} workers");

    let queue = Arc::new(std::sync::Mutex::new(pending.into_iter()));
    let generated = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let (queue, generated, failed) = (queue.clone(), generated.clone(), failed.clone());
        let storage = state.storage.clone();
        let tools = state.config.media_tools();
        let max_dim = state.config.thumbnail_max_dim;
        handles.push(tokio::task::spawn_blocking(move || loop {
            let Some(record) = queue.lock().unwrap().next() else {
                return;
            };
            match storage.thumbnail_bytes(&record, &tools, max_dim) {
                Some(_) => {
                    let done = generated.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if done % 250 == 0 {
                        tracing::info!("thumbnail backfill: {done}/{total}");
                    }
                }
                None => {
                    failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!("could not thumbnail {} ({})", record.filename, record.rel_path);
                }
            }
        }));
    }
    for handle in handles {
        let _ = handle.await;
    }
    tracing::info!(
        "thumbnail backfill complete: {} generated, {} failed",
        generated.load(std::sync::atomic::Ordering::Relaxed),
        failed.load(std::sync::atomic::Ordering::Relaxed)
    );
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
