//! Statistics endpoints.
//!
//! Metric semantics (documented in AGENTS.md and the docs site):
//!   * `pageviews`   — total page views
//!   * `visitors`    — unique visitors (daily-rotating, non-reversible id)
//!   * `views`       — number of sessions (visits)
//!   * `bounces`     — sessions with exactly one page view
//!   * `visitDuration` — average session duration in seconds

use axum::extract::{Path, Query, State};
use axum::Json;
use duckdb::types::Value;

use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{BreakdownRow, MetricValue, MetricsSummary, StatsQuery};

/// Default window when the caller does not specify one.
const DEFAULT_RANGE_DAYS: i64 = 7;

/// A resolved time window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Range {
    start: chrono::NaiveDateTime,
    end: chrono::NaiveDateTime,
}

impl Range {
    /// The window immediately before this one, of the same length.
    fn previous(&self) -> Range {
        let span = self.end - self.start;
        Range {
            start: self.start - span,
            end: self.start,
        }
    }

    /// Length in seconds.
    fn seconds(&self) -> i64 {
        (self.end - self.start).num_seconds()
    }
}

/// Resolve the requested window (Umami sends unix milliseconds).
pub(crate) fn resolve_range(query: &StatsQuery) -> Range {
    let end = query
        .end_at
        .map(|ms| chrono::DateTime::from_timestamp_millis(ms).map(|d| d.naive_utc()))
        .unwrap_or_else(|| Some(crate::db::now()))
        .unwrap_or_else(crate::db::now);

    let start = query
        .start_at
        .map(|ms| chrono::DateTime::from_timestamp_millis(ms).map(|d| d.naive_utc()))
        .unwrap_or_else(|| Some(end - chrono::Duration::days(DEFAULT_RANGE_DAYS)))
        .unwrap_or_else(|| end - chrono::Duration::days(DEFAULT_RANGE_DAYS));

    Range { start, end }
}

/// A WHERE clause built from the optional filters.
pub(crate) struct Filters {
    clauses: Vec<String>,
    values: Vec<Value>,
}

/// Convert a timestamp into a DuckDB parameter.
fn ts(value: chrono::NaiveDateTime) -> Value {
    let epoch = chrono::DateTime::from_timestamp(0, 0)
        .map(|d| d.naive_utc())
        .unwrap_or_default();
    let micros = (value - epoch).num_microseconds().unwrap_or(0);
    Value::Timestamp(duckdb::types::TimeUnit::Microsecond, micros)
}

impl Filters {
    pub(crate) fn new(website_id: &str, range: Range) -> Self {
        let mut filters = Filters {
            clauses: vec!["website_id = ?".to_string()],
            values: vec![Value::Text(website_id.to_string())],
        };
        filters.push_value("created_at >= ?", ts(range.start));
        filters.push_value("created_at < ?", ts(range.end));
        filters
    }

    pub(crate) fn push_value(&mut self, clause: &str, value: Value) {
        self.clauses.push(clause.to_string());
        self.values.push(value);
    }

    pub(crate) fn push_text(&mut self, clause: &str, value: &str) {
        self.push_value(clause, Value::Text(value.to_string()));
    }

    /// Apply the dimension filters shared by every endpoint.
    pub(crate) fn apply_dimensions(&mut self, query: &StatsQuery) {
        for (column, value) in [
            ("url", &query.url),
            ("referrer", &query.referrer),
            ("browser", &query.browser),
            ("os", &query.os),
            ("device", &query.device),
            ("country", &query.country),
        ] {
            if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                self.push_text(&format!("{column} = ?"), value);
            }
        }
    }

    /// The SQL condition and its parameters.
    pub(crate) fn finish(&self) -> (String, Vec<Value>) {
        (self.clauses.join(" AND "), self.values.clone())
    }

    /// The parameters as a slice the driver understands.
    pub(crate) fn params(&self) -> Vec<&dyn duckdb::types::ToSql> {
        self.values
            .iter()
            .map(|v| v as &dyn duckdb::types::ToSql)
            .collect()
    }
}

/// `GET /api/websites/{id}/stats` — the summary metrics.
pub async fn summary(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<MetricsSummary>, ApiError> {
    let range = resolve_range(&query);
    let current = compute_metrics(&state, &id, &query, range)?;
    let previous = compute_metrics(&state, &id, &query, range.previous())?;

    Ok(Json(compare(current, previous)))
}

/// Compute the raw metric values for a window.
fn compute_metrics(
    state: &AppState,
    website_id: &str,
    query: &StatsQuery,
    range: Range,
) -> Result<(i64, i64, i64, i64, f64), ApiError> {
    let mut filters = Filters::new(website_id, range);
    filters.apply_dimensions(query);
    let (where_clause, _) = filters.finish();

    let sql = format!(
        "SELECT
            COALESCE(SUM(views), 0)                              AS pageviews,
            COUNT(DISTINCT visitor_id)                          AS visitors,
            COUNT(DISTINCT id)                                  AS visits,
            COUNT(*) FILTER (WHERE views <= 1)                   AS bounces,
            COALESCE(AVG(duration), 0)                           AS avg_duration
         FROM session
         WHERE {where_clause}"
    );

    let values = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(filters.params().as_slice())?;
            match rows.next()? {
                Some(row) => Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, f64>(4)?,
                )),
                None => Ok((0, 0, 0, 0, 0.0)),
            }
        })
        .map_err(ApiError::internal)?;

    Ok(values)
}

/// Build a metric with its period-over-period change.
fn compare(current: (i64, i64, i64, i64, f64), previous: (i64, i64, i64, i64, f64)) -> MetricsSummary {
    MetricsSummary {
        pageviews: metric(current.0, previous.0 as f64),
        visitors: metric(current.1, previous.1 as f64),
        views: metric(current.2, previous.2 as f64),
        bounces: metric(current.3, previous.3 as f64),
        visit_duration: metric(current.4 as i64, previous.4),
    }
}

fn metric(value: i64, previous: f64) -> MetricValue {
    let change = if previous == 0.0 {
        if value == 0 {
            0.0
        } else {
            100.0
        }
    } else {
        ((value as f64 - previous) / previous) * 100.0
    };
    MetricValue {
        value,
        change: (change * 10.0).round() / 10.0,
    }
}

/// `GET /api/websites/{id}/stats/metrics` — the same metrics without the change.
pub async fn metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<MetricsSummary>, ApiError> {
    let range = resolve_range(&query);
    let (pageviews, visitors, views, bounces, duration) =
        compute_metrics(&state, &id, &query, range)?;

    Ok(Json(MetricsSummary {
        pageviews: metric(pageviews, 0.0),
        visitors: metric(visitors, 0.0),
        views: metric(views, 0.0),
        bounces: metric(bounces, 0.0),
        visit_duration: metric(duration as i64, 0.0),
    }))
}

/// `GET /api/websites/{id}/stats/timeseries` — page views over time.
pub async fn timeseries(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let range = resolve_range(&query);
    let mut filters = Filters::new(&id, range);
    filters.apply_dimensions(&query);
    let (where_clause, _) = filters.finish();

    // Bucket by hour for short windows, by day for long ones.
    let granularity = if range.seconds() <= 2 * 24 * 3600 {
        "hour"
    } else {
        "day"
    };

    let sql = format!(
        "SELECT strftime(date_trunc('{granularity}', created_at), '%Y-%m-%dT%H:00:00Z') AS bucket,
                COALESCE(SUM(views), 0) AS value
         FROM session
         WHERE {where_clause}
         GROUP BY bucket
         ORDER BY bucket ASC"
    );

    let rows = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(filters.params().as_slice())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(BreakdownRow {
                    name: row.get::<_, String>(0)?,
                    value: row.get::<_, i64>(1)?,
                    url: None,
                    date: Some(row.get::<_, String>(0)?),
                });
            }
            Ok(out)
        })
        .map_err(ApiError::internal)?;

    Ok(Json(rows))
}

/// A breakdown over one dimension of the session table.
async fn breakdown(
    state: &AppState,
    website_id: &str,
    query: &StatsQuery,
    column: &str,
    aggregate: Aggregate,
    limit: i64,
) -> Result<Vec<BreakdownRow>, ApiError> {
    let range = resolve_range(query);
    let mut filters = Filters::new(website_id, range);
    filters.apply_dimensions(query);
    let (where_clause, _) = filters.finish();

    let expression = match aggregate {
        Aggregate::Views => "COALESCE(SUM(views), 0)".to_string(),
        Aggregate::Visitors => "COUNT(DISTINCT visitor_id)".to_string(),
        Aggregate::Sessions => "COUNT(DISTINCT id)".to_string(),
    };

    let sql = format!(
        "SELECT COALESCE({column}, 'unknown') AS name, {expression} AS value
         FROM session
         WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC
         LIMIT {limit}"
    );

    state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(filters.params().as_slice())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(BreakdownRow {
                    name: row.get::<_, String>(0)?,
                    value: row.get::<_, i64>(1)?,
                    url: None,
                    date: None,
                });
            }
            Ok(out)
        })
        .map_err(ApiError::internal)
}

/// Which aggregate a breakdown uses.
enum Aggregate {
    Views,
    Visitors,
    Sessions,
}

fn limit_of(query: &StatsQuery) -> i64 {
    query.limit.unwrap_or(50).clamp(1, 1000)
}

/// `GET /api/websites/{id}/stats/pages` — top pages.
pub async fn pages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let rows = breakdown(&state, &id, &query, "url", Aggregate::Views, limit_of(&query)).await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| BreakdownRow {
                url: Some(row.name.clone()),
                ..row
            })
            .collect(),
    ))
}

/// `GET /api/websites/{id}/stats/referrers` — traffic sources.
pub async fn referrers(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let rows = breakdown(
        &state,
        &id,
        &query,
        "referrer",
        Aggregate::Views,
        limit_of(&query),
    )
    .await?;
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
    Ok(Json(
        breakdown(&state, &id, &query, "browser", Aggregate::Visitors, limit_of(&query)).await?,
    ))
}

/// `GET /api/websites/{id}/stats/os`
pub async fn os(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    Ok(Json(
        breakdown(&state, &id, &query, "os", Aggregate::Visitors, limit_of(&query)).await?,
    ))
}

/// `GET /api/websites/{id}/stats/devices`
pub async fn devices(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    Ok(Json(
        breakdown(&state, &id, &query, "device", Aggregate::Visitors, limit_of(&query)).await?,
    ))
}

/// `GET /api/websites/{id}/stats/countries`
pub async fn countries(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    Ok(Json(
        breakdown(&state, &id, &query, "country", Aggregate::Visitors, limit_of(&query)).await?,
    ))
}

/// `GET /api/websites/{id}/stats/languages`
pub async fn languages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    Ok(Json(
        breakdown(&state, &id, &query, "language", Aggregate::Visitors, limit_of(&query)).await?,
    ))
}

/// `GET /api/websites/{id}/stats/screens`
pub async fn screens(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    Ok(Json(
        breakdown(&state, &id, &query, "screen", Aggregate::Visitors, limit_of(&query)).await?,
    ))
}

/// `GET /api/websites/{id}/stats/events` — custom events, grouped by name.
pub async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    let range = resolve_range(&query);
    let mut filters = Filters::new(&id, range);
    if let Some(name) = query.event.as_ref().filter(|v| !v.is_empty()) {
        filters.push_text("name = ?", name);
    }
    let (where_clause, _) = filters.finish();

    let sql = format!(
        "SELECT COALESCE(name, 'unknown') AS name, COUNT(*) AS value
         FROM event
         WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC
         LIMIT {}",
        limit_of(&query)
    );

    let rows = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(filters.params().as_slice())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(BreakdownRow {
                    name: row.get::<_, String>(0)?,
                    value: row.get::<_, i64>(1)?,
                    url: None,
                    date: None,
                });
            }
            Ok(out)
        })
        .map_err(ApiError::internal)?;

    Ok(Json(rows))
}

/// `GET /api/websites/{id}/stats/urls` — the number of distinct URLs visited.
pub async fn urls(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<BreakdownRow>>, ApiError> {
    Ok(Json(
        breakdown(&state, &id, &query, "url", Aggregate::Sessions, limit_of(&query)).await?,
    ))
}
