//! How many bytes does one page view cost?
//!
//! "How big is an analytics database" depends entirely on how repetitive the traffic
//! is, so this probe measures three levels of realism — the same dataset, generated
//! three ways — and reports the cost per page view.
//!
//! It also measures what the index of a `PRIMARY KEY` would add, because that is
//! the one overhead an append-only design does not need.
//!
//! ```bash
//! cargo run --release --example row_cost -p sumami-service -- real 100000
//! ```

use std::time::Instant;

use duckdb::Connection;


/// The apples-to-apples number.
///
/// The PostgreSQL comparison (issue #23) was measured on a dataset whose ids are
/// `md5(...)` — 32 characters each. This measures **the same dataset** on our side,
/// so the ratio means something.
fn parity() -> anyhow::Result<()> {
    println!("=== the same dataset the PostgreSQL comparison used ===");
    let dir = std::env::temp_dir().join("sumami-row-cost-parity");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    let mut sizes = Vec::new();
    for rows in [100_000i64, 1_000_000] {
        let path = dir.join(format!("parity-{rows}.duckdb"));
        let conn = Connection::open(&path)?;
        // The real schema: the same 42 columns, the same expressions.
        conn.execute_batch(&format!(
            "CREATE TABLE event ({ddl});
             INSERT INTO event
             SELECT
               i, 'w1', md5((1 + (i / 2))::text || '-s'), md5((i % 3000)::text || '-v'),
               now()::TIMESTAMP - INTERVAL (i % 30000000) MINUTE,
               '/post/' || (i % 5000)::VARCHAR,
               'utm_source=twitter&utm_campaign=spring&utm_medium=cpc',
               'A reasonably long article title number ' || (i % 2000)::VARCHAR,
               (ARRAY['news.ycombinator.com','www.google.com','github.com','x.com'])[1 + (i % 4)],
               '/ref/path/' || (i % 1000)::VARCHAR, '?q=search+' || (i % 500)::VARCHAR, 'example.com',
               (ARRAY['Chrome','Safari','Firefox','Edge'])[1 + (i % 4)], '120',
               (ARRAY['Windows','macOS','iOS','Android'])[1 + (i % 4)], '10',
               (ARRAY['desktop','mobile','tablet'])[1 + (i % 3)], (ARRAY['1920x1080','390x844','768x1024'])[1 + (i % 3)],
               (ARRAY['en-US','zh-CN','de-DE'])[1 + (i % 3)],
               (ARRAY['US','CN','DE','JP'])[1 + (i % 4)], NULL, NULL,
               (ARRAY['twitter','newsletter','google','hn'])[1 + (i % 4)], 'cpc', 'spring',
               'cta-' || (i % 5)::VARCHAR, 'analytics',
               CASE WHEN i % 9 = 0 THEN 'gclid-' || i::VARCHAR ELSE NULL END,
               CASE WHEN i % 11 = 0 THEN 'fbclid-' || i::VARCHAR ELSE NULL END,
               NULL, NULL, NULL, NULL,
               600.0 + (i % 1800), 30.0 + (i % 260), (i % 100) / 1000.0,
               400.0 + (i % 900), 60.0 + (i % 220),
               'pageview', NULL, NULL, NULL
             FROM generate_series(1, {rows}) t(i);",
            ddl = sumami_service::db::EVENT_COLUMNS
        ))?;
        conn.execute_batch("CHECKPOINT")?;
        let size = std::fs::metadata(&path)?.len();
        println!("  {rows:>9} views -> {:>8.2} MB", size as f64 / 1e6);
        sizes.push(size);
    }

    let marginal = (sizes[1] as f64 - sizes[0] as f64) / (1_000_000 - 100_000) as f64;
    println!();
    println!("  our marginal cost:      {marginal:.1} B/浏览");
    println!("  PostgreSQL marginal:    4750.0 B/浏览");
    println!("  ratio:                  {:.0}x", 4750.0 / marginal);
    println!();

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The three levels of realism.
#[derive(Clone)]
struct Level {
    name: &'static str,
    /// The SQL expression that builds the URL.
    url: String,
    /// The SQL expression that builds the title.
    title: String,
    /// The SQL expression that builds the referrer path.
    referrer: String,
    /// The SQL expression that builds the query string.
    query: String,
    /// The SQL expression for `utm_source`.
    utm: String,
    /// The SQL expression for `gclid`.
    click: String,
    /// The SQL expression for the session id.
    session: String,
    /// The SQL expression for the visitor id.
    visitor: String,
    /// The hostname.
    host: &'static str,
}


/// The ids the server really hands out are 128-bit UUIDs — 36 characters each, and
/// they are stored in **every** row. How much does that cost, and what would the
/// DuckDB `UUID` type (16 bytes) cost instead?
fn ids() -> anyhow::Result<()> {
    println!("=== the cost of the session/visitor ids ===");
    let dir = std::env::temp_dir().join("sumami-row-cost-ids");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let level = levels()[1].clone();
    let rows = 100_000i64;

    // A short id: what the seed and the benchmarks use.
    let short = build_ids(&dir.join("short.duckdb"), &level, rows, "VARCHAR", "short")?;
    // The real thing: a 36-character UUID.
    let uuid_text = build_ids(&dir.join("uuid.duckdb"), &level, rows, "VARCHAR", "uuid")?;
    // The same value, but stored as the 16-byte DuckDB UUID type.
    let uuid_bin = build_ids(&dir.join("uuidbin.duckdb"), &level, rows, "UUID", "uuid")?;

    println!();
    println!("  short ids (the seed):        {:>7.1} B/浏览", short / rows as f64);
    println!("  UUID text (what the server does): {:>7.1} B/浏览", uuid_text / rows as f64);
    println!("  UUID type (16 bytes):        {:>7.1} B/浏览", uuid_bin / rows as f64);
    println!(
        "  -> the id format costs {:+.1} B/浏览; the UUID type saves {:+.1} B/浏览",
        uuid_text / rows as f64 - short / rows as f64,
        uuid_text / rows as f64 - uuid_bin / rows as f64
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Build the `event` table with the id columns in a given type.
fn build_ids(
    path: &std::path::Path,
    level: &Level,
    rows: i64,
    id_type: &str,
    style: &str,
) -> anyhow::Result<f64> {
    let conn = Connection::open(path)?;
    let ddl = sumami_service::db::EVENT_COLUMNS
        .replace("session_id      VARCHAR NOT NULL", &format!("session_id      {id_type} NOT NULL"))
        .replace("visitor_id      VARCHAR NOT NULL", &format!("visitor_id      {id_type} NOT NULL"));
    conn.execute_batch(&format!("CREATE TABLE event ({ddl});"))?;

    let (session, visitor) = match style {
        "uuid" => (
            // The same shape the server produces: 8-4-4-4-12 hex.
            "substr(md5('s' || (1 + (i / 2))::VARCHAR), 1, 8) || '-' ||
             substr(md5('s' || (1 + (i / 2))::VARCHAR), 9, 4) || '-' ||
             substr(md5('s' || (1 + (i / 2))::VARCHAR), 13, 4) || '-' ||
             substr(md5('s' || (1 + (i / 2))::VARCHAR), 17, 4) || '-' ||
             substr(md5('s' || (1 + (i / 2))::VARCHAR), 21, 12)",
            "substr(md5('v' || (i % 30000)::VARCHAR), 1, 8) || '-' ||
             substr(md5('v' || (i % 30000)::VARCHAR), 9, 4) || '-' ||
             substr(md5('v' || (i % 30000)::VARCHAR), 13, 4) || '-' ||
             substr(md5('v' || (i % 30000)::VARCHAR), 17, 4) || '-' ||
             substr(md5('v' || (i % 30000)::VARCHAR), 21, 12)",
        ),
        _ => (
            "'s' || (1 + (i / 2))::VARCHAR",
            "'v' || (i % 30000)::VARCHAR",
        ),
    };

    let sql = format!(
        "INSERT INTO event
         SELECT
           i, 'w1', {session}, {visitor},
           now()::TIMESTAMP - INTERVAL (i % 30000000) MINUTE,
           {url},
           'utm_source=twitter&utm_medium=cpc&utm_campaign=campaign-' || (i % 200)::VARCHAR
             || '&utm_content=cta&utm_term=term',
           {title},
           (ARRAY['news.ycombinator.com','www.google.com','github.com','x.com'])[1 + (i % 4)],
           {referrer}, {query}, '{host}',
           (ARRAY['Chrome','Safari','Firefox','Edge'])[1 + (i % 4)], '120',
           (ARRAY['Windows','macOS','iOS','Android'])[1 + (i % 4)], '10',
           (ARRAY['desktop','mobile','tablet'])[1 + (i % 3)], (ARRAY['1920x1080','390x844','768x1024'])[1 + (i % 3)],
           (ARRAY['en-US','zh-CN','de-DE'])[1 + (i % 3)],
           (ARRAY['US','CN','DE','JP'])[1 + (i % 4)], NULL, NULL,
           'twitter', 'cpc', 'campaign-' || (i % 200)::VARCHAR,
           'cta-' || (i % 20)::VARCHAR, 'term-' || (i % 50)::VARCHAR,
           CASE WHEN i % 3 = 0 THEN 'gclid-' || i::VARCHAR ELSE NULL END,
           CASE WHEN i % 11 = 0 THEN 'fbclid-' || i::VARCHAR ELSE NULL END,
           NULL, NULL, NULL, NULL,
           600.0 + (i % 1800), 30.0 + (i % 260), (i % 100) / 1000.0,
           400.0 + (i % 900), 60.0 + (i % 220),
           'pageview', NULL, NULL, NULL
         FROM generate_series(1, {rows}) t(i);",
        url = level.url,
        title = level.title,
        referrer = level.referrer,
        query = level.query,
        host = level.host,
    );
    conn.execute_batch(&sql)?;
    conn.execute_batch("CHECKPOINT")?;
    Ok(std::fs::metadata(path)?.len() as f64)
}

/// The three levels.
fn levels() -> Vec<Level> {
    vec![
        Level {
            name: "synthetic",
            url: "'/post/' || (i % 5000)::VARCHAR".to_string(),
            title: "'A reasonably long article title number ' || (i % 2000)::VARCHAR".to_string(),
            referrer: "'/ref/path/' || (i % 1000)::VARCHAR".to_string(),
            query: "'?q=search+' || (i % 500)::VARCHAR".to_string(),
            utm: "CASE WHEN i % 10 = 0 THEN NULL ELSE 'twitter' END".to_string(),
            click: "CASE WHEN i % 9 = 0 THEN 'gclid-' || i::VARCHAR ELSE NULL END".to_string(),
            session: "'s' || (1 + (i / 2))::VARCHAR".to_string(),
            visitor: "'v' || (i % 3000)::VARCHAR".to_string(),
            host: "example.com",
        },
        Level {
            name: "real",
            url: "'/blog/2026/11/a-really-long-article-slug-about-analytics-' || (i % 50000)::VARCHAR || '-tail'".to_string(),
            title: "'Why your analytics database grows faster than your traffic — a long, realistic article title ' || (i % 20000)::VARCHAR".to_string(),
            referrer: "'/item?id=' || (i % 100000)::VARCHAR || '&utm_source=hn'".to_string(),
            query: "'?q=' || (i % 100000)::VARCHAR".to_string(),
            utm: "'twitter'".to_string(),
            click: "CASE WHEN i % 3 = 0 THEN 'gclid-' || i::VARCHAR ELSE NULL END".to_string(),
            session: "'s' || (1 + (i / 2))::VARCHAR".to_string(),
            visitor: "'v' || (i % 30000)::VARCHAR".to_string(),
            host: "blog.example.com",
        },
        Level {
            name: "worst",
            url: "'/p/' || md5(i::text)::VARCHAR || '/' || md5((i+1)::text)::VARCHAR".to_string(),
            title: "'Title ' || md5((i+2)::text)::VARCHAR || ' for page ' || md5((i+3)::text)::VARCHAR".to_string(),
            referrer: "'/item/' || md5((i+4)::text)::VARCHAR".to_string(),
            query: "'?q=' || md5((i+5)::text)::VARCHAR".to_string(),
            utm: "'src-' || (i % 5000)::VARCHAR".to_string(),
            click: "CASE WHEN i % 3 = 0 THEN 'gclid-' || md5((i+6)::text)::VARCHAR ELSE NULL END".to_string(),
            session: "md5((1 + (i / 2))::text)::VARCHAR".to_string(),
            visitor: "md5((i % 30000)::text)::VARCHAR".to_string(),
            host: "blog.example.com",
        },
    ]
}

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("sumami-row-cost");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    println!("{:<12} {:>12} {:>14} {:>14}", "level", "page views", "file", "per view");
    println!("{}", "-".repeat(56));

    for level in levels() {
        let path = dir.join(format!("{}.duckdb", level.name));
        let conn = Connection::open(&path)?;
        // The real schema — the one the server creates.
        sumami_service::db::init_schema(&conn)?;

        let started = Instant::now();
        fill(&conn, &level, 100_000)?;
        let seconds = started.elapsed().as_secs_f64();

        conn.execute_batch("CHECKPOINT")?;
        let size = std::fs::metadata(&path)?.len();
        let per_view = size as f64 / 100_000.0;

        println!(
            "{:<12} {:>12} {:>14} {:>14}",
            level.name,
            "100000",
            fmt_bytes(size),
            format!("{per_view:.1} B")
        );

        // What the index of a PRIMARY KEY would have cost, on this exact schema.
        let before = size;
        conn.execute_batch("CREATE INDEX idx_event_id ON event (id)")?;
        conn.execute_batch("CHECKPOINT")?;
        let after = std::fs::metadata(&path)?.len();
        println!(
            "{:<12} {:>12} {:>14} {:>14}   <- with a PRIMARY KEY: +{:.1} B/view ({:.1}%)",
            "",
            "",
            "",
            fmt_bytes(after - before),
            (after - before) as f64 / 100_000.0,
            (after - before) as f64 / size as f64 * 100.0
        );
        println!("{:<12} fill took {seconds:.1}s\n", "");
    }

    let _ = std::fs::remove_dir_all(&dir);
    migration()?;
    ids()?;
    parity()?;
    Ok(())
}

/// What does the v2 → v3 migration do to the file?
///
/// The v2 database has a primary key on `event`, so it carries a persisted ART
/// index. The migration rebuilds the table without it — but DuckDB frees the blocks
/// of a dropped table instead of truncating the file, so the question is whether
/// the space is actually given back.
fn migration() -> anyhow::Result<()> {
    println!("=== the v2 -> v3 migration ===");
    let dir = std::env::temp_dir().join("sumami-row-cost-migrate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    let path = dir.join("v2.duckdb");
    {
        let conn = Connection::open(&path)?;
        // The v2 schema: the same table, with a primary key.
        let ddl = sumami_service::db::EVENT_COLUMNS.replace(
            "id              BIGINT NOT NULL",
            "id              BIGINT PRIMARY KEY",
        );
        conn.execute_batch(&format!("CREATE TABLE event ({ddl});"))?;
        conn.execute_batch(
            "CREATE TABLE meta (key VARCHAR PRIMARY KEY, value VARCHAR);
             INSERT INTO meta VALUES ('schema_version', '2');",
        )?;
        fill(&conn, &levels()[1], 100_000)?;
        conn.execute_batch("CHECKPOINT")?;
    }

    let before = std::fs::metadata(&path)?.len();
    println!("  a v2 database with 100000 views: {}", fmt_bytes(before));

    // Opening it migrates it: the file is rebuilt and swapped in.
    {
        let db = sumami_service::pool::Db::open(&path, "256MB")?;
        db.with_read(|conn| {
            conn.execute_batch("CHECKPOINT")?;
            Ok::<_, anyhow::Error>(())
        })?;
    }
    let after = std::fs::metadata(&path)?.len();
    println!("  after the migration:              {}", fmt_bytes(after));

    // And what a fresh database of the same data costs.
    let fresh_path = dir.join("fresh.duckdb");
    {
        let conn = Connection::open(&fresh_path)?;
        sumami_service::db::init_schema(&conn)?;
        fill(&conn, &levels()[1], 100_000)?;
        conn.execute_batch("CHECKPOINT")?;
    }
    let fresh = std::fs::metadata(&fresh_path)?.len();
    println!("  a fresh v3 database:              {}", fmt_bytes(fresh));
    println!(
        "  -> the migrated file is {:+.1} B/view against a fresh database",
        (after as f64 - fresh as f64) / 100_000.0
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Fill the event table with a dataset of the given level.
fn fill(conn: &Connection, level: &Level, rows: i64) -> anyhow::Result<()> {
    let sql = format!(
        "INSERT INTO event
         SELECT
           i, 'w1', {session}, {visitor},
           now()::TIMESTAMP - INTERVAL (i % 30000000) MINUTE,
           {url},
           'utm_source=' || {utm} || '&utm_medium=cpc&utm_campaign=campaign-' || (i % 200)::VARCHAR
             || '&utm_content=cta&utm_term=term',
           {title},
           (ARRAY['news.ycombinator.com','www.google.com','github.com','x.com'])[1 + (i % 4)],
           {referrer}, {query}, '{host}',
           (ARRAY['Chrome','Safari','Firefox','Edge'])[1 + (i % 4)], '120',
           (ARRAY['Windows','macOS','iOS','Android'])[1 + (i % 4)], '10',
           (ARRAY['desktop','mobile','tablet'])[1 + (i % 3)],
           (ARRAY['1920x1080','390x844','768x1024'])[1 + (i % 3)],
           (ARRAY['en-US','zh-CN','de-DE'])[1 + (i % 3)],
           (ARRAY['US','CN','DE','JP'])[1 + (i % 4)], NULL, NULL,
           {utm}, 'cpc', 'campaign-' || (i % 200)::VARCHAR,
           'cta-' || (i % 20)::VARCHAR, 'term-' || (i % 50)::VARCHAR,
           {click},
           CASE WHEN i % 11 = 0 THEN 'fbclid-' || i::VARCHAR ELSE NULL END,
           NULL, NULL, NULL, NULL,
           600.0 + (i % 1800), 30.0 + (i % 260), (i % 100) / 1000.0,
           400.0 + (i % 900), 60.0 + (i % 220),
           'pageview', NULL, NULL, NULL
         FROM generate_series(1, {rows}) t(i);",
        session = level.session,
        visitor = level.visitor,
        url = level.url,
        utm = level.utm,
        title = level.title,
        referrer = level.referrer,
        query = level.query,
        host = level.host,
        click = level.click,
    );
    conn.execute_batch(&sql)?;
    Ok(())
}

/// A byte count as a human string.
fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{:.0} KB", bytes as f64 / 1e3)
    }
}
