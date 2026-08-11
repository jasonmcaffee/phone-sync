//! Runtime configuration for the Phone Sync backend.
//!
//! Values come from environment variables with sensible dev defaults so the
//! server runs out-of-the-box on macOS during development and can be locked
//! down via env on the Windows 11 production host.

use std::path::{Path, PathBuf};

/// Fully resolved server configuration.
#[derive(Clone)]
pub struct Config {
    /// Address to bind, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// Root directory where the metadata index and thumbnail cache live.
    pub data_dir: PathBuf,
    /// Root directory the imported photos/videos are filed under, in
    /// `<year>/<yyyymm>-<suffix>` folders — e.g. `E:\pictures\2026\202608-phone-sync`.
    pub media_root: PathBuf,
    /// Suffix appended to each month folder name (`202608-phone-sync`).
    pub media_folder_suffix: String,
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
/// media root and, ideally, the password hash via env.
pub fn load() -> Config {
    let data_dir = std::env::var("PHONE_SYNC_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));

    // Where the actual photos/videos land. Defaults to the historical
    // content-addressed location inside the data dir so existing installs and
    // the test suite keep working when the variable is unset.
    let media_root = std::env::var("PHONE_SYNC_MEDIA_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("media"));

    // Dev default: seed password "modestMouse1!" hashed at startup so the hash
    // is always valid. Production should set PHONE_SYNC_PASSWORD_HASH explicitly.
    let password_hash = std::env::var("PHONE_SYNC_PASSWORD_HASH")
        .unwrap_or_else(|_| crate::auth::hash_password("modestMouse1!"));

    Config {
        bind_addr: std::env::var("PHONE_SYNC_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
        media_folder_suffix: std::env::var("PHONE_SYNC_MEDIA_FOLDER_SUFFIX")
            .unwrap_or_else(|_| "phone-sync".to_string()),
        username: std::env::var("PHONE_SYNC_USER").unwrap_or_else(|_| "jason".to_string()),
        password_hash,
        jwt_secret: resolve_jwt_secret(&data_dir),
        token_ttl_secs: std::env::var("PHONE_SYNC_TOKEN_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(365 * 24 * 60 * 60),
        max_upload_bytes: std::env::var("PHONE_SYNC_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2 * 1024 * 1024 * 1024),
        data_dir,
        media_root,
    }
}

/// Resolves the JWT signing secret: an explicit `PHONE_SYNC_JWT_SECRET` wins,
/// otherwise a random 256-bit secret is generated once and persisted in the data
/// dir so tokens survive restarts. Shipping a known constant would let anyone
/// mint a valid token for an internet-facing deployment, so there is no
/// hard-coded fallback beyond an in-memory random secret.
/// @param data_dir - directory the generated secret is persisted in
fn resolve_jwt_secret(data_dir: &Path) -> String {
    match std::env::var("PHONE_SYNC_JWT_SECRET") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => load_or_create_jwt_secret(data_dir).unwrap_or_else(|_| random_hex_secret()),
    }
}

/// Reads the persisted signing secret, generating and writing one on first run.
/// @param data_dir - directory holding the `jwt-secret` file
fn load_or_create_jwt_secret(data_dir: &Path) -> std::io::Result<String> {
    let path = data_dir.join("jwt-secret");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    std::fs::create_dir_all(data_dir)?;
    let secret = random_hex_secret();
    std::fs::write(&path, &secret)?;
    Ok(secret)
}

/// Generates a 256-bit hex secret from the OS CSPRNG.
fn random_hex_secret() -> String {
    use rand_core::RngCore;
    let mut bytes = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
