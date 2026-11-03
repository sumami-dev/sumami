//! Where the CLI gets its data: a local DuckDB file, or a running Sumami server.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use duckdb::{AccessMode, Config, Connection};
use serde_json::{json, Value};

/// A time window.
#[derive(Debug, Clone, Copy)]
pub struct Range {
    /// Start of the window (inclusive).
    pub start: chrono::NaiveDateTime,
    /// End of the window (exclusive).
    pub end: chrono::NaiveDateTime,
}

impl Range {
    /// The default window: the last 7 days.
    pub fn default_window() -> Self {
        let end = chrono::Utc::now().naive_utc();
        Range {
            start: end - chrono::Duration::days(7),
            end,
        }
    }

    /// Unix milliseconds, for the HTTP API.
    pub fn as_millis(&self) -> (i64, i64) {
        let to_ms = |t: chrono::NaiveDateTime| {
            t.and_utc().timestamp_millis()
        };
        (to_ms(self.start), to_ms(self.end))
    }
}

/// Parse a time expression.
///
/// Accepts `7d`, `24h`, `30m`, a date (`2026-01-01`) or an RFC 3339 timestamp.
pub fn parse_time(value: &str) -> Result<chrono::NaiveDateTime> {
    let value = value.trim();
    let now = chrono::Utc::now().naive_utc();

    if let Some(number) = value.strip_suffix('d') {
        let days: i64 = number.parse().context("invalid number of days")?;
        return Ok(now - chrono::Duration::days(days));
    }
    if let Some(number) = value.strip_suffix('h') {
        let hours: i64 = number.parse().context("invalid number of hours")?;
        return Ok(now - chrono::Duration::hours(hours));
    }
    if let Some(number) = value.strip_suffix('m') {
        let minutes: i64 = number.parse().context("invalid number of minutes")?;
        return Ok(now - chrono::Duration::minutes(minutes));
    }
    if value == "now" {
        return Ok(now);
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).unwrap_or(now));
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(date.naive_utc());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }

    Err(anyhow!(
        "cannot understand the time '{value}' (try 7d, 24h, 2026-01-01 or an RFC 3339 timestamp)"
    ))
}

/// A report the CLI can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// The list of websites.
    Websites,
    /// The summary metrics.
    Summary,
    /// The top pages.
    Pages,
    /// The traffic sources.
    Referrers,
    /// The browser breakdown.
    Browsers,
    /// The operating system breakdown.
    Os,
    /// The device breakdown.
    Devices,
    /// The country breakdown.
    Countries,
    /// The language breakdown.
    Languages,
    /// The screen size breakdown.
    Screens,
    /// Custom events.
    Events,
    /// The session log.
    Sessions,
    /// The time series.
    Timeseries,
}

/// Where the data comes from.
pub enum Source {
    /// A local DuckDB file.
    Local {
        /// The connection.
        conn: Connection,
        /// The path, for messages.
        path: PathBuf,
    },
    /// A running Sumami server.
    Remote {
        /// The base URL, without a trailing slash.
        base: String,
        /// The bearer token.
        token: String,
    },
}

impl Source {
    /// Open a local database file (read-only).
    ///
    /// DuckDB takes an exclusive lock on the file, so a database that a running server
    /// holds cannot be opened directly. In that case we work on a snapshot copy, which
    /// is exactly what an offline analysis wants.
    pub fn local(path: &Path) -> Result<Self> {
        match Self::open_local(path, AccessMode::ReadOnly) {
            Ok(conn) => Ok(Source::Local {
                conn,
                path: path.to_path_buf(),
            }),
            Err(err) => {
                let snapshot = Self::snapshot(path)?;
                let conn = Self::open_local(&snapshot, AccessMode::ReadOnly)?;
                eprintln!(
                    "[sumami] the database is in use by a running server ({err}); \
                     analysing a snapshot copy instead"
                );
                Ok(Source::Local {
                    conn,
                    path: snapshot,
                })
            }
        }
    }

    /// Open a local database file for writing.
    pub fn local_writable(path: &Path) -> Result<Self> {
        let conn = Self::open_local(path, AccessMode::ReadWrite)?;
        Ok(Source::Local {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Open a connection with the given access mode.
    fn open_local(path: &Path, mode: AccessMode) -> Result<Connection> {
        let config = Config::default().access_mode(mode)?;
        Connection::open_with_flags(path, config)
            .with_context(|| format!("opening {}", path.display()))
    }

    /// Copy the database (and its WAL) to a temporary directory.
    fn snapshot(path: &Path) -> Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!("sumami-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let target = dir.join("sumami.duckdb");
        let _ = std::fs::remove_file(&target);
        std::fs::copy(path, &target)
            .with_context(|| format!("copying {} to {}", path.display(), target.display()))?;
        // The write-ahead log holds the most recent commits.
        let wal = path.with_extension("duckdb.wal");
        if wal.exists() {
            let _ = std::fs::copy(&wal, target.with_extension("duckdb.wal"));
        }
        Ok(target)
    }

    /// Log in to a remote server and return a source.
    pub fn remote(
        base: &str,
        token: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Self> {
        let base = base.trim_end_matches('/').to_string();

        if let Some(token) = token {
            return Ok(Source::Remote {
                base,
                token: token.to_string(),
            });
        }

        let (username, password) = match (username, password) {
            (Some(u), Some(p)) => (u.to_string(), p.to_string()),
            _ => {
                let stored = stored_credentials()?;
                match stored {
                    Some((u, p)) => (u, p),
                    None => {
                        return Err(anyhow!(
                            "no credentials: pass --username/--password, or --token, \
                             or log in once with `sumami login`"
                        ))
                    }
                }
            }
        };

        let token = login(&base, &username, &password)?;
        Ok(Source::Remote { base, token })
    }

    /// Run a report and return the result as JSON.
    pub fn report(&self, report: Report, website: Option<&str>, range: Range, limit: i64) -> Result<Value> {
        match self {
            Source::Local { conn, .. } => local_report(conn, report, website, range, limit),
            Source::Remote { base, token } => {
                remote_report(base, token, report, website, range, limit)
            }
        }
    }

    /// Delete data that is older than the retention window of its website.
    ///
    /// Websites with `retention_days = 0` keep their data forever and are skipped.
    pub fn prune(&self, website: Option<&str>) -> Result<(u64, u64)> {
        match self {
            Source::Local { conn, .. } => {
                let mut sessions = 0u64;
                let mut events = 0u64;
                let now = chrono::Utc::now().naive_utc();

                // Collect the (website, cutoff) pairs first: the deletion itself must not
                // run while the cursor is open on the same table.
                let mut cutoffs: Vec<(String, chrono::NaiveDateTime)> = Vec::new();
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
            Source::Remote { .. } => Err(anyhow!(
                "pruning over HTTP is not supported in this version; \
                 stop the server and run `sumami prune --db <path>`"
            )),
        }
    }

    /// Erase a website and all of its data.
    pub fn erase(&self, website: &str) -> Result<u64> {
        match self {
            Source::Local { conn, .. } => {
                let events = conn.execute("DELETE FROM event WHERE website_id = ?", [website])?;
                let sessions = conn.execute("DELETE FROM session WHERE website_id = ?", [website])?;
                let websites = conn.execute("DELETE FROM website WHERE id = ?", [website])?;
                Ok((events + sessions + websites) as u64)
            }
            Source::Remote { .. } => Err(anyhow!(
                "erasing over HTTP is not supported in this version; \
                 run `sumami erase --db <path> <website> against a stopped server"
            )),
        }
    }
}

/// Read the stored credentials.
fn stored_credentials() -> Result<Option<(String, String)>> {
    let path = credentials_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let value: Value = serde_json::from_str(&content)?;
    let username = value.get("username").and_then(|v| v.as_str()).map(str::to_string);
    let password = value.get("password").and_then(|v| v.as_str()).map(str::to_string);
    Ok(username.zip(password))
}

/// Where the credentials are stored.
pub fn credentials_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("sumami").join("credentials.json")
}

/// Store the credentials (mode 0600).
pub fn store_credentials(username: &str, password: &str) -> Result<PathBuf> {
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({ "username": username, "password": password }))?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// Log in and return a token.
fn login(base: &str, username: &str, password: &str) -> Result<String> {
    let agent = ureq::Agent::new_with_defaults();
    let mut response = agent
        .post(&format!("{base}/api/login"))
        .send_json(json!({ "username": username, "password": password }))
        .map_err(|e| anyhow!("login failed: {e}"))?;

    let body: Value = response
        .body_mut()
        .read_json()
        .map_err(|e| anyhow!("reading the login response: {e}"))?;

    body.get("token")
        .and_then(|t| t.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("the server did not return a token"))
}

/// Run a report against the HTTP API.
fn remote_report(
    base: &str,
    token: &str,
    report: Report,
    website: Option<&str>,
    range: Range,
    limit: i64,
) -> Result<Value> {
    let agent = ureq::Agent::new_with_defaults();
    let (start, end) = range.as_millis();

    let path = match report {
        Report::Websites => "/api/websites".to_string(),
        _ => {
            let website = website.ok_or_else(|| anyhow!("this report needs a website id"))?;
            let suffix = match report {
                Report::Summary => "/stats",
                Report::Pages => "/stats/pages",
                Report::Referrers => "/stats/referrers",
                Report::Browsers => "/stats/browsers",
                Report::Os => "/stats/os",
                Report::Devices => "/stats/devices",
                Report::Countries => "/stats/countries",
                Report::Languages => "/stats/languages",
                Report::Screens => "/stats/screens",
                Report::Events => "/stats/events",
                Report::Sessions => "/sessions",
                Report::Timeseries => "/stats/timeseries",
                Report::Websites => unreachable!(),
            };
            format!("/api/websites/{website}{suffix}")
        }
    };

    let url = format!("{base}{path}?startAt={start}&endAt={end}&limit={limit}");
    let mut response = agent
        .get(&url)
        .header("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| anyhow!("request failed: {e}"))?;

    response
        .body_mut()
        .read_json()
        .map_err(|e| anyhow!("reading the response: {e}"))
}

/// Run a report against a local database.
///
/// This calls the **same** functions the HTTP API calls
/// (`sumami_service::reports`), so the local mode and the API can never disagree.
fn local_report(
    conn: &Connection,
    report: Report,
    website: Option<&str>,
    range: Range,
    limit: i64,
) -> Result<Value> {
    use sumami_service::reports::{self, Dimension, ReportQuery};

    // Only the reports about one website need its id.
    let website_id = if report == Report::Websites {
        String::new()
    } else {
        website
            .map(str::to_string)
            .ok_or_else(|| anyhow!("this report needs a website id"))?
    };

    let mut query = ReportQuery::new(&website_id, reports::TimeRange {
        start: range.start,
        end: range.end,
    });
    query.limit = limit.clamp(1, 1000);

    let value = match report {
        Report::Websites => serde_json::to_value(reports::websites(conn)?)?,
        Report::Summary => serde_json::to_value(reports::summary_with_change(conn, &query)?)?,
        Report::Timeseries => serde_json::to_value(reports::timeseries(conn, &query)?)?,
        Report::Pages => {
            serde_json::to_value(reports::breakdown(conn, &query, Dimension::Url)?)?
        }
        Report::Referrers => {
            let mut rows = reports::breakdown(conn, &query, Dimension::Referrer)?;
            for row in &mut rows {
                if row.name == "unknown" {
                    row.name = "(direct)".to_string();
                }
            }
            serde_json::to_value(rows)?
        }
        Report::Browsers => {
            serde_json::to_value(reports::breakdown(conn, &query, Dimension::Browser)?)?
        }
        Report::Os => serde_json::to_value(reports::breakdown(conn, &query, Dimension::Os)?)?,
        Report::Devices => {
            serde_json::to_value(reports::breakdown(conn, &query, Dimension::Device)?)?
        }
        Report::Countries => {
            serde_json::to_value(reports::breakdown(conn, &query, Dimension::Country)?)?
        }
        Report::Languages => {
            serde_json::to_value(reports::breakdown(conn, &query, Dimension::Language)?)?
        }
        Report::Screens => {
            serde_json::to_value(reports::breakdown(conn, &query, Dimension::Screen)?)?
        }
        Report::Events => serde_json::to_value(reports::events(conn, &query)?)?,
        Report::Sessions => serde_json::to_value(reports::sessions(conn, &query)?)?,
    };

    Ok(value)
}
