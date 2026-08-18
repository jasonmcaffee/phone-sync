//! Shared application state injected into every handler.

use std::sync::Arc;

use crate::config::Config;
use crate::publish::PublishStore;
use crate::storage::Storage;

/// Cloneable handle to shared, thread-safe application state.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<Storage>,
    /// The subset of the library published to the public media site (task-1569).
    pub publish: Arc<PublishStore>,
}
