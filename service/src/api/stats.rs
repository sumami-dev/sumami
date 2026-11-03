//! Statistics endpoints.
//!
//! Every handler here is a thin wrapper around [`crate::reports`] — the same code the
//! CLI runs. The metric semantics are defined in exactly one place:
//!
//!   * `pageviews`    — total page views
//!   * `visitors`     — unique visitors (daily-rotating id)
//!   * `views`        — number of sessions (visits)
//!   * `bounces`      — sessions with exactly one page view
//!   * `visitDuration` — average session duration in seconds

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{BreakdownRow, MetricsSummary, MetricValue, StatsQuery};
use crate::storage::{Dimension, ReportQuery, TimeRange};

/// Resolve the requested window (Umami sends unix milliseconds).
pub(crate) fn resolve_range(query: &StatsQuery) -> TimeRange {
    let end = query
        .end_at
        .map(|ms| chrono::DateTime::from_timestamp_millis(ms).map(|d| d.naive_utc()))
        .unwrap_or_else(|| Some(crate::db::now()))
        .unwrap_or_else(crate::db::now);

    let start = query
        .start_at
        .map(|ms| chrono::DateTime::from_timestamp_millis(ms).map(|d| d.naive_utc()))
        .unwrap_or_else(|| Some(end - chrono::Duration::days(7)))
        .unwrap_or_else(|| end - chrono::Duration::days(7));

    TimeRange { start, end }
}

/// Build the shared report query from the request parameters.
pub(crate) fn report_query(id: &str, query: &StatsQuery) -> ReportQuery {
    let mut report = ReportQuery::new(id, resolve_range(query));
    report.dimensions = crate::storage::Dimensions {
        url: query.url.clone(),
        referrer: query.referrer.clone(),
        browser: query.browser.clone(),
        os: query.os.clone(),
        device: query.device.clone(),
        country: query.country.clone(),
    };
    report.limit = query.limit.unwrap_or(50).clamp(1, 1000);
    report.offset = query.offset.unwrap_or(0).max(0);
    report
}



/// `GET /api/websites/{id}/stats` — the summary metrics.
pub async fn summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<MetricsSummary>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.summary_with_change(&report)?))
}

/// `GET /api/websites/{id}/stats/metrics` — the same metrics, without the change.
pub async fn metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<MetricsSummary>, ApiError> {
    let report = report_query(&id, &query);
    let metrics = state.storage.summary(&report)?;
    Ok(Json(MetricsSummary {
        pageviews: MetricValue { value: metrics.pageviews, change: 0.0 },
        visitors: MetricValue { value: metrics.visitors, change: 0.0 },
        views: MetricValue { value: metrics.views, change: 0.0 },
        bounces: MetricValue { value: metrics.bounces, change: 0.0 },
        visit_duration: MetricValue { value: metrics.visit_duration as i64, change: 0.0 },
    }))
}

/// `GET /api/websites/{id}/stats/timeseries` — page views over time.
pub async fn timeseries(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.timeseries(&report)?))
}

/// `GET /api/websites/{id}/stats/pages` — top pages.
pub async fn pages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Url).await
}

/// `GET /api/websites/{id}/stats/referrers` — traffic sources.
pub async fn referrers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let rows = breakdown(state, id, query, Dimension::Referrer).await?;
    let rows = rows.0;
    Ok(Json(
        rows.into_iter()
            .map(|mut row| {
                if row.name == "unknown" {
                    row.name = "(direct)".to_string();
                }
                row
            })
            .collect(),
    ))
}

/// `GET /api/websites/{id}/stats/browsers`
pub async fn browsers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Browser).await
}

/// `GET /api/websites/{id}/stats/os`
pub async fn os(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Os).await
}

/// `GET /api/websites/{id}/stats/devices`
pub async fn devices(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Device).await
}

/// `GET /api/websites/{id}/stats/countries`
pub async fn countries(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Country).await
}

/// `GET /api/websites/{id}/stats/languages`
pub async fn languages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Language).await
}

/// `GET /api/websites/{id}/stats/screens`
pub async fn screens(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    breakdown(state, id, query, Dimension::Screen).await
}

/// `GET /api/websites/{id}/stats/events` — custom events, grouped by name.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.events(&report)?))
}

/// `GET /api/websites/{id}/stats/urls` — the number of distinct URLs visited.
pub async fn urls(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.breakdown(&report, Dimension::Url)?))
}

/// Shared breakdown handler.
async fn breakdown(
    state: AppState,
    id: String,
    query: StatsQuery,
    dimension: Dimension,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let report = report_query(&id, &query);
    Ok(Json(state.storage.breakdown(&report, dimension)?))
}
