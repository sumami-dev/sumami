//! Authentication: password hashing, JWT issuing, and the request extractor.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::error::ApiError;

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// User id.
    pub sub: String,
    /// Username.
    pub username: String,
    /// Role (`admin` for now).
    pub role: String,
    /// Issued at (seconds since epoch).
    pub iat: i64,
    /// Expiry (seconds since epoch).
    pub exp: i64,
}

/// How long a token is valid.
pub const TOKEN_TTL_SECONDS: i64 = 60 * 60 * 24 * 7;

/// Hash a password with Argon2id.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    // A random salt from the OS; the password-hash crate's RNG feature is not enabled
    // on purpose (it would pull in another dependency for one line).
    // 18 bytes -> a valid unpadded base64 salt (the length must be a multiple of 3).
    let salt = SaltString::encode_b64(&crate::config::random_bytes(18))
        .map_err(|e| anyhow::anyhow!("generating salt: {e}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))?;
    Ok(hash.to_string())
}

/// Verify a password against a stored Argon2 hash.
pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Issue a signed token for a user.
pub fn issue_token(
    secret: &str,
    user_id: &str,
    username: &str,
    role: &str,
) -> anyhow::Result<String> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        role: role.to_string(),
        iat: now,
        exp: now + TOKEN_TTL_SECONDS,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| anyhow::anyhow!("signing token: {e}"))
}

/// Decode and validate a token.
pub fn decode_token(secret: &str, token: &str) -> anyhow::Result<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| anyhow::anyhow!("invalid token: {e}"))
}

/// An authenticated caller.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub username: String,
    pub role: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthorized)?;

        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .unwrap_or(header)
            .trim();

        let claims =
            decode_token(&state.jwt_secret, token).map_err(|_| ApiError::Unauthorized)?;

        Ok(AuthUser {
            id: claims.sub,
            username: claims.username,
            role: claims.role,
        })
    }
}
