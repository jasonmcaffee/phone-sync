//! Shared application state injected into every handler.

use std::sync::Arc;

use crate::config::Config;
use crate::storage::Storage;

/// Cloneable handle to shared, thread-safe application state.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<Storage>,
}
