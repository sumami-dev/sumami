//! Raw session and event listings.

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::api::stats::{resolve_range, Filters};
use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{EventRow, SessionRow, StatsQuery};

/// `GET /api/websites/{id}/sessions` — the session log.
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let range = resolve_range(&query);
    let mut filters = Filters::new(&id, range);
    filters.apply_dimensions(&query);
    let (where_clause, _) = filters.finish();

    let limit = query.limit.unwrap_or(50).clamp(1, 1000);
    let offset = query.offset.unwrap_or(0).max(0);

    let sql = format!(
        "SELECT id, website_id, created_at, updated_at, hostname, url, title, referrer, entry_url,
                browser, os, device, country, region, city, language, screen, views, duration
         FROM session
         WHERE {where_clause}
         ORDER BY created_at DESC
         LIMIT {limit} OFFSET {offset}"
    );

    let rows = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(filters.params().as_slice())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(SessionRow {
                    id: row.get(0)?,
                    website_id: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    hostname: row.get(4)?,
                    url: row.get(5)?,
                    title: row.get(6)?,
                    referrer: row.get(7)?,
                    entry_url: row.get(8)?,
                    browser: row.get(9)?,
                    os: row.get(10)?,
                    device: row.get(11)?,
                    country: row.get(12)?,
                    region: row.get(13)?,
                    city: row.get(14)?,
                    language: row.get(15)?,
                    screen: row.get(16)?,
                    views: row.get(17)?,
                    duration: row.get(18)?,
                });
            }
            Ok(out)
        })
        .map_err(ApiError::internal)?;

    Ok(Json(rows))
}

/// `GET /api/websites/{id}/events` — the custom event log.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<EventRow>>, ApiError> {
    let range = resolve_range(&query);
    let mut filters = Filters::new(&id, range);
    if let Some(name) = query.event.as_ref().filter(|v| !v.is_empty()) {
        filters.push_text("name = ?", name);
    }
    if let Some(text) = query.query.as_ref().filter(|v| !v.is_empty()) {
        filters.push_text("data LIKE ?", &format!("%{text}%"));
    }
    let (where_clause, _) = filters.finish();

    let limit = query.limit.unwrap_or(50).clamp(1, 1000);
    let offset = query.offset.unwrap_or(0).max(0);

    let sql = format!(
        "SELECT id, website_id, session_id, created_at, name, url, hostname, data
         FROM event
         WHERE {where_clause}
         ORDER BY created_at DESC
         LIMIT {limit} OFFSET {offset}"
    );

    let rows = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(filters.params().as_slice())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(EventRow {
                    id: row.get(0)?,
                    website_id: row.get(1)?,
                    session_id: row.get(2)?,
                    created_at: row.get(3)?,
                    name: row.get(4)?,
                    url: row.get(5)?,
                    hostname: row.get(6)?,
                    data: row.get(7)?,
                });
            }
            Ok(out)
        })
        .map_err(ApiError::internal)?;

    Ok(Json(rows))
}
