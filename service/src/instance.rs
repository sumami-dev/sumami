//! The instance store — everything an instance keeps that is **not** analytics.
//!
//! The data plane (the events) is append-only and read as aggregations: a column
//! store is right for it. Everything else an instance holds is the opposite shape:
//!
//! | | |
//! | --- | --- |
//! | **accounts and logins** | small, mutated, needs a transaction and a unique constraint |
//! | **the log** | appended, but read by time and by actor |
//! | **the GSC import** | a bulk dump that must land in one transaction or not at all |
//! | **the query cache** | small, written and read constantly, expires |
//! | **query snapshots** | a saved report, edited and re-run |
//! | **the AI chat** | sessions and messages, appended and read in order |
//!
//! All of that belongs in a **row store with transactions** — SQLite — not in the
//! column store that holds the events. It also means the accounts live in their own
//! small file, so they survive an analytics rebuild and can be backed up in a
//! second.
//!
//! The private edition replaces this store with a PostgreSQL one; the trait is the
//! same, so the API layer does not know which it is talking to.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// The instance store — the small, transactional half of an instance.
///
/// The open-source build stores it in SQLite. The private edition can store it in
/// PostgreSQL instead; the trait is the same, so the API layer does not care.
pub trait Instance: Send + Sync {
    /// The account with this name.
    fn account(&self, username: &str) -> Result<Option<Account>>;
    /// Every account.
    fn accounts(&self) -> Result<Vec<Account>>;
    /// Create the account, or replace its password.
    fn set_password(&self, username: &str, password_hash: &str) -> Result<Account>;
    /// Record a sign-in attempt.
    fn record_login(
        &self,
        username: &str,
        ok: bool,
        ip: Option<&str>,
        agent: Option<&str>,
    ) -> Result<()>;
    /// The sign-in attempts, newest first.
    fn logins(&self, limit: i64) -> Result<Vec<LoginRecord>>;

    /// Append a line to the log.
    fn log(&self, entry: &LogEntry) -> Result<()>;
    /// The log, newest first.
    fn logs(&self, kind: Option<&str>, limit: i64) -> Result<Vec<LogEntry>>;

    /// Import a Search Console export — all of it, or none of it.
    fn import_gsc(
        &self,
        website_id: &str,
        rows: &[GscRow],
        source: Option<&str>,
    ) -> Result<ImportReport>;
    /// The imported Search Console rows, newest day first.
    fn gsc_rows(&self, website_id: &str, limit: i64) -> Result<Vec<GscRow>>;
    /// The imports of a website.
    fn gsc_imports(&self, website_id: &str) -> Result<Vec<ImportReport>>;

    /// A cached value, if it is still fresh.
    fn cache_get(&self, key: &str) -> Result<Option<String>>;
    /// Put a value in the cache.
    fn cache_put(&self, key: &str, value: &str, ttl: Option<i64>) -> Result<()>;
    /// Drop the expired entries.
    fn cache_purge(&self) -> Result<u64>;
    /// How many entries the cache holds.
    fn cache_len(&self) -> Result<usize>;

    /// Save a report under a name.
    fn save_snapshot(&self, name: &str, query: &str, result: &str) -> Result<Snapshot>;
    /// The saved reports.
    fn snapshots(&self) -> Result<Vec<Snapshot>>;
    /// Delete a saved report.
    fn delete_snapshot(&self, name: &str) -> Result<bool>;

    /// Start a conversation.
    fn chat_session(&self, website_id: Option<&str>, title: Option<&str>) -> Result<ChatSession>;
    /// Add a message to a conversation.
    fn chat_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        data: Option<&str>,
    ) -> Result<ChatMessage>;
    /// The conversations, newest first.
    fn chat_sessions(&self) -> Result<Vec<ChatSession>>;
    /// The messages of a conversation, oldest first.
    fn chat_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>>;

    /// The teams of this instance.
    fn teams(&self) -> Result<Vec<Team>>;
    /// One team.
    fn team(&self, id: &str) -> Result<Option<Team>>;
    /// Create a team.
    fn create_team(&self, name: &str) -> Result<Team>;
    /// Delete a team, and every membership in it.
    fn delete_team(&self, id: &str) -> Result<bool>;
    /// The members of a team.
    fn members(&self, team_id: &str) -> Result<Vec<Member>>;
    /// Add a member to a team, or change their role.
    fn add_member(&self, team_id: &str, username: &str, role: &str) -> Result<Member>;
    /// Remove a member from a team.
    fn remove_member(&self, team_id: &str, username: &str) -> Result<bool>;
    /// Put an account in a team, or take it out of it.
    fn set_account_team(&self, username: &str, team_id: Option<&str>) -> Result<Account>;
}

/// The instance store, in SQLite.
pub struct SqliteInstance {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl SqliteInstance {
    /// Open (or create) the store and make sure the schema exists.
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

    /// Open an in-memory store (tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        })
    }

    /// The file the store lives in.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `f` with the connection.
    pub fn with_connection<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut conn)
    }

    /// The schema.
    fn prepare(conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5_000)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        conn.execute_batch(
            r#"
-- The accounts. The password hash never leaves this table.
CREATE TABLE IF NOT EXISTS "user" (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    role          TEXT NOT NULL DEFAULT 'admin',
    team_id       TEXT
);

-- A team: several people, several websites, one set of permissions.
CREATE TABLE IF NOT EXISTS team (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS membership (
    team_id   TEXT NOT NULL,
    username  TEXT NOT NULL,
    role      TEXT NOT NULL DEFAULT 'member',
    PRIMARY KEY (team_id, username)
);

-- Every sign-in attempt, successful or not.
CREATE TABLE IF NOT EXISTS login (
    id        INTEGER PRIMARY KEY,
    username  TEXT NOT NULL,
    at        INTEGER NOT NULL,
    ok        INTEGER NOT NULL,
    ip        TEXT,
    agent     TEXT
);
CREATE INDEX IF NOT EXISTS login_at ON login (at);
CREATE INDEX IF NOT EXISTS login_username ON login (username);

-- The instance log: what happened, who did it.
CREATE TABLE IF NOT EXISTS log (
    id          INTEGER PRIMARY KEY,
    at          INTEGER NOT NULL,
    level       TEXT NOT NULL,
    kind        TEXT NOT NULL,
    actor       TEXT,
    website_id  TEXT,
    message     TEXT NOT NULL,
    data        TEXT
);
CREATE INDEX IF NOT EXISTS log_at ON log (at);

-- The Google Search Console import. A dump lands in one transaction: either the
-- whole file is in, or none of it is.
CREATE TABLE IF NOT EXISTS gsc_import (
    id          INTEGER PRIMARY KEY,
    website_id  TEXT NOT NULL,
    at          INTEGER NOT NULL,
    rows        INTEGER NOT NULL,
    source      TEXT
);

CREATE TABLE IF NOT EXISTS gsc_row (
    id           INTEGER PRIMARY KEY,
    import_id    INTEGER NOT NULL,
    website_id   TEXT NOT NULL,
    date         TEXT NOT NULL,
    query        TEXT,
    page         TEXT,
    clicks       INTEGER NOT NULL DEFAULT 0,
    impressions  INTEGER NOT NULL DEFAULT 0,
    ctr          REAL,
    position     REAL
);
CREATE INDEX IF NOT EXISTS gsc_row_website_date ON gsc_row (website_id, date);

-- The query cache. Small, hot, and it expires.
CREATE TABLE IF NOT EXISTS cache (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER
);

-- A saved report: the query and the result it produced.
CREATE TABLE IF NOT EXISTS snapshot (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    created_at  INTEGER NOT NULL,
    query       TEXT NOT NULL,
    result      TEXT NOT NULL
);

-- The AI assistant's conversations.
CREATE TABLE IF NOT EXISTS chat_session (
    id          TEXT PRIMARY KEY,
    website_id  TEXT,
    title       TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_message (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL,
    at          INTEGER NOT NULL,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    data        TEXT
);
CREATE INDEX IF NOT EXISTS chat_message_session ON chat_message (session_id, at);
"#,
        )?;
        Ok(())
    }
}

// --------------------------------------------------------------------------------------
// The accounts
// --------------------------------------------------------------------------------------

/// An account.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// The account id.
    pub id: String,
    /// The login name.
    pub username: String,
    /// The hashed password.
    pub password_hash: String,
    /// When the account was created.
    pub created_at: chrono::NaiveDateTime,
    /// The instance role: `admin` (everything) or `member`.
    ///
    /// This is **not** the role in a team — that one belongs to the membership
    /// and is `owner`, `member` or `viewer`.
    pub role: String,
    /// The team the account belongs to.
    pub team_id: Option<String>,
}

/// A sign-in attempt.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRecord {
    /// When it happened.
    pub at: chrono::NaiveDateTime,
    /// The login name that was tried.
    pub username: String,
    /// Whether it worked.
    pub ok: bool,
    /// The client address.
    pub ip: Option<String>,
    /// The user agent.
    pub agent: Option<String>,
}

impl SqliteInstance {
    /// The account with this name.
    pub fn account(&self, username: &str) -> Result<Option<Account>> {
        self.with_connection(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, username, password_hash, created_at, role, team_id
                     FROM \"user\" WHERE username = ?1",
                    params![username],
                    |row| {
                        Ok(Account {
                            id: row.get(0)?,
                            username: row.get(1)?,
                            password_hash: row.get(2)?,
                            created_at: from_ts(row.get(3)?),
                            role: row.get(4)?,
                            team_id: row.get(5)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    /// Every account.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, username, password_hash, created_at, role, team_id
                 FROM \"user\" ORDER BY username",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(Account {
                    id: row.get(0)?,
                    username: row.get(1)?,
                    password_hash: row.get(2)?,
                    created_at: from_ts(row.get(3)?),
                    role: row.get(4)?,
                    team_id: row.get(5)?,
                });
            }
            Ok(out)
        })
    }

    /// Create the account, or replace its password.
    ///
    /// This is how the admin account comes to exist on the first run.
    pub fn set_password(&self, username: &str, password_hash: &str) -> Result<Account> {
        self.with_connection(|conn| {
            let existing: Option<String> = {
                let mut stmt = conn.prepare("SELECT id FROM \"user\" WHERE username = ?1")?;
                let mut rows = stmt.query(params![username])?;
                match rows.next()? {
                    Some(row) => Some(row.get(0)?),
                    None => None,
                }
            };

            match existing {
                Some(_) => {
                    // Only the password changes. The role and the team are the
                    // instance's business, not the password's: resetting the
                    // password of a team member must not make them an admin
                    // again.
                    conn.execute(
                        "UPDATE \"user\" SET password_hash = ?2 WHERE username = ?1",
                        params![username, password_hash],
                    )?;
                    let account = conn.query_row(
                        "SELECT id, username, password_hash, created_at, role, team_id
                         FROM \"user\" WHERE username = ?1",
                        params![username],
                        |row| {
                            Ok(Account {
                                id: row.get(0)?,
                                username: row.get(1)?,
                                password_hash: row.get(2)?,
                                created_at: from_ts(row.get(3)?),
                                role: row.get(4)?,
                                team_id: row.get(5)?,
                            })
                        },
                    )?;
                    Ok(account)
                }
                None => {
                    let id = uuid::Uuid::new_v4().to_string();
                    let now = ts(crate_now());
                    conn.execute(
                        "INSERT INTO \"user\" (id, username, password_hash, created_at, role)
                         VALUES (?1, ?2, ?3, ?4, 'admin')",
                        params![id, username, password_hash, now],
                    )?;
                    Ok(Account {
                        id,
                        username: username.to_string(),
                        password_hash: password_hash.to_string(),
                        created_at: from_ts(now),
                        role: "admin".to_string(),
                        team_id: None,
                    })
                }
            }
        })
    }

    /// Record a sign-in attempt.
    pub fn record_login(
        &self,
        username: &str,
        ok: bool,
        ip: Option<&str>,
        agent: Option<&str>,
    ) -> Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO login (username, at, ok, ip, agent) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![username, ts(crate_now()), ok as i64, ip, agent],
            )?;
            Ok(())
        })
    }

    /// The sign-in attempts, newest first.
    pub fn logins(&self, limit: i64) -> Result<Vec<LoginRecord>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT username, at, ok, ip, agent FROM login
                 ORDER BY at DESC, id DESC LIMIT ?1",
            )?;
            let mut rows = stmt.query(params![limit.clamp(1, 1000)])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(LoginRecord {
                    at: from_ts(row.get(1)?),
                    username: row.get(0)?,
                    ok: row.get::<_, i64>(2)? != 0,
                    ip: row.get(3)?,
                    agent: row.get(4)?,
                });
            }
            Ok(out)
        })
    }
}

// --------------------------------------------------------------------------------------
// The log
// --------------------------------------------------------------------------------------

/// One line of the instance log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// When it happened.
    pub at: chrono::NaiveDateTime,
    /// `info`, `warn` or `error`.
    pub level: String,
    /// What kind of thing it was (`login`, `website`, `import`, `cache`, …).
    pub kind: String,
    /// Who did it.
    pub actor: Option<String>,
    /// Which website it was about.
    pub website_id: Option<String>,
    /// The message.
    pub message: String,
    /// Anything else, as JSON.
    pub data: Option<String>,
}

impl SqliteInstance {
    /// Append a line to the log.
    pub fn log(&self, entry: &LogEntry) -> Result<()> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO log (at, level, kind, actor, website_id, message, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    ts(entry.at),
                    entry.level,
                    entry.kind,
                    entry.actor,
                    entry.website_id,
                    entry.message,
                    entry.data
                ],
            )?;
            Ok(())
        })
    }

    /// The log, newest first.
    pub fn logs(&self, kind: Option<&str>, limit: i64) -> Result<Vec<LogEntry>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT at, level, kind, actor, website_id, message, data FROM log
                 WHERE (?1 IS NULL OR kind = ?1)
                 ORDER BY at DESC, id DESC LIMIT ?2",
            )?;
            let mut rows = stmt.query(params![kind, limit.clamp(1, 1000)])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(LogEntry {
                    at: from_ts(row.get(0)?),
                    level: row.get(1)?,
                    kind: row.get(2)?,
                    actor: row.get(3)?,
                    website_id: row.get(4)?,
                    message: row.get(5)?,
                    data: row.get(6)?,
                });
            }
            Ok(out)
        })
    }
}

// --------------------------------------------------------------------------------------
// The Google Search Console import
// --------------------------------------------------------------------------------------

/// One row of a Search Console export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GscRow {
    /// The day (`YYYY-MM-DD`).
    pub date: String,
    /// The search query.
    pub query: Option<String>,
    /// The landing page.
    pub page: Option<String>,
    /// The clicks.
    pub clicks: i64,
    /// The impressions.
    pub impressions: i64,
    /// The click-through rate.
    pub ctr: Option<f64>,
    /// The average position.
    pub position: Option<f64>,
}

/// What an import did.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// The import id.
    pub id: i64,
    /// How many rows landed.
    pub rows: usize,
    /// When it happened.
    pub at: chrono::NaiveDateTime,
}

impl SqliteInstance {
    /// Import a Search Console export.
    ///
    /// **All or nothing**: the whole file lands in one transaction, so a dump that
    /// fails halfway does not leave half a week of rows behind.
    pub fn import_gsc(
        &self,
        website_id: &str,
        rows: &[GscRow],
        source: Option<&str>,
    ) -> Result<ImportReport> {
        let now = ts(crate_now());
        self.with_connection(|conn| {
            let tx = conn.transaction()?;

            tx.execute(
                "INSERT INTO gsc_import (website_id, at, rows, source) VALUES (?1, ?2, ?3, ?4)",
                params![website_id, now, rows.len() as i64, source],
            )?;
            let import_id = tx.last_insert_rowid();

            {
                let mut insert = tx.prepare(
                    "INSERT INTO gsc_row (import_id, website_id, date, query, page, clicks, impressions, ctr, position)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?;
                for row in rows {
                    insert.execute(params![
                        import_id,
                        website_id,
                        row.date,
                        row.query,
                        row.page,
                        row.clicks,
                        row.impressions,
                        row.ctr,
                        row.position
                    ])?;
                }
            }

            tx.commit()?;
            Ok(ImportReport {
                id: import_id,
                rows: rows.len(),
                at: from_ts(now),
            })
        })
    }

    /// The imported Search Console rows of a website, newest day first.
    pub fn gsc_rows(&self, website_id: &str, limit: i64) -> Result<Vec<GscRow>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT date, query, page, clicks, impressions, ctr, position FROM gsc_row
                 WHERE website_id = ?1 ORDER BY date DESC, id DESC LIMIT ?2",
            )?;
            let mut rows = stmt.query(params![website_id, limit.clamp(1, 1000)])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(GscRow {
                    date: row.get(0)?,
                    query: row.get(1)?,
                    page: row.get(2)?,
                    clicks: row.get(3)?,
                    impressions: row.get(4)?,
                    ctr: row.get(5)?,
                    position: row.get(6)?,
                });
            }
            Ok(out)
        })
    }

    /// The imports of a website.
    pub fn gsc_imports(&self, website_id: &str) -> Result<Vec<ImportReport>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, at, rows FROM gsc_import WHERE website_id = ?1 ORDER BY at DESC",
            )?;
            let mut rows = stmt.query(params![website_id])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(ImportReport {
                    id: row.get(0)?,
                    at: from_ts(row.get(1)?),
                    rows: row.get::<_, i64>(2)? as usize,
                });
            }
            Ok(out)
        })
    }
}

// --------------------------------------------------------------------------------------
// The query cache
// --------------------------------------------------------------------------------------

impl SqliteInstance {
    /// A cached value, if it is still fresh.
    pub fn cache_get(&self, key: &str) -> Result<Option<String>> {
        self.with_connection(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM cache WHERE key = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                    params![key, ts(crate_now())],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    /// Put a value in the cache. `ttl` is in seconds; `None` means it never expires.
    pub fn cache_put(&self, key: &str, value: &str, ttl: Option<i64>) -> Result<()> {
        let now = ts(crate_now());
        let expires = ttl.map(|seconds| now + seconds);
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO cache (key, value, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET
                    value = excluded.value,
                    created_at = excluded.created_at,
                    expires_at = excluded.expires_at",
                params![key, value, now, expires],
            )?;
            Ok(())
        })
    }

    /// Drop the entries that have expired. Returns how many went.
    pub fn cache_purge(&self) -> Result<u64> {
        self.with_connection(|conn| {
            let removed = conn.execute(
                "DELETE FROM cache WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                params![ts(crate_now())],
            )?;
            Ok(removed as u64)
        })
    }

    /// How many entries the cache holds.
    pub fn cache_len(&self) -> Result<usize> {
        self.with_connection(|conn| {
            Ok(conn.query_row("SELECT count(*) FROM cache", [], |row| row.get::<_, i64>(0))? as usize)
        })
    }
}

// --------------------------------------------------------------------------------------
// The query snapshots
// --------------------------------------------------------------------------------------

/// A saved report.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// The snapshot id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// When it was taken.
    pub created_at: chrono::NaiveDateTime,
    /// The query that produced it.
    pub query: String,
    /// The result, as JSON.
    pub result: String,
}

impl SqliteInstance {
    /// Save a report under a name, replacing the one with that name.
    pub fn save_snapshot(&self, name: &str, query: &str, result: &str) -> Result<Snapshot> {
        let now = ts(crate_now());
        self.with_connection(|conn| {
            let id: String = conn
                .query_row(
                    "SELECT id FROM snapshot WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            conn.execute(
                "INSERT INTO snapshot (id, name, created_at, query, result) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(name) DO UPDATE SET
                    query = excluded.query,
                    result = excluded.result,
                    created_at = excluded.created_at",
                params![id, name, now, query, result],
            )?;
            Ok(Snapshot {
                id,
                name: name.to_string(),
                created_at: from_ts(now),
                query: query.to_string(),
                result: result.to_string(),
            })
        })
    }

    /// The saved reports.
    pub fn snapshots(&self) -> Result<Vec<Snapshot>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, created_at, query, result FROM snapshot ORDER BY name",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(Snapshot {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: from_ts(row.get(2)?),
                    query: row.get(3)?,
                    result: row.get(4)?,
                });
            }
            Ok(out)
        })
    }

    /// Delete a saved report.
    pub fn delete_snapshot(&self, name: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let removed = conn.execute("DELETE FROM snapshot WHERE name = ?1", params![name])?;
            Ok(removed > 0)
        })
    }
}

// --------------------------------------------------------------------------------------
// The AI chat
// --------------------------------------------------------------------------------------

/// One message of a conversation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// The message id.
    pub id: i64,
    /// When it was said.
    pub at: chrono::NaiveDateTime,
    /// `user`, `assistant` or `tool`.
    pub role: String,
    /// What was said.
    pub content: String,
    /// Anything else, as JSON.
    pub data: Option<String>,
}

/// One conversation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    /// The session id.
    pub id: String,
    /// The website the conversation is about.
    pub website_id: Option<String>,
    /// Its title.
    pub title: Option<String>,
    /// When it started.
    pub created_at: chrono::NaiveDateTime,
    /// When it was last touched.
    pub updated_at: chrono::NaiveDateTime,
}

impl SqliteInstance {
    /// Start a conversation.
    pub fn chat_session(
        &self,
        website_id: Option<&str>,
        title: Option<&str>,
    ) -> Result<ChatSession> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = ts(crate_now());
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO chat_session (id, website_id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![id, website_id, title, now],
            )?;
            Ok(())
        })?;
        Ok(ChatSession {
            id,
            website_id: website_id.map(str::to_string),
            title: title.map(str::to_string),
            created_at: from_ts(now),
            updated_at: from_ts(now),
        })
    }

    /// Add a message to a conversation.
    pub fn chat_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        data: Option<&str>,
    ) -> Result<ChatMessage> {
        let now = ts(crate_now());
        self.with_connection(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO chat_message (session_id, at, role, content, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session_id, now, role, content, data],
            )?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "UPDATE chat_session SET updated_at = ?2 WHERE id = ?1",
                params![session_id, now],
            )?;
            tx.commit()?;
            Ok(ChatMessage {
                id,
                at: from_ts(now),
                role: role.to_string(),
                content: content.to_string(),
                data: data.map(str::to_string),
            })
        })
    }

    /// The conversations, newest first.
    pub fn chat_sessions(&self) -> Result<Vec<ChatSession>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, website_id, title, created_at, updated_at FROM chat_session
                 ORDER BY updated_at DESC",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(ChatSession {
                    id: row.get(0)?,
                    website_id: row.get(1)?,
                    title: row.get(2)?,
                    created_at: from_ts(row.get(3)?),
                    updated_at: from_ts(row.get(4)?),
                });
            }
            Ok(out)
        })
    }

    /// The messages of a conversation, oldest first.
    pub fn chat_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, at, role, content, data FROM chat_message
                 WHERE session_id = ?1 ORDER BY at ASC, id ASC",
            )?;
            let mut rows = stmt.query(params![session_id])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(ChatMessage {
                    id: row.get(0)?,
                    at: from_ts(row.get(1)?),
                    role: row.get(2)?,
                    content: row.get(3)?,
                    data: row.get(4)?,
                });
            }
            Ok(out)
        })
    }
}

// --------------------------------------------------------------------------------------
// The teams
// --------------------------------------------------------------------------------------

/// A team: several people, several websites, one set of permissions.
///
/// The teams live in the **control plane**, next to the accounts — not in the
/// analytics database. The events of a site belong to the site; who may see
/// them is a question about the instance, and it belongs with the accounts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    /// The team id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// When it was created.
    pub created_at: chrono::NaiveDateTime,
}

/// One member of a team.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// The team the membership belongs to.
    pub team_id: String,
    /// The account.
    pub username: String,
    /// The role in that team: `owner`, `member` or `viewer`.
    pub role: String,
}

/// The roles a member of a team can have.
///
/// | role | what they may do |
/// | --- | --- |
/// | `owner` | everything, including managing the team |
/// | `member` | read and write the data |
/// | `viewer` | read only |
///
/// The instance admin (`admin` on the account) is above all of them.
pub const TEAM_ROLES: [&str; 3] = ["owner", "member", "viewer"];

/// Is this a role a member can have?
pub fn is_team_role(role: &str) -> bool {
    TEAM_ROLES.contains(&role)
}

impl SqliteInstance {
    /// The teams of this instance.
    pub fn teams(&self) -> Result<Vec<Team>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare("SELECT id, name, created_at FROM team ORDER BY name")?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(Team {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: from_ts(row.get(2)?),
                });
            }
            Ok(out)
        })
    }

    /// One team.
    pub fn team(&self, id: &str) -> Result<Option<Team>> {
        self.with_connection(|conn| {
            Ok(conn
                .query_row(
                    "SELECT id, name, created_at FROM team WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok(Team {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            created_at: from_ts(row.get(2)?),
                        })
                    },
                )
                .optional()?)
        })
    }

    /// Create a team.
    pub fn create_team(&self, name: &str) -> Result<Team> {
        let name = name.trim();
        if name.is_empty() {
            return Err(anyhow::anyhow!("the team needs a name"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let now = ts(crate_now());
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO team (id, name, created_at) VALUES (?1, ?2, ?3)",
                params![id, name, now],
            )?;
            Ok(())
        })?;
        Ok(Team {
            id,
            name: name.to_string(),
            created_at: from_ts(now),
        })
    }

    /// Delete a team, and every membership in it.
    ///
    /// The accounts that were in it keep their accounts; they just are not in a
    /// team any more.
    pub fn delete_team(&self, id: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE \"user\" SET team_id = NULL WHERE team_id = ?1",
                params![id],
            )?;
            tx.execute("DELETE FROM membership WHERE team_id = ?1", params![id])?;
            let removed = tx.execute("DELETE FROM team WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(removed > 0)
        })
    }

    /// The members of a team.
    pub fn members(&self, team_id: &str) -> Result<Vec<Member>> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT team_id, username, role FROM membership WHERE team_id = ?1
                 ORDER BY username",
            )?;
            let mut rows = stmt.query(params![team_id])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(Member {
                    team_id: row.get(0)?,
                    username: row.get(1)?,
                    role: row.get(2)?,
                });
            }
            Ok(out)
        })
    }

    /// Add a member to a team, or change their role.
    ///
    /// The account has to exist: a membership of an account that is not there
    /// would be a row nobody can use.
    ///
    /// **Two roles, not one.** The account keeps its instance role (`admin`),
    /// and the *team* role lives in the membership — that is what the
    /// `membership.role` column is for. Putting an account in a team therefore
    /// never takes its instance rights away, and taking it out of the team
    /// never gives it any back.
    pub fn add_member(&self, team_id: &str, username: &str, role: &str) -> Result<Member> {
        if !is_team_role(role) {
            return Err(anyhow::anyhow!(
                "'{role}' is not a role (owner, member, viewer)"
            ));
        }
        let now = ts(crate_now());
        self.with_connection(|conn| {
            let tx = conn.transaction()?;

            let exists: i64 = tx.query_row(
                "SELECT count(*) FROM team WHERE id = ?1",
                params![team_id],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(anyhow::anyhow!("no team '{team_id}'"));
            }

            let account: i64 = tx.query_row(
                "SELECT count(*) FROM \"user\" WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )?;
            if account == 0 {
                return Err(anyhow::anyhow!("no account '{username}'"));
            }

            tx.execute(
                "INSERT INTO membership (team_id, username, role) VALUES (?1, ?2, ?3)
                 ON CONFLICT(team_id, username) DO UPDATE SET role = excluded.role",
                params![team_id, username, role],
            )?;
            tx.execute(
                "UPDATE \"user\" SET team_id = ?2 WHERE username = ?1",
                params![username, team_id],
            )?;
            tx.commit()?;
            Ok(())
        })?;

        let _ = now;
        Ok(Member {
            team_id: team_id.to_string(),
            username: username.to_string(),
            role: role.to_string(),
        })
    }

    /// Remove a member from a team.
    pub fn remove_member(&self, team_id: &str, username: &str) -> Result<bool> {
        self.with_connection(|conn| {
            let tx = conn.transaction()?;
            let removed = tx.execute(
                "DELETE FROM membership WHERE team_id = ?1 AND username = ?2",
                params![team_id, username],
            )?;
            // The account stays, but it is not in a team any more.
            tx.execute(
                "UPDATE \"user\" SET team_id = NULL WHERE username = ?1 AND team_id = ?2",
                params![username, team_id],
            )?;
            tx.commit()?;
            Ok(removed > 0)
        })
    }

    /// Put an account in a team, or take it out of it.
    pub fn set_account_team(&self, username: &str, team_id: Option<&str>) -> Result<Account> {
        self.with_connection(|conn| {
            if let Some(team_id) = team_id {
                let exists: i64 = conn.query_row(
                    "SELECT count(*) FROM team WHERE id = ?1",
                    params![team_id],
                    |row| row.get(0),
                )?;
                if exists == 0 {
                    return Err(anyhow::anyhow!("no team '{team_id}'"));
                }
            }
            conn.execute(
                "UPDATE \"user\" SET team_id = ?2 WHERE username = ?1",
                params![username, team_id],
            )?;
            conn.query_row(
                "SELECT id, username, password_hash, created_at, role, team_id
                 FROM \"user\" WHERE username = ?1",
                params![username],
                |row| {
                    Ok(Account {
                        id: row.get(0)?,
                        username: row.get(1)?,
                        password_hash: row.get(2)?,
                        created_at: from_ts(row.get(3)?),
                        role: row.get(4)?,
                        team_id: row.get(5)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("no account '{username}'"))
        })
    }
}

// --------------------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------------------

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

// --------------------------------------------------------------------------------------
// The trait
// --------------------------------------------------------------------------------------

impl Instance for SqliteInstance {
    fn account(&self, username: &str) -> Result<Option<Account>> {
        SqliteInstance::account(self, username)
    }

    fn accounts(&self) -> Result<Vec<Account>> {
        SqliteInstance::accounts(self)
    }

    fn set_password(&self, username: &str, password_hash: &str) -> Result<Account> {
        SqliteInstance::set_password(self, username, password_hash)
    }

    fn record_login(
        &self,
        username: &str,
        ok: bool,
        ip: Option<&str>,
        agent: Option<&str>,
    ) -> Result<()> {
        SqliteInstance::record_login(self, username, ok, ip, agent)
    }

    fn logins(&self, limit: i64) -> Result<Vec<LoginRecord>> {
        SqliteInstance::logins(self, limit)
    }

    fn log(&self, entry: &LogEntry) -> Result<()> {
        SqliteInstance::log(self, entry)
    }

    fn logs(&self, kind: Option<&str>, limit: i64) -> Result<Vec<LogEntry>> {
        SqliteInstance::logs(self, kind, limit)
    }

    fn import_gsc(
        &self,
        website_id: &str,
        rows: &[GscRow],
        source: Option<&str>,
    ) -> Result<ImportReport> {
        SqliteInstance::import_gsc(self, website_id, rows, source)
    }

    fn gsc_rows(&self, website_id: &str, limit: i64) -> Result<Vec<GscRow>> {
        SqliteInstance::gsc_rows(self, website_id, limit)
    }

    fn gsc_imports(&self, website_id: &str) -> Result<Vec<ImportReport>> {
        SqliteInstance::gsc_imports(self, website_id)
    }

    fn cache_get(&self, key: &str) -> Result<Option<String>> {
        SqliteInstance::cache_get(self, key)
    }

    fn cache_put(&self, key: &str, value: &str, ttl: Option<i64>) -> Result<()> {
        SqliteInstance::cache_put(self, key, value, ttl)
    }

    fn cache_purge(&self) -> Result<u64> {
        SqliteInstance::cache_purge(self)
    }

    fn cache_len(&self) -> Result<usize> {
        SqliteInstance::cache_len(self)
    }

    fn save_snapshot(&self, name: &str, query: &str, result: &str) -> Result<Snapshot> {
        SqliteInstance::save_snapshot(self, name, query, result)
    }

    fn snapshots(&self) -> Result<Vec<Snapshot>> {
        SqliteInstance::snapshots(self)
    }

    fn delete_snapshot(&self, name: &str) -> Result<bool> {
        SqliteInstance::delete_snapshot(self, name)
    }

    fn chat_session(&self, website_id: Option<&str>, title: Option<&str>) -> Result<ChatSession> {
        SqliteInstance::chat_session(self, website_id, title)
    }

    fn chat_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        data: Option<&str>,
    ) -> Result<ChatMessage> {
        SqliteInstance::chat_message(self, session_id, role, content, data)
    }

    fn chat_sessions(&self) -> Result<Vec<ChatSession>> {
        SqliteInstance::chat_sessions(self)
    }

    fn chat_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        SqliteInstance::chat_messages(self, session_id)
    }

    fn teams(&self) -> Result<Vec<Team>> {
        SqliteInstance::teams(self)
    }

    fn team(&self, id: &str) -> Result<Option<Team>> {
        SqliteInstance::team(self, id)
    }

    fn create_team(&self, name: &str) -> Result<Team> {
        SqliteInstance::create_team(self, name)
    }

    fn delete_team(&self, id: &str) -> Result<bool> {
        SqliteInstance::delete_team(self, id)
    }

    fn members(&self, team_id: &str) -> Result<Vec<Member>> {
        SqliteInstance::members(self, team_id)
    }

    fn add_member(&self, team_id: &str, username: &str, role: &str) -> Result<Member> {
        SqliteInstance::add_member(self, team_id, username, role)
    }

    fn remove_member(&self, team_id: &str, username: &str) -> Result<bool> {
        SqliteInstance::remove_member(self, team_id, username)
    }

    fn set_account_team(&self, username: &str, team_id: Option<&str>) -> Result<Account> {
        SqliteInstance::set_account_team(self, username, team_id)
    }
}
