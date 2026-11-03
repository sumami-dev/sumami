//! Raw session and event listings — thin wrappers around [`crate::reports`].

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::api::stats::report_query;
use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{EventRow, SessionRow, StatsQuery};

/// `GET /api/websites/{id}/sessions` — the session log.
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.sessions(&report)?))
}

/// `GET /api/websites/{id}/events` — the custom event log.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<EventRow>>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.event_log(&report)?))
}
