//! The one implementation of every report.
//!
//! The server and the CLI are the same program in two modes, so they must not have
//! two implementations of "what a page view is" or "how many visitors did this page
//! have". Everything in this module is shared: the API handlers are thin wrappers
//! around it, and the CLI's local mode calls exactly the same functions.
//!
//! Consequence: the numbers the CLI prints and the numbers the API returns are equal
//! **by construction**, not by convention.

use anyhow::Result;
use chrono::NaiveDateTime;
use duckdb::types::Value;
use duckdb::Connection;

use crate::models::{
    BreakdownRow, EventRow, MetricsSummary, MetricValue, SessionRow, Website,
};

/// A time window (start inclusive, end exclusive).
#[derive(Debug, Clone, Copy)]
pub struct TimeRange {
    /// Start of the window.
    pub start: NaiveDateTime,
    /// End of the window.
    pub end: NaiveDateTime,
}

impl TimeRange {
    /// The window immediately before this one, of the same length.
    pub fn previous(&self) -> TimeRange {
        let span = self.end - self.start;
        TimeRange {
            start: self.start - span,
            end: self.start,
        }
    }

    /// The default window: the last 7 days.
    pub fn last_days(days: i64) -> TimeRange {
        let end = crate::db::now();
        TimeRange {
            start: end - chrono::Duration::days(days),
            end,
        }
    }
}

/// The dimension filters (Umami-compatible query parameters).
#[derive(Debug, Clone, Default)]
pub struct Dimensions {
    /// Only this page.
    pub url: Option<String>,
    /// Only this referrer.
    pub referrer: Option<String>,
    /// Only this browser.
    pub browser: Option<String>,
    /// Only this operating system.
    pub os: Option<String>,
    /// Only this device class.
    pub device: Option<String>,
    /// Only this country.
    pub country: Option<String>,
}

/// Everything a report needs.
#[derive(Debug, Clone)]
pub struct ReportQuery {
    /// The website the report is about.
    pub website_id: String,
    /// The time window.
    pub range: TimeRange,
    /// The dimension filters.
    pub dimensions: Dimensions,
    /// How many rows to return.
    pub limit: i64,
    /// How many rows to skip.
    pub offset: i64,
}

impl ReportQuery {
    /// A query for one website and one window.
    pub fn new(website_id: &str, range: TimeRange) -> Self {
        Self {
            website_id: website_id.to_string(),
            range,
            dimensions: Dimensions::default(),
            limit: 50,
            offset: 0,
        }
    }
}

/// The dimensions a breakdown can be computed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    /// The page path.
    Url,
    /// The referring page.
    Referrer,
    /// The browser family.
    Browser,
    /// The operating system family.
    Os,
    /// The device class.
    Device,
    /// The country.
    Country,
    /// The interface language.
    Language,
    /// The viewport size.
    Screen,
}

impl Dimension {
    /// The column in the `session` table.
    pub fn column(self) -> &'static str {
        match self {
            Dimension::Url => "url",
            Dimension::Referrer => "referrer",
            Dimension::Browser => "browser",
            Dimension::Os => "os",
            Dimension::Device => "device",
            Dimension::Country => "country",
            Dimension::Language => "language",
            Dimension::Screen => "screen",
        }
    }

    /// How the rows of a breakdown are counted.
    ///
    /// Content dimensions (pages, referrers) count views; audience dimensions
    /// (browser, OS, device, country, language, screen) count unique visitors.
    pub fn aggregate(self) -> &'static str {
        match self {
            Dimension::Url | Dimension::Referrer => "COALESCE(SUM(views), 0)",
            _ => "COUNT(DISTINCT visitor_id)",
        }
    }

    /// Every dimension, in the order the API exposes them.
    pub fn all() -> [Dimension; 8] {
        [
            Dimension::Url,
            Dimension::Referrer,
            Dimension::Browser,
            Dimension::Os,
            Dimension::Device,
            Dimension::Country,
            Dimension::Language,
            Dimension::Screen,
        ]
    }
}

/// The raw metric values for a window.
#[derive(Debug, Clone, Copy, Default)]
pub struct SummaryMetrics {
    /// Total page views.
    pub pageviews: i64,
    /// Unique visitors (daily-rotating id).
    pub visitors: i64,
    /// Number of sessions.
    pub views: i64,
    /// Sessions with a single page view.
    pub bounces: i64,
    /// Average session duration in seconds.
    pub visit_duration: f64,
}

/// A WHERE clause and its parameters.
struct Filters {
    clauses: Vec<String>,
    values: Vec<Value>,
}

impl Filters {
    /// Start from a website and a time window.
    fn new(query: &ReportQuery) -> Self {
        let mut filters = Filters {
            clauses: vec!["website_id = ?".to_string()],
            values: vec![Value::Text(query.website_id.clone())],
        };
        filters.push_value("created_at >= ?", timestamp(query.range.start));
        filters.push_value("created_at < ?", timestamp(query.range.end));

        for (column, value) in [
            ("url", &query.dimensions.url),
            ("referrer", &query.dimensions.referrer),
            ("browser", &query.dimensions.browser),
            ("os", &query.dimensions.os),
            ("device", &query.dimensions.device),
            ("country", &query.dimensions.country),
        ] {
            if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                filters.push_text(&format!("{column} = ?"), value);
            }
        }

        filters
    }

    /// Add a parameterised condition.
    fn push_value(&mut self, clause: &str, value: Value) {
        self.clauses.push(clause.to_string());
        self.values.push(value);
    }

    /// Add a text parameterised condition.
    fn push_text(&mut self, clause: &str, value: &str) {
        self.push_value(clause, Value::Text(value.to_string()));
    }

    /// The SQL condition.
    fn clause(&self) -> String {
        self.clauses.join(" AND ")
    }

    /// The parameters, in order.
    fn params(&self) -> Vec<&dyn duckdb::types::ToSql> {
        self.values
            .iter()
            .map(|v| v as &dyn duckdb::types::ToSql)
            .collect()
    }
}

/// Convert a timestamp into a DuckDB parameter.
fn timestamp(value: NaiveDateTime) -> Value {
    let epoch = chrono::DateTime::from_timestamp(0, 0)
        .map(|d| d.naive_utc())
        .unwrap_or_default();
    let micros = (value - epoch).num_microseconds().unwrap_or(0);
    Value::Timestamp(duckdb::types::TimeUnit::Microsecond, micros)
}

/// List the websites.
pub fn websites(conn: &Connection) -> Result<Vec<Website>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, domain, created_at, updated_at, retention_days, country_header
         FROM website ORDER BY created_at DESC",
    )?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(Website {
            id: row.get(0)?,
            name: row.get(1)?,
            domain: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
            retention_days: row.get(5)?,
            country_header: row.get(6)?,
        });
    }
    Ok(out)
}

/// Compute the summary metrics for a window.
pub fn summary(conn: &Connection, query: &ReportQuery) -> Result<SummaryMetrics> {
    let filters = Filters::new(query);
    let sql = format!(
        "SELECT
            COALESCE(SUM(views), 0)                    AS pageviews,
            COUNT(DISTINCT visitor_id)                AS visitors,
            COUNT(DISTINCT id)                        AS visits,
            COUNT(*) FILTER (WHERE views <= 1)         AS bounces,
            COALESCE(AVG(duration), 0)                 AS avg_duration
         FROM session
         WHERE {}",
        filters.clause()
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(filters.params().as_slice())?;
    match rows.next()? {
        Some(row) => Ok(SummaryMetrics {
            pageviews: row.get(0)?,
            visitors: row.get(1)?,
            views: row.get(2)?,
            bounces: row.get(3)?,
            visit_duration: row.get(4)?,
        }),
        None => Ok(SummaryMetrics::default()),
    }
}

/// Build the API-shaped summary, including the period-over-period change.
pub fn summary_with_change(conn: &Connection, query: &ReportQuery) -> Result<MetricsSummary> {
    let current = summary(conn, query)?;
    let mut previous_query = query.clone();
    previous_query.range = query.range.previous();
    let previous = summary(conn, &previous_query)?;

    Ok(MetricsSummary {
        pageviews: metric(current.pageviews, previous.pageviews as f64),
        visitors: metric(current.visitors, previous.visitors as f64),
        views: metric(current.views, previous.views as f64),
        bounces: metric(current.bounces, previous.bounces as f64),
        visit_duration: metric(current.visit_duration as i64, previous.visit_duration),
    })
}

/// Build a metric with its period-over-period change.
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

/// Break a website down over one dimension.
pub fn breakdown(
    conn: &Connection,
    query: &ReportQuery,
    dimension: Dimension,
) -> Result<Vec<BreakdownRow>> {
    let filters = Filters::new(query);
    let sql = format!(
        "SELECT COALESCE({column}, 'unknown') AS name, {aggregate} AS value
         FROM session
         WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC
         LIMIT {limit}",
        column = dimension.column(),
        aggregate = dimension.aggregate(),
        where_clause = filters.clause(),
        limit = query.limit.clamp(1, 1000),
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(filters.params().as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        out.push(BreakdownRow {
            url: if dimension == Dimension::Url {
                Some(name.clone())
            } else {
                None
            },
            name,
            value: row.get(1)?,
            date: None,
        });
    }
    Ok(out)
}

/// Page views over time.
pub fn timeseries(conn: &Connection, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
    let filters = Filters::new(query);

    // Bucket by hour for short windows, by day for long ones.
    let granularity = if (query.range.end - query.range.start).num_seconds() <= 2 * 24 * 3600 {
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
         ORDER BY bucket ASC",
        where_clause = filters.clause(),
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(filters.params().as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        out.push(BreakdownRow {
            name: name.clone(),
            value: row.get(1)?,
            url: None,
            date: Some(name),
        });
    }
    Ok(out)
}

/// The custom events, grouped by name.
pub fn events(conn: &Connection, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
    let mut filters = Filters::new(query);
    filters.clauses.push("name IS NOT NULL".to_string());

    let sql = format!(
        "SELECT COALESCE(name, 'unknown') AS name, COUNT(*) AS value
         FROM event
         WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC
         LIMIT {limit}",
        where_clause = filters.clause(),
        limit = query.limit.clamp(1, 1000),
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(filters.params().as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(BreakdownRow {
            name: row.get(0)?,
            value: row.get(1)?,
            url: None,
            date: None,
        });
    }
    Ok(out)
}

/// The raw event log.
pub fn event_log(conn: &Connection, query: &ReportQuery) -> Result<Vec<EventRow>> {
    let filters = Filters::new(query);
    let sql = format!(
        "SELECT id, website_id, session_id, created_at, name, url, hostname, data
         FROM event
         WHERE {where_clause}
         ORDER BY created_at DESC
         LIMIT {limit} OFFSET {offset}",
        where_clause = filters.clause(),
        limit = query.limit.clamp(1, 1000),
        offset = query.offset.max(0),
    );

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
}

/// The session log.
pub fn sessions(conn: &Connection, query: &ReportQuery) -> Result<Vec<SessionRow>> {
    let filters = Filters::new(query);
    let sql = format!(
        "SELECT id, website_id, created_at, updated_at, hostname, url, title, referrer, entry_url,
                browser, os, device, country, region, city, language, screen, views, duration
         FROM session
         WHERE {where_clause}
         ORDER BY created_at DESC
         LIMIT {limit} OFFSET {offset}",
        where_clause = filters.clause(),
        limit = query.limit.clamp(1, 1000),
        offset = query.offset.max(0),
    );

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
}
