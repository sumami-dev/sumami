//! The schema.
//!
//! **One fact table.** Every page view is one row in `event`, carrying the same fields
//! Umami carries — that is the baseline, and the storage engine is what makes it
//! affordable. Sessions, visitors and page views are all *derived* from it, so nothing
//! is stored twice and nothing can drift apart.
//!
//! `revenue` is a separate table because it is written by the owner (imported from
//! Carbon Ads, AdSense, …), not by the tracker.

use anyhow::Result;

use duckdb::Connection;

/// Schema version of the database.
pub const SCHEMA_VERSION: i64 = 2;

/// Create the schema if it does not exist.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
-- One row per page view (or custom event). The field set mirrors Umami's
-- `website_event` row for row.
CREATE TABLE IF NOT EXISTS event (
    id              BIGINT PRIMARY KEY,
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
    data            VARCHAR
);

CREATE TABLE IF NOT EXISTS revenue (
    id          BIGINT PRIMARY KEY,
    website_id  VARCHAR NOT NULL,
    session_id  VARCHAR,
    event_id    BIGINT,
    event_name  VARCHAR,
    currency    VARCHAR NOT NULL DEFAULT 'USD',
    revenue     DOUBLE NOT NULL,
    created_at  TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS website (
    id                VARCHAR PRIMARY KEY,
    name              VARCHAR NOT NULL,
    domain            VARCHAR,
    created_at        TIMESTAMP NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    retention_days    INTEGER NOT NULL DEFAULT 0,
    country_header    VARCHAR
);

CREATE TABLE IF NOT EXISTS meta (
    key   VARCHAR PRIMARY KEY,
    value VARCHAR
);

CREATE TABLE IF NOT EXISTS "user" (
    id            VARCHAR PRIMARY KEY,
    username      VARCHAR NOT NULL UNIQUE,
    password_hash VARCHAR NOT NULL,
    created_at    TIMESTAMP NOT NULL,
    role          VARCHAR NOT NULL DEFAULT 'admin'
);
"#,
    )?;

    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

/// Now, as a naive UTC timestamp.
pub fn now() -> chrono::NaiveDateTime {
    chrono::Utc::now().naive_utc()
}
