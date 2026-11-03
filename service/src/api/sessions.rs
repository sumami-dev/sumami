//! Raw session and event listings — thin wrappers around [`crate::reports`].

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::api::stats::report_query;
use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{EventRow, SessionRow, StatsQuery};
use crate::reports;

/// `GET /api/websites/{id}/sessions` — the session log.
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let report = report_query(&id, &query);
    let rows = state
        .db
        .with_read(|conn| reports::sessions(conn, &report))
        .map_err(ApiError::internal)?;
    Ok(Json(rows))
}

/// `GET /api/websites/{id}/events` — the custom event log.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<EventRow>>, ApiError> {
    let report = report_query(&id, &query);
    let rows = state
        .db
        .with_read(|conn| reports::event_log(conn, &report))
        .map_err(ApiError::internal)?;
    Ok(Json(rows))
}
