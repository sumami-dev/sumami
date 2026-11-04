//! Where does the write time go?
//!
//! The three-engine comparison says DuckDB is **slower than SQLite** on small
//! batches — the way the tracker actually writes. That is worth knowing, but
//! "slower" is not an explanation. This probe takes the write path apart and
//! times each piece:
//!
//! | piece | what it is |
//! | --- | --- |
//! | the id query | `SELECT MAX(id) + 1 FROM event` — how the next row id is found |
//! | the appender | `conn.appender("event")` — starting an append transaction |
//! | the appends | binding 42 values per row |
//! | the flush | committing the append transaction |
//!
//! It also runs two variants, so the cost of the design choices is visible:
//!
//! * **the id counter** — the next id is kept in memory instead of asked for;
//! * **one appender** — the appender is created once and flushed per batch.
//!
//! ```bash
//! cargo run --release --example write_probe -p sumimi-service -- 100000
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use sumimi_service::models::{Hit, HitKind};
use sumimi_service::pool::Db;
use sumimi_service::sqlite::SqliteBackend;
use sumimi_service::storage::Storage;

/// How many rows to write.
fn rows() -> usize {
    std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000)
}

/// How many rows per batch.
fn batch() -> usize {
    std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(100)
}

/// The pieces of one write path, in microseconds.
#[derive(Debug, Default, Clone, Copy)]
struct Parts {
    id_query: Duration,
    appender: Duration,
    appends: Duration,
    flush: Duration,
}

impl Parts {
    fn total(&self) -> Duration {
        self.id_query + self.appender + self.appends + self.flush
    }

    fn ms(&self) -> f64 {
        self.total().as_secs_f64() * 1000.0
    }
}

/// Write `hits` to DuckDB the way the server does it: one appender per batch,
/// the next id asked for every time.
fn duckdb_current(hits: &[Hit]) -> anyhow::Result<Parts> {
    let path = temp("duckdb-current");
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path, "256MB")?;

    let mut parts = Parts::default();
    db.with_write(|conn| {
        for chunk in hits.chunks(batch()) {
            let started = Instant::now();
            let mut next_id: i64 = conn.query_row(
                "SELECT COALESCE(MAX(id), 0) + 1 FROM event",
                [],
                |row| row.get(0),
            )?;
            parts.id_query += started.elapsed();

            let started = Instant::now();
            let mut appender = conn.appender("event")?;
            parts.appender += started.elapsed();

            let started = Instant::now();
            for hit in chunk {
                append_row(&mut appender, next_id, hit)?;
                next_id += 1;
            }
            parts.appends += started.elapsed();

            let started = Instant::now();
            appender.flush()?;
            parts.flush += started.elapsed();
        }
        Ok::<_, anyhow::Error>(())
    })?;
    let _ = std::fs::remove_file(&path);
    Ok(parts)
}

/// The same, but the next id is kept in memory.
fn duckdb_counter(hits: &[Hit]) -> anyhow::Result<Parts> {
    let path = temp("duckdb-counter");
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path, "256MB")?;

    let mut parts = Parts::default();
    db.with_write(|conn| {
        let mut next_id: i64 = conn.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM event",
            [],
            |row| row.get(0),
        )?;
        for chunk in hits.chunks(batch()) {
            let started = Instant::now();
            let mut appender = conn.appender("event")?;
            parts.appender += started.elapsed();

            let started = Instant::now();
            for hit in chunk {
                append_row(&mut appender, next_id, hit)?;
                next_id += 1;
            }
            parts.appends += started.elapsed();

            let started = Instant::now();
            appender.flush()?;
            parts.flush += started.elapsed();
        }
        Ok::<_, anyhow::Error>(())
    })?;
    let _ = std::fs::remove_file(&path);
    Ok(parts)
}

/// The same, but the appender is created **once** and flushed per batch.
fn duckdb_one_appender(hits: &[Hit]) -> anyhow::Result<Parts> {
    let path = temp("duckdb-one-appender");
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path, "256MB")?;

    let mut parts = Parts::default();
    db.with_write(|conn| {
        let mut next_id: i64 = conn.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM event",
            [],
            |row| row.get(0),
        )?;
        let mut appender = conn.appender("event")?;
        for chunk in hits.chunks(batch()) {
            let started = Instant::now();
            for hit in chunk {
                append_row(&mut appender, next_id, hit)?;
                next_id += 1;
            }
            parts.appends += started.elapsed();

            let started = Instant::now();
            appender.flush()?;
            parts.flush += started.elapsed();
        }
        Ok::<_, anyhow::Error>(())
    })?;
    let _ = std::fs::remove_file(&path);
    Ok(parts)
}

/// The SQLite backend, for the comparison.
fn sqlite(hits: &[Hit]) -> anyhow::Result<(f64, u64)> {
    let path = temp("sqlite");
    let _ = std::fs::remove_file(&path);
    let backend = SqliteBackend::open(&path)?;

    let started = Instant::now();
    for chunk in hits.chunks(batch()) {
        backend.write_batch(chunk)?;
    }
    let seconds = started.elapsed().as_secs_f64();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&path);
    Ok((seconds, size))
}

/// Append one row, in the column order of the schema.
fn append_row(
    appender: &mut duckdb::Appender<'_>,
    id: i64,
    hit: &Hit,
) -> anyhow::Result<()> {
    appender.append_row(duckdb::params![
        id,
        hit.website_id,
        hit.session_id,
        hit.visitor_id,
        hit.created_at,
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
    Ok(())
}

/// How expensive is `MAX(id)` on a table of `rows` rows?
///
/// The schema comment says the id is handed out by `MAX(id) + 1`, "answered from
/// the column statistics in constant time". This checks that claim.
fn id_query_cost(rows: usize) -> anyhow::Result<(f64, f64)> {
    let path = temp(&format!("id-cost-{rows}"));
    let _ = std::fs::remove_file(&path);
    let db = Db::open(&path, "256MB")?;

    let hits = generate(rows as i64);
    db.with_write(|conn| {
        let mut appender = conn.appender("event")?;
        let mut next_id: i64 = 1;
        for hit in &hits {
            append_row(&mut appender, next_id, hit)?;
            next_id += 1;
        }
        appender.flush()?;
        Ok::<_, anyhow::Error>(())
    })?;

    let (plain, coalesced) = db.with_read(|conn| {
        // The plain aggregate.
        let started = Instant::now();
        let _: Option<i64> = conn.query_row("SELECT MAX(id) FROM event", [], |row| row.get(0))?;
        let plain = started.elapsed().as_secs_f64() * 1000.0;

        // The one the server actually runs.
        let started = Instant::now();
        let _: i64 = conn.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM event",
            [],
            |row| row.get(0),
        )?;
        let coalesced = started.elapsed().as_secs_f64() * 1000.0;
        Ok::<_, anyhow::Error>((plain, coalesced))
    })?;

    let _ = std::fs::remove_file(&path);
    Ok((plain, coalesced))
}

fn main() -> anyhow::Result<()> {
    let total = rows();
    let per_batch = batch();
    let hits = generate(total as i64);

    println!(
        "{total} rows, {per_batch} per batch, the production schema (42 columns)\n"
    );

    let current = duckdb_current(&hits)?;
    let counter = duckdb_counter(&hits)?;
    let one = duckdb_one_appender(&hits)?;
    let (sqlite_seconds, sqlite_size) = sqlite(&hits)?;

    println!(
        "{:<24} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "path", "total", "id query", "appender", "appends", "flush"
    );
    println!("{}", "-".repeat(78));
    for (name, parts) in [
        ("the server, today", &current),
        ("the id counter", &counter),
        ("one appender", &one),
    ] {
        println!(
            "{:<24} {:>8.0} ms {:>8.0} ms {:>8.0} ms {:>8.0} ms {:>8.0} ms",
            name,
            parts.ms(),
            parts.id_query.as_secs_f64() * 1000.0,
            parts.appender.as_secs_f64() * 1000.0,
            parts.appends.as_secs_f64() * 1000.0,
            parts.flush.as_secs_f64() * 1000.0,
        );
    }
    println!(
        "{:<24} {:>8.0} ms {:>30}",
        "SQLite",
        sqlite_seconds * 1000.0,
        format!("{} B/row", sqlite_size as f64 / total as f64)
    );

    println!();
    println!("the id query, on a table of N rows");
    println!("{}", "-".repeat(78));
    println!(
        "{:<24} {:>14} {:>20}",
        "rows", "MAX(id)", "COALESCE(MAX(id),0)+1"
    );
    for rows in [100usize, 10_000, 100_000, 1_000_000] {
        let (plain, coalesced) = id_query_cost(rows)?;
        println!("{:<24} {:>11.3} ms {:>17.3} ms", rows, plain, coalesced);
    }

    println!();
    println!("per row, in microseconds");
    println!("{}", "-".repeat(78));
    for (name, parts) in [
        ("the server, today", &current),
        ("the id counter", &counter),
        ("one appender", &one),
    ] {
        println!(
            "{:<24} {:>8.2} us {:>8.2} us {:>8.2} us {:>8.2} us",
            name,
            parts.id_query.as_secs_f64() * 1e6 / total as f64,
            parts.appender.as_secs_f64() * 1e6 / total as f64,
            parts.appends.as_secs_f64() * 1e6 / total as f64,
            parts.flush.as_secs_f64() * 1e6 / total as f64,
        );
    }
    println!(
        "{:<24} {:>8.2} us",
        "SQLite",
        sqlite_seconds * 1e6 / total as f64
    );

    Ok(())
}

/// A temporary path.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sumimi-write-probe-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sumimi.data")
}

/// Traffic that looks like a site's.
fn generate(views: i64) -> Vec<Hit> {
    let pages = [
        "/",
        "/posts/duckdb-vs-postgres",
        "/posts/self-hosting-on-a-budget",
        "/docs/install",
        "/tags/rust",
    ];
    let referrers = ["news.ycombinator.com", "www.google.com", "duckduckgo.com", "github.com", ""];
    let browsers = ["Chrome", "Safari", "Firefox", "Edge"];
    let systems = ["Windows", "macOS", "iOS", "Android"];
    let countries = ["US", "CN", "DE", "GB", "JP"];
    let base = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();

    let mut out = Vec::with_capacity(views as usize);
    for i in 0..views {
        let session = i / 3;
        let at = base + chrono::Duration::seconds(i * 37 % (30 * 86_400));
        out.push(Hit {
            website_id: "w1".to_string(),
            session_id: sumimi_service::ua::uuid_from_text(&format!("session-{session:08x}")),
            visitor_id: sumimi_service::ua::uuid_from_text(&format!("visitor-{:08x}", session % 40_000)),
            retention_days: 0,
            kind: HitKind::Pageview,
            created_at: at,
            url: Some(pages[(i as usize) % pages.len()].to_string()),
            url_query: Some("utm_source=twitter&utm_medium=cpc".to_string()),
            title: Some(format!("A post about topic {}", i % 500)),
            referrer: {
                let referrer = referrers[(i as usize) % referrers.len()];
                if referrer.is_empty() {
                    None
                } else {
                    Some(referrer.to_string())
                }
            },
            referrer_path: Some("/item?id=1".to_string()),
            referrer_query: Some("?id=1".to_string()),
            hostname: Some("example.com".to_string()),
            browser: Some(browsers[(i as usize) % browsers.len()].to_string()),
            browser_version: Some("120".to_string()),
            os: Some(systems[(i as usize) % systems.len()].to_string()),
            os_version: Some("11".to_string()),
            device: Some(if i % 3 == 0 { "mobile" } else { "desktop" }.to_string()),
            screen: Some("1920x1080".to_string()),
            language: Some("en-US".to_string()),
            country: Some(countries[(i as usize) % countries.len()].to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            utm_source: Some("twitter".to_string()),
            utm_medium: Some("cpc".to_string()),
            utm_campaign: Some("spring".to_string()),
            utm_content: Some("cta".to_string()),
            utm_term: Some("analytics".to_string()),
            gclid: None,
            fbclid: None,
            msclkid: None,
            ttclid: None,
            lifatid: None,
            twclid: None,
            lcp: Some(600.0 + (i % 2000) as f64),
            inp: Some(20.0 + (i % 300) as f64),
            cls: Some((i % 100) as f64 / 1000.0),
            fcp: Some(400.0 + (i % 900) as f64),
            ttfb: Some(60.0 + (i % 220) as f64),
            name: None,
            tag: None,
            data: None,
        });
    }
    out
}
