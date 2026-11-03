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
    let user = state.storage.user(&request.username)?;

    let Some(user) = user else {
        // The same message as a wrong password, so the endpoint does not leak which
        // usernames exist.
        return Err(ApiError::BadRequest(
            "invalid username or password".into(),
        ));
    };

    if !crate::auth::verify_password(&request.password, &user.password_hash) {
        return Err(ApiError::BadRequest(
            "invalid username or password".into(),
        ));
    }

    let token = crate::auth::issue_token(&state.jwt_secret, &user.id, &user.username, &user.role)?;
    let _ = &state;

    Ok(Json(LoginResponse {
        token,
        user: User {
            id: user.id,
            username: user.username,
            role: user.role,
            created_at: user.created_at,
        },
    }))
}

/// `POST /api/auth/refresh` — issue a fresh token for a valid one.
pub async fn refresh(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<LoginResponse>, ApiError> {
    let token = crate::auth::issue_token(&state.jwt_secret, &user.id, &user.username, &user.role)?;

    let created_at = state
        .storage
        .user(&user.username)?
        .map(|u| u.created_at)
        .unwrap_or_else(crate::db::now);

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
