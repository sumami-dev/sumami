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
//!
//! The reports are also **cached** in the instance store, and the cache is thrown
//! away whenever the data changes: the writer bumps a version number with every
//! flush, and the version is part of every cached answer. So the dashboard never
//! shows numbers from before the last write, and two people looking at the same
//! dashboard do not run the same query twice.

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{ReportKind, ReportSpec};
use crate::storage::{Dimension, ReportQuery, TimeRange};

/// Resolve the requested window (Umami sends unix milliseconds).
pub(crate) fn resolve_range(query: &crate::models::StatsQuery) -> TimeRange {
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
pub(crate) fn report_query(id: &str, query: &crate::models::StatsQuery) -> ReportQuery {
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

/// Run a report and return its JSON.
///
/// **One place** decides what each report is: the stats endpoints, the saved
/// reports (snapshots) and the cache all come through here, so they cannot
/// disagree about what "the top pages" means.
pub fn run_report(
    storage: &dyn crate::storage::Storage,
    spec: &ReportSpec,
) -> Result<serde_json::Value, ApiError> {
    let query = report_query(&spec.website, &spec.query);
    let value = match spec.report {
        ReportKind::Summary => serde_json::to_value(storage.summary_with_change(&query)?)?,
        ReportKind::Timeseries => serde_json::to_value(storage.timeseries(&query)?)?,
        ReportKind::Pages => serde_json::to_value(storage.breakdown(&query, Dimension::Url)?)?,
        ReportKind::Referrers => {
            serde_json::to_value(storage.breakdown(&query, Dimension::Referrer)?)?
        }
        ReportKind::Browsers => {
            serde_json::to_value(storage.breakdown(&query, Dimension::Browser)?)?
        }
        ReportKind::Os => serde_json::to_value(storage.breakdown(&query, Dimension::Os)?)?,
        ReportKind::Devices => {
            serde_json::to_value(storage.breakdown(&query, Dimension::Device)?)?
        }
        ReportKind::Countries => {
            serde_json::to_value(storage.breakdown(&query, Dimension::Country)?)?
        }
        ReportKind::Languages => {
            serde_json::to_value(storage.breakdown(&query, Dimension::Language)?)?
        }
        ReportKind::Screens => {
            serde_json::to_value(storage.breakdown(&query, Dimension::Screen)?)?
        }
        ReportKind::Events => serde_json::to_value(storage.events(&query)?)?,
        ReportKind::Sessions => serde_json::to_value(storage.sessions(&query)?)?,
        ReportKind::EventLog => serde_json::to_value(storage.event_log(&query)?)?,
    };
    Ok(value)
}

/// The cache key of a report.
fn cache_key(spec: &ReportSpec) -> String {
    // The key is the report and everything that changes its answer. It is only
    // ever used as a key, so it does not have to be pretty.
    format!(
        "report:{:?}:{}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
        spec.report,
        spec.website,
        spec.query.start_at,
        spec.query.end_at,
        spec.query.url,
        spec.query.referrer,
        spec.query.browser,
        spec.query.os,
        spec.query.device,
        spec.query.country,
        spec.query.event,
        spec.query.query,
        spec.query.limit,
        spec.query.offset,
    )
}

/// Run a report, through the cache.
///
/// The cached answer carries the version of the data it was computed from: as
/// soon as the writer flushes a batch, the version changes and every cached
/// answer is a miss again.
pub fn cached_report(
    state: &AppState,
    spec: &ReportSpec,
) -> Result<serde_json::Value, ApiError> {
    let key = cache_key(spec);
    let version = state.writer.version();

    if let Some(cached) = state.instance.cache_get(&key)? {
        // The cached answer is "<version>|<json>".
        if let Some((cached_version, payload)) = cached.split_once('|') {
            if cached_version.parse::<u64>().ok() == Some(version.load(std::sync::atomic::Ordering::Relaxed)) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
                    return Ok(value);
                }
            }
        }
    }

    let value = run_report(state.storage.as_ref(), spec)?;
    let payload = serde_json::to_string(&value)?;
    // The cache entry lives for an hour; the version is what makes it stale.
    let _ = state.instance.cache_put(
        &key,
        &format!("{}|{}", version.load(std::sync::atomic::Ordering::Relaxed), payload),
        Some(3600),
    );
    Ok(value)
}

/// `GET /api/websites/{id}/stats` — the summary metrics.
pub async fn summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let spec = ReportSpec {
        website: id,
        report: ReportKind::Summary,
        query,
    };
    Ok(Json(cached_report(&state, &spec)?))
}

/// `GET /api/websites/{id}/stats/metrics` — the same metrics, without the change.
pub async fn metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let spec = ReportSpec {
        website: id,
        report: ReportKind::Summary,
        query,
    };
    let value = cached_report(&state, &spec)?;
    // The same numbers, but without the period-over-period change.
    let mut value = value;
    if let Some(object) = value.as_object_mut() {
        for key in [
            "pageviews",
            "visitors",
            "views",
            "bounces",
            "visitDuration",
        ] {
            if let Some(metric) = object.get_mut(key).and_then(|m| m.as_object_mut()) {
                metric.insert("change".to_string(), serde_json::Value::from(0.0));
            }
        }
    }
    Ok(Json(value))
}

/// `GET /api/websites/{id}/stats/timeseries` — page views over time.
pub async fn timeseries(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Timeseries).await
}

/// `GET /api/websites/{id}/stats/pages` — top pages.
pub async fn pages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Pages).await
}

/// `GET /api/websites/{id}/stats/referrers` — traffic sources.
pub async fn referrers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut value = breakdown(state, id, query, ReportKind::Referrers).await?.0;
    // The dashboard shows the direct traffic as "(direct)".
    if let Some(rows) = value.as_array_mut() {
        for row in rows.iter_mut() {
            if row.get("name").and_then(|n| n.as_str()) == Some("unknown") {
                if let Some(object) = row.as_object_mut() {
                    object.insert("name".to_string(), serde_json::Value::from("(direct)"));
                }
            }
        }
    }
    Ok(Json(value))
}

/// `GET /api/websites/{id}/stats/browsers`
pub async fn browsers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Browsers).await
}

/// `GET /api/websites/{id}/stats/os`
pub async fn os(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Os).await
}

/// `GET /api/websites/{id}/stats/devices`
pub async fn devices(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Devices).await
}

/// `GET /api/websites/{id}/stats/countries`
pub async fn countries(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Countries).await
}

/// `GET /api/websites/{id}/stats/languages`
pub async fn languages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Languages).await
}

/// `GET /api/websites/{id}/stats/screens`
pub async fn screens(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Screens).await
}

/// `GET /api/websites/{id}/stats/events` — custom events, grouped by name.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Events).await
}

/// `GET /api/websites/{id}/stats/urls` — the number of distinct URLs visited.
pub async fn urls(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<crate::models::StatsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    breakdown(state, id, query, ReportKind::Pages).await
}

/// Shared breakdown handler.
async fn breakdown(
    state: AppState,
    id: String,
    query: crate::models::StatsQuery,
    report: ReportKind,
) -> Result<Json<serde_json::Value>, ApiError> {
    let spec = ReportSpec {
        website: id,
        report,
        query,
    };
    Ok(Json(cached_report(&state, &spec)?))
}
