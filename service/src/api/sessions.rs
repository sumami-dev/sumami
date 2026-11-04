//! Raw session and event listings — thin wrappers around [`crate::reports`].

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::api::stats::cached_report;
use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{ReportKind, ReportSpec};

/// `GET /api/websites/{id}/sessions` — the session log.
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let spec = ReportSpec {
        website: id,
        report: ReportKind::Sessions,
        query,
    };
    Ok(Json(cached_report(&state, &spec)?))
}

/// `GET /api/websites/{id}/events` — the raw event log.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let spec = ReportSpec {
        website: id,
        report: ReportKind::EventLog,
        query,
    };
    Ok(Json(cached_report(&state, &spec)?))
}
