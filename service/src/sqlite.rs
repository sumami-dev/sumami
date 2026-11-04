//! The SQLite backend — the alternative to DuckDB.
//!
//! SQLite is a row store: it stores more bytes than DuckDB for the same data, but it
//! is a single file with no server, it allows many readers next to one writer, and it
//! is available everywhere. For teams that already run everything on SQLite — or that
//! want a backend with no surprises — this is the backend to use.
//!
//! The **same** [`Storage`](crate::storage::Storage) trait as DuckDB, the same
//! reports, the same numbers: the conformance suite runs against both.
//!
//! **One fact table**, exactly like the DuckDB backend: every page view is one row
//! in `event`, and the sessions and visitors are derived from it. Timestamps are
//! stored as **epoch seconds** (INTEGER), which is SQLite's natural representation
//! and makes range filters a plain integer comparison.
//!
//! The SQL is SQLite's dialect on purpose — bucketing, distinct counting and the
//! "the row of the newest hit" lookups all differ between engines.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{
    BreakdownRow, EventRow, Hit, MetricsSummary, MetricValue, Revenue, SessionRow, Website,
};
use crate::storage::{
    Dimension, ReportQuery, Storage, StorageStats, SummaryMetrics, UserRecord, WebsitePatch,
};

/// The SQLite backend.
pub struct SqliteBackend {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl SqliteBackend {
    /// Open (or create) a database and make sure the schema exists.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        Self::prepare(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        })
    }

    /// Open an in-memory database (tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        })
    }

    /// The pragmas and the schema.
    fn prepare(conn: &Connection) -> Result<()> {
        // WAL lets readers work while the writer is busy — the opposite of DuckDB,
        // which takes an exclusive lock on the file.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5_000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        conn.execute_batch(
            r#"
-- One row per page view (or custom event). The field set mirrors Umami's
-- `website_event` row for row. No primary key beyond the row id: this table is
-- append only and is only ever read as a filtered scan.
CREATE TABLE IF NOT EXISTS event (
    id              INTEGER PRIMARY KEY,
    website_id      TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    visitor_id      TEXT NOT NULL,
    created_at      INTEGER NOT NULL,

    url             TEXT,
    url_query       TEXT,
    title           TEXT,

    referrer        TEXT,
    referrer_path   TEXT,
    referrer_query  TEXT,
    hostname        TEXT,

    browser         TEXT,
    browser_version TEXT,
    os              TEXT,
    os_version      TEXT,
    device          TEXT,
    screen          TEXT,
    language        TEXT,
    country         TEXT,
    region          TEXT,
    city            TEXT,

    utm_source      TEXT,
    utm_medium      TEXT,
    utm_campaign    TEXT,
    utm_content     TEXT,
    utm_term        TEXT,

    gclid           TEXT,
    fbclid          TEXT,
    msclkid         TEXT,
    ttclid          TEXT,
    lifatid         TEXT,
    twclid          TEXT,

    lcp             REAL,
    inp             REAL,
    cls             REAL,
    fcp             REAL,
    ttfb            REAL,

    event_type      TEXT,
    event_name      TEXT,
    tag             TEXT,
    data            TEXT
);
CREATE INDEX IF NOT EXISTS event_website_created ON event (website_id, created_at);

CREATE TABLE IF NOT EXISTS revenue (
    id          INTEGER PRIMARY KEY,
    website_id  TEXT NOT NULL,
    session_id  TEXT,
    event_id    INTEGER,
    event_name  TEXT,
    currency    TEXT NOT NULL DEFAULT 'USD',
    revenue     REAL NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS revenue_website_created ON revenue (website_id, created_at);

CREATE TABLE IF NOT EXISTS website (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    domain          TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    retention_days  INTEGER NOT NULL DEFAULT 0,
    country_header  TEXT
);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS "user" (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    role          TEXT NOT NULL DEFAULT 'admin'
);
"#,
        )?;
        Ok(())
    }

    /// Run `f` with the connection.
    pub fn with_connection<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut conn)
    }

    /// Run `f` with the connection.
    fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        self.with_connection(f)
    }
}

/// Epoch seconds of a timestamp.
fn ts(value: chrono::NaiveDateTime) -> i64 {
    value.and_utc().timestamp()
}

/// A timestamp from epoch seconds.
fn from_ts(value: i64) -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp(value, 0)
        .map(|d| d.naive_utc())
        .unwrap_or_default()
}

/// Now, as a naive UTC timestamp.
fn crate_now() -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc()
}

/// The WHERE clause and its parameters.
struct Filters {
    clauses: Vec<String>,
    values: Vec<rusqlite::types::Value>,
}

impl Filters {
    fn new(query: &ReportQuery) -> Self {
        let mut filters = Filters {
            clauses: vec!["website_id = ?".to_string()],
            values: vec![query.website_id.clone().into()],
        };
        filters.push_value("created_at >= ?", ts(query.range.start));
        filters.push_value("created_at < ?", ts(query.range.end));

        for (column, value) in [
            ("url", &query.dimensions.url),
            ("referrer", &query.dimensions.referrer),
            ("browser", &query.dimensions.browser),
            ("os", &query.dimensions.os),
            ("device", &query.dimensions.device),
            ("country", &query.dimensions.country),
        ] {
            if let Some(value) = value.as_ref().filter(|v| !v.is_empty()) {
                filters.clauses.push(format!("{column} = ?"));
                filters.values.push(value.clone().into());
            }
        }

        filters
    }

    fn push_value(&mut self, clause: &str, value: i64) {
        self.clauses.push(clause.to_string());
        self.values.push(value.into());
    }

    fn clause(&self) -> String {
        self.clauses.join(" AND ")
    }

    fn params(&self) -> Vec<&dyn rusqlite::ToSql> {
        self.values
            .iter()
            .map(|v| v as &dyn rusqlite::ToSql)
            .collect()
    }
}

impl Storage for SqliteBackend {
    fn backend(&self) -> &'static str {
        "sqlite"
    }

    fn websites(&self) -> Result<Vec<Website>> {
        self.with(|conn| {
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
                    created_at: from_ts(row.get(3)?),
                    updated_at: from_ts(row.get(4)?),
                    retention_days: row.get(5)?,
                    country_header: row.get(6)?,
                });
            }
            Ok(out)
        })
    }

    fn website(&self, id: &str) -> Result<Option<Website>> {
        self.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, name, domain, created_at, updated_at, retention_days, country_header
                     FROM website WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok(Website {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            domain: row.get(2)?,
                            created_at: from_ts(row.get(3)?),
                            updated_at: from_ts(row.get(4)?),
                            retention_days: row.get(5)?,
                            country_header: row.get(6)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    fn create_website(
        &self,
        name: &str,
        domain: Option<&str>,
        retention_days: i32,
        country_header: Option<&str>,
    ) -> Result<Website> {
        let now = crate_now();
        let website = Website {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            domain: domain.map(|d| d.to_string()),
            created_at: now,
            updated_at: now,
            retention_days,
            country_header: country_header.map(|c| c.to_string()),
        };
        self.with(|conn| {
            conn.execute(
                "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    website.id,
                    website.name,
                    website.domain,
                    ts(website.created_at),
                    ts(website.updated_at),
                    website.retention_days,
                    website.country_header
                ],
            )?;
            Ok(())
        })?;
        Ok(website)
    }

    fn update_website(&self, id: &str, patch: &WebsitePatch) -> Result<Website> {
        self.with(|conn| {
            let mut website = conn
                .query_row(
                    "SELECT id, name, domain, created_at, updated_at, retention_days, country_header
                     FROM website WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok(Website {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            domain: row.get(2)?,
                            created_at: from_ts(row.get(3)?),
                            updated_at: from_ts(row.get(4)?),
                            retention_days: row.get(5)?,
                            country_header: row.get(6)?,
                        })
                    },
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("no such website: {id}"))?;

            if let Some(name) = patch.name.as_ref() {
                website.name = name.clone();
            }
            if let Some(domain) = patch.domain.as_ref() {
                website.domain = Some(domain.clone());
            }
            if let Some(retention_days) = patch.retention_days {
                website.retention_days = retention_days;
            }
            if let Some(country_header) = patch.country_header.as_ref() {
                website.country_header = Some(country_header.clone());
            }
            website.updated_at = crate_now();

            conn.execute(
                "UPDATE website SET name = ?2, domain = ?3, updated_at = ?4,
                        retention_days = ?5, country_header = ?6
                 WHERE id = ?1",
                params![
                    website.id,
                    website.name,
                    website.domain,
                    ts(website.updated_at),
                    website.retention_days,
                    website.country_header
                ],
            )?;
            Ok(website)
        })
    }

    fn delete_website(&self, id: &str) -> Result<bool> {
        self.with(|conn| {
            // The event table is the only fact table: the sessions and visitors are
            // derived from it, so deleting the events deletes them too.
            conn.execute("DELETE FROM event WHERE website_id = ?1", params![id])?;
            conn.execute("DELETE FROM revenue WHERE website_id = ?1", params![id])?;
            let removed = conn.execute("DELETE FROM website WHERE id = ?1", params![id])?;
            Ok(removed > 0)
        })
    }

    fn summary(&self, query: &ReportQuery) -> Result<SummaryMetrics> {
        self.with(|conn| summary(conn, query))
    }

    fn summary_with_change(&self, query: &ReportQuery) -> Result<MetricsSummary> {
        let current = self.summary(query)?;
        let mut previous_query = query.clone();
        previous_query.range = query.range.previous();
        let previous = self.summary(&previous_query)?;

        Ok(MetricsSummary {
            pageviews: metric(current.pageviews, previous.pageviews as f64),
            visitors: metric(current.visitors, previous.visitors as f64),
            views: metric(current.views, previous.views as f64),
            bounces: metric(current.bounces, previous.bounces as f64),
            visit_duration: metric(current.visit_duration as i64, previous.visit_duration),
        })
    }

    fn breakdown(&self, query: &ReportQuery, dimension: Dimension) -> Result<Vec<BreakdownRow>> {
        self.with(|conn| breakdown(conn, query, dimension))
    }

    fn timeseries(&self, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
        self.with(|conn| timeseries(conn, query))
    }

    fn events(&self, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
        self.with(|conn| events(conn, query))
    }

    fn event_log(&self, query: &ReportQuery) -> Result<Vec<EventRow>> {
        self.with(|conn| event_log(conn, query))
    }

    fn sessions(&self, query: &ReportQuery) -> Result<Vec<SessionRow>> {
        self.with(|conn| sessions(conn, query))
    }

    fn write_batch(&self, hits: &[Hit]) -> Result<()> {
        let now = crate_now();
        let kept: Vec<&Hit> = hits
            .iter()
            .filter(|hit| {
                if hit.retention_days > 0 {
                    let cutoff = now - chrono::Duration::days(hit.retention_days as i64);
                    hit.created_at >= cutoff
                } else {
                    true
                }
            })
            .collect();

        self.with(|conn| {
            let tx = conn.transaction()?;

            // Prepare once: re-preparing per row dominated the write path.
            let mut insert = tx.prepare_cached(
                r#"
INSERT INTO event (
    id, website_id, session_id, visitor_id, created_at,
    url, url_query, title,
    referrer, referrer_path, referrer_query, hostname,
    browser, browser_version, os, os_version, device, screen, language,
    country, region, city,
    utm_source, utm_medium, utm_campaign, utm_content, utm_term,
    gclid, fbclid, msclkid, ttclid, lifatid, twclid,
    lcp, inp, cls, fcp, ttfb,
    event_type, event_name, tag, data
) VALUES (
    NULL, ?1, ?2, ?3, ?4,
    ?5, ?6, ?7,
    ?8, ?9, ?10, ?11,
    ?12, ?13, ?14, ?15, ?16, ?17, ?18,
    ?19, ?20, ?21,
    ?22, ?23, ?24, ?25, ?26,
    ?27, ?28, ?29, ?30, ?31, ?32,
    ?33, ?34, ?35, ?36, ?37,
    ?38, ?39, ?40, ?41
)"#,
            )?;
            for hit in &kept {
                insert.execute(params![
                    hit.website_id,
                    hit.session_id,
                    hit.visitor_id,
                    ts(hit.created_at),
                    hit.url,
                    hit.url_query,
                    hit.title,
                    hit.referrer,
                    hit.referrer_path,
                    hit.referrer_query,
                    hit.hostname,
                    hit.browser,
                    hit.browser_version,
                    hit.os,
                    hit.os_version,
                    hit.device,
                    hit.screen,
                    hit.language,
                    hit.country,
                    hit.region,
                    hit.city,
                    hit.utm_source,
                    hit.utm_medium,
                    hit.utm_campaign,
                    hit.utm_content,
                    hit.utm_term,
                    hit.gclid,
                    hit.fbclid,
                    hit.msclkid,
                    hit.ttclid,
                    hit.lifatid,
                    hit.twclid,
                    hit.lcp,
                    hit.inp,
                    hit.cls,
                    hit.fcp,
                    hit.ttfb,
                    hit.kind.as_str(),
                    hit.name,
                    hit.tag,
                    hit.data,
                ])?;
            }
            drop(insert);

            tx.commit()?;
            Ok(())
        })
    }

    fn write_revenue(&self, records: &[Revenue]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        self.with(|conn| {
            let tx = conn.transaction()?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO revenue (id, website_id, session_id, event_id, event_name, currency, revenue, created_at)
                 VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for record in records {
                insert.execute(params![
                    record.website_id,
                    record.session_id,
                    record.event_id,
                    record.event_name,
                    record.currency,
                    record.revenue,
                    ts(record.created_at),
                ])?;
            }
            drop(insert);
            tx.commit()?;
            Ok(())
        })
    }

    fn revenue(&self, query: &ReportQuery) -> Result<Vec<Revenue>> {
        self.with(|conn| revenue(conn, query))
    }

    fn prune(&self, website: Option<&str>) -> Result<(u64, u64)> {
        self.with(|conn| {
            let now = crate_now();
            let mut stmt = conn.prepare(
                "SELECT id, retention_days FROM website WHERE (?1 IS NULL OR id = ?1)",
            )?;
            let rows = stmt
                .query_map(params![website], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);

            let mut removed = 0u64;
            for (id, retention_days) in rows {
                if retention_days <= 0 {
                    continue;
                }
                let cutoff = now - chrono::Duration::days(retention_days);
                removed += conn.execute(
                    "DELETE FROM event WHERE website_id = ?1 AND created_at < ?2",
                    params![id, ts(cutoff)],
                )? as u64;
                removed += conn.execute(
                    "DELETE FROM revenue WHERE website_id = ?1 AND created_at < ?2",
                    params![id, ts(cutoff)],
                )? as u64;
            }
            Ok((removed, 0))
        })
    }

    fn delete_before(
        &self,
        before: chrono::NaiveDateTime,
        website: Option<&str>,
    ) -> Result<(u64, u64)> {
        self.with(|conn| {
            let mut removed = 0u64;
            if let Some(website) = website {
                removed += conn.execute(
                    "DELETE FROM event WHERE website_id = ?1 AND created_at < ?2",
                    params![website, ts(before)],
                )? as u64;
                removed += conn.execute(
                    "DELETE FROM revenue WHERE website_id = ?1 AND created_at < ?2",
                    params![website, ts(before)],
                )? as u64;
            } else {
                removed += conn.execute(
                    "DELETE FROM event WHERE created_at < ?1",
                    params![ts(before)],
                )? as u64;
                removed += conn.execute(
                    "DELETE FROM revenue WHERE created_at < ?1",
                    params![ts(before)],
                )? as u64;
            }
            Ok((removed, 0))
        })
    }

    fn erase(&self, website: &str) -> Result<u64> {
        self.with(|conn| {
            let mut removed = 0u64;
            removed += conn.execute("DELETE FROM revenue WHERE website_id = ?1", params![website])? as u64;
            removed += conn.execute("DELETE FROM event WHERE website_id = ?1", params![website])? as u64;
            removed += conn.execute("DELETE FROM website WHERE id = ?1", params![website])? as u64;
            Ok(removed)
        })
    }

    fn user(&self, username: &str) -> Result<Option<UserRecord>> {
        self.with(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, username, role, created_at, password_hash FROM \"user\" WHERE username = ?1",
                    params![username],
                    |row| {
                        Ok(UserRecord {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            role: row.get(2)?,
                            created_at: from_ts(row.get(3)?),
                            password_hash: row.get(4)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    fn set_user_password(&self, username: &str, password_hash: &str) -> Result<UserRecord> {
        self.with(|conn| {
            // The account is created on the first call — that is how the admin user
            // comes to exist — and the password is replaced afterwards.
            let existing: Option<String> = {
                let mut stmt =
                    conn.prepare("SELECT id FROM \"user\" WHERE username = ?1")?;
                let mut rows = stmt.query(params![username])?;
                match rows.next()? {
                    Some(row) => Some(row.get(0)?),
                    None => None,
                }
            };

            let id = match existing {
                Some(id) => {
                    conn.execute(
                        "UPDATE \"user\" SET password_hash = ?2 WHERE username = ?1",
                        params![username, password_hash],
                    )?;
                    id
                }
                None => {
                    let id = uuid::Uuid::new_v4().to_string();
                    conn.execute(
                        "INSERT INTO \"user\" (id, username, password_hash, created_at, role)
                         VALUES (?1, ?2, ?3, ?4, 'admin')",
                        params![id, username, password_hash, ts(crate_now())],
                    )?;
                    id
                }
            };

            conn.query_row(
                "SELECT id, username, role, created_at, password_hash FROM \"user\" WHERE username = ?1",
                params![username],
                |row| {
                    Ok(UserRecord {
                        id: row.get(0)?,
                        username: row.get(1)?,
                        role: row.get(2)?,
                        created_at: from_ts(row.get(3)?),
                        password_hash: row.get(4)?,
                    })
                },
            )
            .map_err(Into::into)
        })
    }

    fn stats(&self) -> Result<StorageStats> {
        let websites = self.with(|conn| {
            let mut stmt = conn.prepare("SELECT count(*) FROM website")?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(row.get::<_, i64>(0)?),
                None => Ok(0),
            }
        })?;

        let size_bytes = std::fs::metadata(&self.path)
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        Ok(StorageStats {
            backend: "sqlite".to_string(),
            memory_bytes: 0,
            memory_limit: "0B".to_string(),
            threads: 1,
            size_bytes,
            websites,
        })
    }
}

// --------------------------------------------------------------------------------------
// The reports
// --------------------------------------------------------------------------------------

/// Compute the summary metrics for a window.
fn summary(conn: &Connection, query: &ReportQuery) -> Result<SummaryMetrics> {
    let filters = Filters::new(query);

    let sql = format!(
        "SELECT COUNT(*), COUNT(DISTINCT visitor_id), COUNT(DISTINCT session_id)
         FROM event WHERE {}",
        filters.clause()
    );
    let mut stmt = conn.prepare(&sql)?;
    let (pageviews, visitors, views) = match stmt.query(filters.params().as_slice())?.next()? {
        Some(row) => (row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        None => (0, 0, 0),
    };
    drop(stmt);

    // Bounces (sessions with a single view) and the average session length.
    let sql = format!(
        "SELECT COUNT(*) FILTER (WHERE n = 1),
                COALESCE(AVG(d), 0)
         FROM (
             SELECT session_id,
                    COUNT(*) AS n,
                    MAX(created_at) - MIN(created_at) AS d
             FROM event WHERE {}
             GROUP BY session_id
         )",
        filters.clause()
    );
    let mut stmt = conn.prepare(&sql)?;
    let (bounces, duration) = match stmt.query(filters.params().as_slice())?.next()? {
        Some(row) => (row.get::<_, i64>(0)?, row.get::<_, f64>(1)?),
        None => (0, 0.0),
    };
    drop(stmt);

    Ok(SummaryMetrics {
        pageviews,
        visitors,
        views,
        bounces,
        visit_duration: duration,
    })
}

/// Break a website down over one dimension.
fn breakdown(
    conn: &Connection,
    query: &ReportQuery,
    dimension: Dimension,
) -> Result<Vec<BreakdownRow>> {
    let filters = Filters::new(query);

    // Content dimensions count views, audience dimensions count visitors.
    let aggregate = match dimension.aggregate() {
        "views" => "COUNT(*)",
        _ => "COUNT(DISTINCT visitor_id)",
    };

    let sql = format!(
        "SELECT COALESCE({column}, 'unknown') AS name, {aggregate} AS value
         FROM event WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC, name ASC
         LIMIT {limit}",
        column = dimension.column(),
        aggregate = aggregate,
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

/// The page views over time.
fn timeseries(conn: &Connection, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
    let filters = Filters::new(query);

    // Bucket by hour for short windows, by day for long ones.
    let format = if (query.range.end - query.range.start).num_seconds() <= 2 * 24 * 3600 {
        "%Y-%m-%dT%H:00:00Z"
    } else {
        "%Y-%m-%dT00:00:00Z"
    };

    let sql = format!(
        "SELECT strftime('{format}', created_at, 'unixepoch') AS bucket, COUNT(*) AS value
         FROM event WHERE {where_clause}
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
fn events(conn: &Connection, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
    let filters = Filters::new(query);

    let sql = format!(
        "SELECT COALESCE(event_name, 'unknown') AS name, COUNT(*) AS value
         FROM event WHERE {where_clause} AND event_type = 'event'
         GROUP BY name
         ORDER BY value DESC, name ASC",
        where_clause = filters.clause(),
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
fn event_log(conn: &Connection, query: &ReportQuery) -> Result<Vec<EventRow>> {
    let filters = Filters::new(query);

    let sql = format!(
        "SELECT id, website_id, session_id, created_at, event_name, url, hostname, data,
                referrer, referrer_path, referrer_query, country, city, browser, os,
                utm_source, utm_medium, utm_campaign, lcp, inp, cls, device
         FROM event WHERE {where_clause}
         ORDER BY created_at DESC, id DESC
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
            id: row.get::<_, i64>(0)?.to_string(),
            website_id: row.get(1)?,
            session_id: row.get(2)?,
            created_at: from_ts(row.get(3)?),
            name: row.get(4)?,
            url: row.get(5)?,
            hostname: row.get(6)?,
            data: row.get(7)?,
            referrer: row.get(8)?,
            referrer_path: row.get(9)?,
            referrer_query: row.get(10)?,
            country: row.get(11)?,
            city: row.get(12)?,
            browser: row.get(13)?,
            os: row.get(14)?,
            device: row.get(21)?,
            utm_source: row.get(15)?,
            utm_medium: row.get(16)?,
            utm_campaign: row.get(17)?,
            lcp: row.get(18)?,
            inp: row.get(19)?,
            cls: row.get(20)?,
        });
    }
    Ok(out)
}

/// The session log: one row per session, derived from its page views.
fn sessions(conn: &Connection, query: &ReportQuery) -> Result<Vec<SessionRow>> {
    let filters = Filters::new(query);

    // The landing page and the last page are the first and the last hit of the
    // session — SQLite has no `arg_min`/`arg_max`, so they are correlated
    // lookups. The device fields are constant within a session, so `MAX` returns
    // the same value DuckDB's `any_value` does.
    let sql = format!(
        "SELECT session_id,
                MIN(created_at),
                MAX(created_at),
                COUNT(*),
                (SELECT e2.url FROM event e2
                  WHERE e2.session_id = e.session_id ORDER BY e2.created_at DESC LIMIT 1),
                (SELECT e3.url FROM event e3
                  WHERE e3.session_id = e.session_id ORDER BY e3.created_at ASC LIMIT 1),
                MAX(title),
                MAX(referrer),
                MAX(referrer_path),
                MAX(hostname),
                MAX(browser),
                MAX(os),
                MAX(device),
                MAX(country),
                MAX(region),
                MAX(city),
                MAX(language),
                MAX(screen),
                MAX(visitor_id),
                MAX(created_at) - MIN(created_at)
         FROM event e WHERE {where_clause}
         GROUP BY session_id
         ORDER BY MIN(created_at) DESC, session_id ASC
         LIMIT {limit} OFFSET {offset}",
        where_clause = filters.clause(),
        limit = query.limit.clamp(1, 1000),
        offset = query.offset.max(0),
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(filters.params().as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let created_at = from_ts(row.get(1)?);
        let updated_at = from_ts(row.get(2)?);
        let views: i64 = row.get(3)?;
        let duration: i64 = row.get(19).unwrap_or(0);
        out.push(SessionRow {
            id: row.get(0)?,
            website_id: query.website_id.clone(),
            created_at,
            updated_at,
            hostname: row.get(9)?,
            url: row.get(4)?,
            title: row.get(6)?,
            referrer: row.get(7)?,
            referrer_path: row.get(8)?,
            entry_url: row.get(5)?,
            browser: row.get(10)?,
            os: row.get(11)?,
            device: row.get(12)?,
            country: row.get(13)?,
            region: row.get(14)?,
            city: row.get(15)?,
            language: row.get(16)?,
            screen: row.get(17)?,
            views,
            duration,
        });
    }
    Ok(out)
}

/// The revenue of a website over a window.
fn revenue(conn: &Connection, query: &ReportQuery) -> Result<Vec<Revenue>> {
    let filters = Filters::new(query);

    let sql = format!(
        "SELECT id, website_id, session_id, event_id, event_name, currency, revenue, created_at
         FROM revenue WHERE {where_clause}
         ORDER BY created_at DESC, id DESC
         LIMIT {limit} OFFSET {offset}",
        where_clause = filters.clause(),
        limit = query.limit.clamp(1, 1000),
        offset = query.offset.max(0),
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(filters.params().as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(Revenue {
            website_id: row.get(1)?,
            session_id: row.get(2)?,
            event_id: row.get(3)?,
            event_name: row.get(4)?,
            currency: row.get(5)?,
            revenue: row.get(6)?,
            created_at: from_ts(row.get(7)?),
        });
    }
    Ok(out)
}

/// A metric with its period-over-period change.
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
