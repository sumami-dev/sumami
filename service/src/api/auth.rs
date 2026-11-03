//! Authentication endpoints.

use axum::extract::State;
use axum::Json;

use crate::api::AppState;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::models::{LoginRequest, LoginResponse, User};

/// `POST /api/login` — exchange credentials for a token.
pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let user = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, username, password_hash, role, created_at FROM \"user\" WHERE username = ?",
            )?;
            let mut rows = stmt.query([request.username.as_str()])?;
            match rows.next()? {
                Some(row) => Ok(Some((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, chrono::NaiveDateTime>(4)?,
                ))),
                None => Ok(None),
            }
        })
        .map_err(ApiError::internal)?;

    let (id, username, hash, role, created_at) =
        user.ok_or_else(|| ApiError::BadRequest("invalid username or password".into()))?;

    if !crate::auth::verify_password(&request.password, &hash) {
        // Same message as "unknown user" so the endpoint does not leak which usernames exist.
        return Err(ApiError::BadRequest(
            "invalid username or password".into(),
        ));
    }

    let token = crate::auth::issue_token(&state.jwt_secret, &id, &username, &role)
        .map_err(ApiError::internal)?;

    Ok(Json(LoginResponse {
        token,
        user: User {
            id,
            username,
            role,
            created_at,
        },
    }))
}

/// `POST /api/auth/refresh` — issue a fresh token for a valid one.
pub async fn refresh(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<LoginResponse>, ApiError> {
    let token = crate::auth::issue_token(&state.jwt_secret, &user.id, &user.username, &user.role)
        .map_err(ApiError::internal)?;

    let created_at = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare("SELECT created_at FROM \"user\" WHERE id = ?")?;
            let mut rows = stmt.query([user.id.as_str()])?;
            match rows.next()? {
                Some(row) => Ok(row.get::<_, chrono::NaiveDateTime>(0)?),
                None => Ok(chrono::Utc::now().naive_utc()),
            }
        })
        .map_err(ApiError::internal)?;

    Ok(Json(LoginResponse {
        token,
        user: User {
            id: user.id,
            username: user.username,
            role: user.role,
            created_at,
        },
    }))
}

/// `GET /api/me` — the authenticated user.
pub async fn me(user: AuthUser) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({
        "id": user.id,
        "username": user.username,
        "role": user.role,
    })))
}
