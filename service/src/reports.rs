//! The DuckDB backend.
//!
//! Every report and every write goes through here. The SQL is DuckDB-specific on
//! purpose — timestamps, date bucketing, distinct counting and upsert syntax all
//! differ between engines, so each backend implements the trait in its own dialect.
//!
//! The SQLite and PostgreSQL backends live in the private `sumami-pro` repository.

use anyhow::Result;
use chrono::NaiveDateTime;
use duckdb::types::Value;
use duckdb::Connection;

use crate::models::{BreakdownRow, EventRow, Hit, MetricsSummary, MetricValue, SessionRow, Website};
use crate::pool::Db;
use crate::storage::{
    Dimension, ReportQuery, Storage, StorageStats, SummaryMetrics, UserRecord, WebsitePatch,
};

impl Storage for Db {
    fn backend(&self) -> &'static str {
        "duckdb"
    }

    fn websites(&self) -> Result<Vec<Website>> {
        self.with_read(websites)
    }

    fn website(&self, id: &str) -> Result<Option<Website>> {
        self.with_read(|conn| website(conn, id))
    }

    fn create_website(
        &self,
        name: &str,
        domain: Option<&str>,
        retention_days: i32,
        country_header: Option<&str>,
    ) -> Result<Website> {
        self.with_write(|conn| create_website(conn, name, domain, retention_days, country_header))
    }

    fn update_website(&self, id: &str, patch: &WebsitePatch) -> Result<Website> {
        self.with_write(|conn| update_website(conn, id, patch))
    }

    fn delete_website(&self, id: &str) -> Result<bool> {
        self.with_write(|conn| {
            conn.execute("DELETE FROM event WHERE website_id = ?", [id])?;
            conn.execute("DELETE FROM session WHERE website_id = ?", [id])?;
            let removed = conn.execute("DELETE FROM website WHERE id = ?", [id])?;
            Ok(removed > 0)
        })
    }

    fn summary(&self, query: &ReportQuery) -> Result<SummaryMetrics> {
        self.with_read(|conn| summary(conn, query))
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
        self.with_read(|conn| breakdown(conn, query, dimension))
    }

    fn timeseries(&self, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
        self.with_read(|conn| timeseries(conn, query))
    }

    fn events(&self, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
        self.with_read(|conn| events(conn, query))
    }

    fn event_log(&self, query: &ReportQuery) -> Result<Vec<EventRow>> {
        self.with_read(|conn| event_log(conn, query))
    }

    fn sessions(&self, query: &ReportQuery) -> Result<Vec<SessionRow>> {
        self.with_read(|conn| sessions(conn, query))
    }

    fn write_batch(&self, hits: &[Hit]) -> Result<()> {
        self.with_write(|conn| write_batch(conn, hits))
    }

    fn prune(&self, website: Option<&str>) -> Result<(u64, u64)> {
        self.with_write(|conn| prune(conn, website))
    }

    fn delete_before(
        &self,
        before: chrono::NaiveDateTime,
        website: Option<&str>,
    ) -> Result<(u64, u64)> {
        self.with_write(|conn| delete_before(conn, before, website))
    }

    fn erase(&self, website: &str) -> Result<u64> {
        self.with_write(|conn| {
            let events = conn.execute("DELETE FROM event WHERE website_id = ?", [website])?;
            let sessions = conn.execute("DELETE FROM session WHERE website_id = ?", [website])?;
            let websites = conn.execute("DELETE FROM website WHERE id = ?", [website])?;
            Ok((events + sessions + websites) as u64)
        })
    }

    fn user(&self, username: &str) -> Result<Option<UserRecord>> {
        self.with_read(|conn| user(conn, username))
    }

    fn set_user_password(&self, username: &str, password_hash: &str) -> Result<UserRecord> {
        self.with_write(|conn| set_user_password(conn, username, password_hash))
    }

    fn stats(&self) -> Result<StorageStats> {
        let websites = self.with_read(|conn| {
            let mut stmt = conn.prepare("SELECT count(*) FROM website")?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(row.get::<_, i64>(0)?),
                None => Ok(0),
            }
        })?;

        let (memory_bytes, memory_limit, threads) = self.with_read(|conn| {
            let mut usage = 0i64;
            let mut stmt = conn.prepare("SELECT memory_usage_bytes FROM duckdb_memory()")?;
            let mut rows = stmt.query([])?;
            if let Some(row) = rows.next()? {
                usage = row.get(0).unwrap_or(0);
            }
            drop(rows);
            drop(stmt);

            let limit: String = conn
                .query_row(
                    "SELECT CAST(current_setting('memory_limit') AS VARCHAR)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| "unknown".to_string());
            let threads: i64 = conn
                .query_row("SELECT current_setting('threads')", [], |r| r.get(0))
                .unwrap_or(0);
            Ok((usage, limit, threads))
        })?;

        let size_bytes = std::fs::metadata(self.path()).map(|m| m.len() as i64).unwrap_or(0);

        Ok(StorageStats {
            backend: "duckdb".to_string(),
            memory_bytes,
            memory_limit,
            threads,
            size_bytes,
            websites,
        })
    }
}

// --------------------------------------------------------------------------------------
// Websites
// --------------------------------------------------------------------------------------

fn websites(conn: &Connection) -> Result<Vec<Website>> {
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

fn website(conn: &Connection, id: &str) -> Result<Option<Website>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, domain, created_at, updated_at, retention_days, country_header
         FROM website WHERE id = ?",
    )?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(row) => Ok(Some(Website {
            id: row.get(0)?,
            name: row.get(1)?,
            domain: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
            retention_days: row.get(5)?,
            country_header: row.get(6)?,
        })),
        None => Ok(None),
    }
}

fn create_website(
    conn: &Connection,
    name: &str,
    domain: Option<&str>,
    retention_days: i32,
    country_header: Option<&str>,
) -> Result<Website> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now();
    conn.execute(
        "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        duckdb::params![id, name, domain, now, now, retention_days, country_header],
    )?;
    Ok(Website {
        id,
        name: name.to_string(),
        domain: domain.map(str::to_string),
        created_at: now,
        updated_at: now,
        retention_days,
        country_header: country_header.map(str::to_string),
    })
}

fn update_website(conn: &Connection, id: &str, patch: &WebsitePatch) -> Result<Website> {
    let existing = website(conn, id)?
        .ok_or_else(|| anyhow::anyhow!("website not found: {id}"))?;

    let name = patch
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(existing.name);
    let domain = match &patch.domain {
        Some(value) => {
            let value = value.trim().to_lowercase();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        None => existing.domain.clone(),
    };
    let retention_days = patch.retention_days.unwrap_or(existing.retention_days);
    let country_header = patch.country_header.clone().or(existing.country_header.clone());
    let now = crate::db::now();

    conn.execute(
        "UPDATE website SET name = ?, domain = ?, retention_days = ?, country_header = ?, updated_at = ?
         WHERE id = ?",
        duckdb::params![name, domain, retention_days, country_header, now, id],
    )?;

    Ok(Website {
        id: id.to_string(),
        name,
        domain,
        created_at: existing.created_at,
        updated_at: now,
        retention_days,
        country_header,
    })
}

// --------------------------------------------------------------------------------------
// Users
// --------------------------------------------------------------------------------------

fn user(conn: &Connection, username: &str) -> Result<Option<UserRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, role, created_at, password_hash FROM \"user\" WHERE username = ?",
    )?;
    let mut rows = stmt.query([username])?;
    match rows.next()? {
        Some(row) => Ok(Some(UserRecord {
            id: row.get(0)?,
            username: row.get(1)?,
            role: row.get(2)?,
            created_at: row.get(3)?,
            password_hash: row.get(4)?,
        })),
        None => Ok(None),
    }
}

fn set_user_password(
    conn: &Connection,
    username: &str,
    password_hash: &str,
) -> Result<UserRecord> {
    let now = crate::db::now();
    let existing: Option<String> = {
        let mut stmt = conn.prepare("SELECT id FROM \"user\" WHERE username = ?")?;
        let mut rows = stmt.query([username])?;
        match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        }
    };

    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE \"user\" SET password_hash = ? WHERE username = ?",
                duckdb::params![password_hash, username],
            )?;
            Ok(UserRecord {
                id,
                username: username.to_string(),
                role: "admin".to_string(),
                created_at: now,
                password_hash: password_hash.to_string(),
            })
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO \"user\" (id, username, password_hash, created_at, role)
                 VALUES (?, ?, ?, ?, 'admin')",
                duckdb::params![id, username, password_hash, now],
            )?;
            Ok(UserRecord {
                id,
                username: username.to_string(),
                role: "admin".to_string(),
                created_at: now,
                password_hash: password_hash.to_string(),
            })
        }
    }
}

// --------------------------------------------------------------------------------------
// Reports
// --------------------------------------------------------------------------------------

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

/// Compute the summary metrics for a window.
fn summary(conn: &Connection, query: &ReportQuery) -> Result<SummaryMetrics> {
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
fn breakdown(
    conn: &Connection,
    query: &ReportQuery,
    dimension: Dimension,
) -> Result<Vec<BreakdownRow>> {
    let filters = Filters::new(query);
    let aggregate = match dimension.aggregate() {
        "views" => "COALESCE(SUM(views), 0)",
        _ => "COUNT(DISTINCT visitor_id)",
    };
    let sql = format!(
        "SELECT COALESCE({column}, 'unknown') AS name, {aggregate} AS value
         FROM session
         WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC, name ASC
         LIMIT {limit}",
        column = dimension.column(),
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
fn timeseries(conn: &Connection, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
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

/// Custom events, grouped by name.
fn events(conn: &Connection, query: &ReportQuery) -> Result<Vec<BreakdownRow>> {
    let mut filters = Filters::new(query);
    filters.clauses.push("name IS NOT NULL".to_string());

    let sql = format!(
        "SELECT COALESCE(name, 'unknown') AS name, COUNT(*) AS value
         FROM event
         WHERE {where_clause}
         GROUP BY name
         ORDER BY value DESC, name ASC
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
fn event_log(conn: &Connection, query: &ReportQuery) -> Result<Vec<EventRow>> {
    let filters = Filters::new(query);
    let sql = format!(
        "SELECT id, website_id, session_id, created_at, name, url, hostname, data
         FROM event
         WHERE {where_clause}
         ORDER BY created_at DESC, id ASC
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
fn sessions(conn: &Connection, query: &ReportQuery) -> Result<Vec<SessionRow>> {
    let filters = Filters::new(query);
    let sql = format!(
        "SELECT id, website_id, created_at, updated_at, hostname, url, title, referrer, entry_url,
                browser, os, device, country, region, city, language, screen, views, duration
         FROM session
         WHERE {where_clause}
         ORDER BY created_at DESC, id ASC
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

// --------------------------------------------------------------------------------------
// Writes
// --------------------------------------------------------------------------------------

/// Persist a batch of hits in one transaction.
fn write_batch(conn: &Connection, batch: &[Hit]) -> Result<()> {
    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<()> {
        let now = crate::db::now();

        // Retention: drop hits that are older than the website's window.
        let kept: Vec<Hit> = batch
            .iter()
            .filter(|hit| {
                if hit.retention_days > 0 {
                    let cutoff = now - chrono::Duration::days(hit.retention_days as i64);
                    hit.created_at >= cutoff
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        let grouped = crate::storage::aggregate_batch(&kept);

        for session in &grouped.sessions {
            upsert_session(conn, session)?;
        }

        for hit in grouped.events {
            insert_event(conn, hit)?;
        }

        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// Insert or update a session row.
fn upsert_session(
    conn: &Connection,
    s: &crate::storage::SessionAggregate,
) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO session (
    id, website_id, visitor_id, created_at, updated_at, hostname, url, title, referrer, entry_url,
    browser, browser_version, os, os_version, device, country, region, city, language, screen,
    data, views, duration
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
ON CONFLICT (id) DO UPDATE SET
    updated_at      = excluded.updated_at,
    url             = COALESCE(excluded.url, session.url),
    title           = COALESCE(excluded.title, session.title),
    referrer        = COALESCE(excluded.referrer, session.referrer),
    screen          = COALESCE(excluded.screen, session.screen),
    data            = COALESCE(excluded.data, session.data),
    views           = session.views + excluded.views,
    duration        = CAST(epoch(excluded.updated_at - session.created_at) AS INTEGER)
"#,
        duckdb::params![
            s.id,
            s.website_id,
            s.visitor_id,
            s.created_at,
            s.updated_at,
            s.hostname,
            s.url,
            s.title,
            s.referrer,
            s.entry_url,
            s.browser,
            s.browser_version,
            s.os,
            s.os_version,
            s.device,
            s.country,
            s.region,
            s.city,
            s.language,
            s.screen,
            s.data,
            s.views,
        ],
    )?;
    Ok(())
}

/// Insert a custom event row.
fn insert_event(conn: &Connection, hit: &Hit) -> Result<()> {
    conn.execute(
        "INSERT INTO event (id, website_id, session_id, created_at, name, url, hostname, data)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        duckdb::params![
            uuid::Uuid::new_v4().to_string(),
            hit.website_id,
            hit.session_id,
            hit.created_at,
            hit.name,
            hit.url,
            hit.hostname,
            hit.data,
        ],
    )?;
    Ok(())
}

/// Delete everything older than a moment.
fn delete_before(
    conn: &Connection,
    before: chrono::NaiveDateTime,
    website: Option<&str>,
) -> Result<(u64, u64)> {
    let mut sessions = 0u64;
    let mut events = 0u64;
    match website {
        Some(id) => {
            sessions += conn.execute(
                "DELETE FROM session WHERE website_id = ? AND created_at < ?",
                duckdb::params![id, before],
            )? as u64;
            events += conn.execute(
                "DELETE FROM event WHERE website_id = ? AND created_at < ?",
                duckdb::params![id, before],
            )? as u64;
        }
        None => {
            sessions += conn.execute(
                "DELETE FROM session WHERE created_at < ?",
                duckdb::params![before],
            )? as u64;
            events += conn.execute(
                "DELETE FROM event WHERE created_at < ?",
                duckdb::params![before],
            )? as u64;
        }
    }
    Ok((sessions, events))
}

/// Delete data that is older than the retention window of its website.
fn prune(conn: &Connection, website: Option<&str>) -> Result<(u64, u64)> {
    let mut sessions = 0u64;
    let mut events = 0u64;
    let now = crate::db::now();

    // Collect the (website, cutoff) pairs first: the deletion itself must not run
    // while a cursor is open on the same table.
    let mut cutoffs: Vec<(String, NaiveDateTime)> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, retention_days FROM website WHERE (? IS NULL OR id = ?)",
        )?;
        let mut rows = stmt.query(duckdb::params![website, website])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let days: i32 = row.get(1)?;
            if days > 0 {
                cutoffs.push((id, now - chrono::Duration::days(days as i64)));
            }
        }
    }

    for (id, cutoff) in &cutoffs {
        sessions += conn.execute(
            "DELETE FROM session WHERE website_id = ? AND created_at < ?",
            duckdb::params![id, *cutoff],
        )? as u64;
        events += conn.execute(
            "DELETE FROM event WHERE website_id = ? AND created_at < ?",
            duckdb::params![id, *cutoff],
        )? as u64;
    }

    Ok((sessions, events))
}
