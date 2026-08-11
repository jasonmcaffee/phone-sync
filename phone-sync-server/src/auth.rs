//! Authentication: password hashing/verification and JWT sign/verify, plus an
//! axum middleware that guards routes behind a valid Bearer token.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::State;
use axum::http::{header, Request};
use axum::middleware::Next;
use axum::response::Response;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand_core::OsRng;

use crate::error::ApiError;
use crate::models::Claims;
use crate::state::AppState;

/// Hashes a plaintext password with Argon2id and a random salt, returning the
/// PHC string. Used to seed the dev password and (potentially) rotate creds.
pub fn hash_password(plain: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .expect("hashing password should not fail")
        .to_string()
}

/// Verifies a plaintext password against a stored Argon2 PHC hash.
pub fn verify_password(plain: &str, phc_hash: &str) -> bool {
    match PasswordHash::new(phc_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(plain.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Signs a JWT for the given subject with the configured secret and TTL.
/// Returns the token string and its expiry (unix seconds).
pub fn issue_token(subject: &str, secret: &str, ttl_secs: i64) -> (String, i64) {
    let exp = chrono::Utc::now().timestamp() + ttl_secs;
    let claims = Claims {
        sub: subject.to_string(),
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("jwt encoding should not fail");
    (token, exp)
}

/// Decodes and validates a JWT (signature + expiry), returning its claims.
pub fn verify_token(token: &str, secret: &str) -> Result<Claims, ApiError> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|e| ApiError::Unauthorized(format!("invalid token: {e}")))
}

/// axum middleware that requires a valid JWT, supplied either as an
/// `Authorization: Bearer <jwt>` header (used by the iOS app and JSON APIs) or
/// as a `?token=<jwt>` query parameter (used by browser `<img>`/`<video>` tags
/// in the web gallery, which cannot set headers). On success the request
/// proceeds; otherwise a 401 is returned.
pub async fn require_auth(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let header_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let token = header_token
        .or_else(|| token_from_query(request.uri().query()))
        .ok_or_else(|| ApiError::Unauthorized("missing bearer token".into()))?;

    verify_token(&token, &state.config.jwt_secret)?;
    Ok(next.run(request).await)
}

/// Extracts a `token` value from a URL query string, if present.
fn token_from_query(query: Option<&str>) -> Option<String> {
    let query = query?;
    query.split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        match (parts.next(), parts.next()) {
            (Some("token"), Some(value)) => Some(value.to_string()),
            _ => None,
        }
    })
}
