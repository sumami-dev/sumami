//! The schema.
//!
//! **One fact table.** Every page view is one row in `event`, carrying the same fields
//! Umami carries — that is the baseline, and the storage engine is what makes it
//! affordable. Sessions, visitors and page views are all *derived* from it, so nothing
//! is stored twice and nothing can drift apart.
//!
//! `revenue` is a separate table because it is written by the owner (imported from
//! Carbon Ads, AdSense, …), not by the tracker.
//!
//! **No index on the fact tables.** `event` and `revenue` are append only, and every
//! query over them is a filtered scan (`WHERE website_id = ? AND created_at BETWEEN ?`).
//! DuckDB answers those with **zone maps** — one min/max per column per row group,
//! maintained automatically and stored with the data — so a `PRIMARY KEY` would buy
//! nothing. It would cost a lot: DuckDB 2.0 persists the ART index of a primary key
//! to disk (measured: **+10.5 bytes per row**, about 12% of a realistic database),
//! slows every append by ~9%, and enforces the uniqueness constraint on every row.
//! The ids are handed out by the writer (`MAX(id) + 1`, answered from the column
//! statistics in constant time), so the constraint protects nothing.
//!
//! The small dimension tables (`website`, `meta`, `user`) keep their keys: they hold
//! a handful of rows, and `meta` needs its primary key for `ON CONFLICT` to work.

use std::path::{Path, PathBuf};

use anyhow::Result;

use duckdb::Connection;

/// Schema version of the database.
///
/// * v1 — the original session-table model.
/// * v2 — the event-row model (one row per page view).
/// * v3 — the fact tables lost their primary keys (no index on an append-only table).
pub const SCHEMA_VERSION: i64 = 3;

/// The columns of the `event` table.
///
/// One row per page view (or custom event). The field set mirrors Umami's
/// `website_event` row for row.
pub const EVENT_COLUMNS: &str = "
    id              BIGINT NOT NULL,
    website_id      VARCHAR NOT NULL,
    session_id      VARCHAR NOT NULL,
    visitor_id      VARCHAR NOT NULL,
    created_at      TIMESTAMP NOT NULL,

    url             VARCHAR,
    url_query       VARCHAR,
    title           VARCHAR,

    referrer        VARCHAR,
    referrer_path   VARCHAR,
    referrer_query  VARCHAR,
    hostname        VARCHAR,

    browser         VARCHAR,
    browser_version VARCHAR,
    os              VARCHAR,
    os_version      VARCHAR,
    device          VARCHAR,
    screen          VARCHAR,
    language        VARCHAR,
    country         VARCHAR,
    region          VARCHAR,
    city            VARCHAR,

    utm_source      VARCHAR,
    utm_medium      VARCHAR,
    utm_campaign    VARCHAR,
    utm_content     VARCHAR,
    utm_term        VARCHAR,

    gclid           VARCHAR,
    fbclid          VARCHAR,
    msclkid         VARCHAR,
    ttclid          VARCHAR,
    lifatid         VARCHAR,
    twclid          VARCHAR,

    lcp             DOUBLE,
    inp             DOUBLE,
    cls             DOUBLE,
    fcp             DOUBLE,
    ttfb            DOUBLE,

    event_type      VARCHAR,
    event_name      VARCHAR,
    tag             VARCHAR,
    data            VARCHAR";

/// The columns of the `revenue` table.
pub const REVENUE_COLUMNS: &str = "
    id          BIGINT NOT NULL,
    website_id  VARCHAR NOT NULL,
    session_id  VARCHAR,
    event_id    BIGINT,
    event_name  VARCHAR,
    currency    VARCHAR NOT NULL DEFAULT 'USD',
    revenue     DOUBLE NOT NULL,
    created_at  TIMESTAMP NOT NULL";

/// The columns of the `website` table.
pub const WEBSITE_COLUMNS: &str = "
    id                VARCHAR PRIMARY KEY,
    name              VARCHAR NOT NULL,
    domain            VARCHAR,
    created_at        TIMESTAMP NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    retention_days    INTEGER NOT NULL DEFAULT 0,
    country_header    VARCHAR";

/// The columns of the `meta` table.
pub const META_COLUMNS: &str = "
    key   VARCHAR PRIMARY KEY,
    value VARCHAR";

/// The columns of the `user` table.
pub const USER_COLUMNS: &str = "
    id            VARCHAR PRIMARY KEY,
    username      VARCHAR NOT NULL UNIQUE,
    password_hash VARCHAR NOT NULL,
    created_at    TIMESTAMP NOT NULL,
    role          VARCHAR NOT NULL DEFAULT 'admin'";

/// The tables of the schema, with their columns.
const TABLES: &[(&str, &str)] = &[
    ("event", EVENT_COLUMNS),
    ("revenue", REVENUE_COLUMNS),
    ("website", WEBSITE_COLUMNS),
    ("meta", META_COLUMNS),
    ("user", USER_COLUMNS),
];

/// Create the schema if it does not exist.
pub fn init_schema(conn: &Connection) -> Result<()> {
    for (table, columns) in TABLES {
        // `user` is a reserved word.
        let quoted = if *table == "user" {
            "\"user\""
        } else {
            *table
        };
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {quoted} ({columns});"
        ))?;
    }

    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

/// The schema version the database was written with (`0` for a fresh file).
pub fn schema_version(conn: &Connection) -> i64 {
    // `meta` does not exist yet on a fresh database.
    let Ok(mut stmt) = conn.prepare("SELECT value FROM meta WHERE key = 'schema_version'") else {
        return 0;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return 0;
    };
    match rows.next() {
        Ok(Some(row)) => row
            .get::<_, Option<String>>(0)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

/// Rewrite an older database into a new file, and say where the new file is.
///
/// The caller closes the connection and swaps the files over.
///
/// **Why a new file and not an in-place rebuild.** DuckDB cannot drop a constraint,
/// so the fact tables have to be recreated. DuckDB frees the blocks of a dropped
/// table but never gives them back to the filesystem — `VACUUM` does not shrink the
/// file either — so an in-place rebuild of a database with 400 GB of events would
/// leave a 800 GB file with 400 GB of dead space in it. Writing a new file and
/// swapping it in costs the same peak disk space and ends with a compact file.
///
/// The peak disk use is therefore about twice the size of the database. That is
/// unavoidable for any rewrite, and it is a one-off.
pub fn rebuild_to_new_file(path: &Path, conn: &Connection) -> Result<PathBuf> {
    let new_path = new_database_path(path);
    let _ = std::fs::remove_file(&new_path);

    let escaped = new_path.to_string_lossy().replace('\'', "''");

    // Which tables does the old database actually have? This is checked before the
    // new database is attached: `information_schema` spans every attached database.
    let existing: Vec<&str> = TABLES
        .iter()
        .map(|(table, _)| *table)
        .filter(|table| table_exists(conn, table))
        .collect();

    conn.execute_batch(&format!("ATTACH '{escaped}' AS fresh"))?;

    for (table, columns) in TABLES {
        let quoted = if *table == "user" {
            "\"user\""
        } else {
            *table
        };
        conn.execute_batch(&format!("CREATE TABLE fresh.{quoted} ({columns});"))?;
        // A table the old database never had (an early v2 file has no `revenue`)
        // stays empty rather than failing the migration.
        if existing.contains(table) {
            conn.execute_batch(&format!(
                "INSERT INTO fresh.{quoted} SELECT * FROM main.{quoted};"
            ))?;
        }
    }

    // The copy still says it was written by the old version — fix that, or the new
    // file would be migrated again the next time it is opened.
    conn.execute(
        "INSERT INTO fresh.meta (key, value) VALUES ('schema_version', ?) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;

    conn.execute_batch("DETACH fresh")?;
    Ok(new_path)
}

/// Where the rebuilt database is written, before it is swapped in.
fn new_database_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "sumami.duckdb".to_string());
    name.push_str(".rebuilt");
    path.with_file_name(name)
}

/// Does the database have this table?
fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_name = ? AND table_schema = 'main'",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Now, as a naive UTC timestamp.
pub fn now() -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc()
}
