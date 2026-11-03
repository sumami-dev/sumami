//! DuckDB schema and connection helpers.

use anyhow::Result;
use duckdb::Connection;

/// Schema version of the database. Bump when adding migrations.
pub const SCHEMA_VERSION: i64 = 1;

/// Create the schema if it does not exist yet.
///
/// v1 has no migration runner: the schema is additive and `IF NOT EXISTS` throughout.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS website (
    id                VARCHAR PRIMARY KEY,
    name              VARCHAR NOT NULL,
    domain            VARCHAR,
    created_at        TIMESTAMP NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    retention_days    INTEGER NOT NULL DEFAULT 0,
    country_header    VARCHAR
);

CREATE TABLE IF NOT EXISTS session (
    id                VARCHAR PRIMARY KEY,
    website_id        VARCHAR NOT NULL,
    visitor_id        VARCHAR,
    created_at        TIMESTAMP NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    hostname          VARCHAR,
    url               VARCHAR,
    title             VARCHAR,
    referrer          VARCHAR,
    entry_url         VARCHAR,
    browser           VARCHAR,
    browser_version   VARCHAR,
    os                VARCHAR,
    os_version        VARCHAR,
    device            VARCHAR,
    country           VARCHAR,
    region            VARCHAR,
    city              VARCHAR,
    language          VARCHAR,
    screen            VARCHAR,
    data              VARCHAR,
    views             INTEGER NOT NULL DEFAULT 0,
    duration          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS event (
    id                VARCHAR PRIMARY KEY,
    website_id        VARCHAR NOT NULL,
    session_id        VARCHAR,
    created_at        TIMESTAMP NOT NULL,
    name              VARCHAR,
    url               VARCHAR,
    hostname          VARCHAR,
    data              VARCHAR
);

CREATE TABLE IF NOT EXISTS "user" (
    id                VARCHAR PRIMARY KEY,
    username          VARCHAR NOT NULL UNIQUE,
    password_hash     VARCHAR NOT NULL,
    created_at        TIMESTAMP NOT NULL,
    role              VARCHAR NOT NULL DEFAULT 'admin'
);

CREATE TABLE IF NOT EXISTS meta (
    key               VARCHAR PRIMARY KEY,
    value             VARCHAR
);
"#,
    )?;

    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

/// Read a meta value.
#[allow(dead_code)]
pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?")?;
    let mut rows = stmt.query([key])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Write a meta value.
#[allow(dead_code)]
pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

/// Current UTC timestamp as a `chrono::NaiveDateTime` (DuckDB TIMESTAMP has no timezone).
pub fn now() -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc()
}
