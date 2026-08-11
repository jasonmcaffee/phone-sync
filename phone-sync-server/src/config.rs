//! Runtime configuration for the Phone Sync backend.
//!
//! Values come from environment variables with sensible dev defaults so the
//! server runs out-of-the-box on macOS during development and can be locked
//! down via env on the Windows 11 production host.

use std::path::PathBuf;

/// Fully resolved server configuration.
#[derive(Clone)]
pub struct Config {
    /// Address to bind, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// Root directory where media files and the metadata index live.
    pub data_dir: PathBuf,
    /// The single seeded username permitted to sign in.
    pub username: String,
    /// Argon2 PHC hash of the seeded password.
    pub password_hash: String,
    /// HMAC secret used to sign/verify JWTs.
    pub jwt_secret: String,
    /// Token lifetime in seconds (default 1 year).
    pub token_ttl_secs: i64,
    /// Maximum accepted upload size in bytes.
    pub max_upload_bytes: usize,
}

/// Builds the configuration from environment variables, falling back to
/// development defaults (seeded user `jason`). Production must override the
/// JWT secret and, ideally, the password hash via env.
pub fn load() -> Config {
    let data_dir = std::env::var("PHONE_SYNC_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));

    // Dev default: seed password "modestMouse1!" hashed at startup so the hash
    // is always valid. Production should set PHONE_SYNC_PASSWORD_HASH explicitly.
    let password_hash = std::env::var("PHONE_SYNC_PASSWORD_HASH")
        .unwrap_or_else(|_| crate::auth::hash_password("modestMouse1!"));

    Config {
        bind_addr: std::env::var("PHONE_SYNC_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
        data_dir,
        username: std::env::var("PHONE_SYNC_USER").unwrap_or_else(|_| "jason".to_string()),
        password_hash,
        jwt_secret: std::env::var("PHONE_SYNC_JWT_SECRET")
            .unwrap_or_else(|_| "dev-insecure-change-me".to_string()),
        token_ttl_secs: std::env::var("PHONE_SYNC_TOKEN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(365 * 24 * 60 * 60),
        max_upload_bytes: std::env::var("PHONE_SYNC_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2 * 1024 * 1024 * 1024),
    }
}
