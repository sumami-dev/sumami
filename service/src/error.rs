//! A single error type for the HTTP layer.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// An API error with an HTTP status and a JSON body.
#[derive(Debug)]
pub enum ApiError {
    /// 400 — the request was malformed.
    BadRequest(String),
    /// 401 — missing or invalid credentials.
    Unauthorized,
    /// 404 — the resource does not exist.
    NotFound(String),
    /// 409 — the resource already exists.
    Conflict(String),
    /// 500 — something went wrong on our side.
    Internal(String),
}

impl ApiError {
    /// Build an internal error from any error type.
    pub fn internal(err: impl std::fmt::Display) -> Self {
        ApiError::Internal(err.to_string())
    }

    /// The HTTP status code for this error.
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The human-readable message.
    pub fn message(&self) -> String {
        match self {
            ApiError::BadRequest(m)
            | ApiError::NotFound(m)
            | ApiError::Conflict(m)
            | ApiError::Internal(m) => m.clone(),
            ApiError::Unauthorized => "unauthorized".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(json!({ "error": self.message(), "status": status.as_u16() }));
        (status, body).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::internal(err)
    }
}
