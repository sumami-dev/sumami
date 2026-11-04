//! DuckDB or SQLite — the same data, the same reports, side by side.
//!
//! Both engines implement the same [`Storage`](crate::storage::Storage) trait, so
//! the comparison is fair by construction: the same hits, the same queries, the
//! same numbers.
//!
//! ```bash
//! cargo run --release --example engine_compare -p sumami-service -- 200000
//! ```

use std::path::PathBuf;
use std::time::Instant;

use chrono::{Duration, NaiveDate};
use sumami_service::models::{Hit, HitKind};
use sumami_service::pool::Db;
use sumami_service::sqlite::SqliteBackend;
use sumami_service::storage::{Dimension, ReportQuery, Storage, TimeRange};

fn main() -> anyhow::Result<()> {
    let views: i64 = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(200_000);

    println!("{views} page views, the same data in both engines\n");

    let hits = generate(views);
    let website = "w1";
    let range = TimeRange {
        start: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap().and_hms_opt(0, 0, 0).unwrap(),
        end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap().and_hms_opt(0, 0, 0).unwrap(),
    };

    // --- DuckDB ---------------------------------------------------------
    let duck_path = temp("duckdb");
    let duck = Db::open(&duck_path, "256MB")?;
    duck.create_website("Example", Some("example.com"), 0, None)?;
    let duck_write = time(|| duck.write_batch(&hits));
    let duck_size = file_size(&duck_path);

    // --- SQLite ---------------------------------------------------------
    let sqlite_path = temp("sqlite");
    let sqlite = SqliteBackend::open(&sqlite_path)?;
    sqlite.create_website("Example", Some("example.com"), 0, None)?;
    let sqlite_write = time(|| sqlite.write_batch(&hits));
    let sqlite_size = file_size(&sqlite_path);

    // --- the queries ----------------------------------------------------
    let mut query = ReportQuery::new(website, range.clone());
    query.limit = 50;

    let mut rows = Vec::new();
    for (label, duck_time, sqlite_time) in [
        (
            "summary",
            time(|| duck.summary(&query)),
            time(|| sqlite.summary(&query)),
        ),
        (
            "timeseries",
            time(|| duck.timeseries(&query)),
            time(|| sqlite.timeseries(&query)),
        ),
        (
            "pages (top 50)",
            time(|| duck.breakdown(&query, Dimension::Url)),
            time(|| sqlite.breakdown(&query, Dimension::Url)),
        ),
        (
            "referrers",
            time(|| duck.breakdown(&query, Dimension::Referrer)),
            time(|| sqlite.breakdown(&query, Dimension::Referrer)),
        ),
        (
            "browsers",
            time(|| duck.breakdown(&query, Dimension::Browser)),
            time(|| sqlite.breakdown(&query, Dimension::Browser)),
        ),
        (
            "countries",
            time(|| duck.breakdown(&query, Dimension::Country)),
            time(|| sqlite.breakdown(&query, Dimension::Country)),
        ),
        (
            "sessions (50)",
            time(|| duck.sessions(&query)),
            time(|| sqlite.sessions(&query)),
        ),
        (
            "event log (50)",
            time(|| duck.event_log(&query)),
            time(|| sqlite.event_log(&query)),
        ),
    ] {
        rows.push((label, duck_time, sqlite_time));
    }

    println!("{:<26} {:>12} {:>12} {:>9}", "", "DuckDB", "SQLite", "ratio");
    println!("{}", "-".repeat(63));
    println!(
        "{:<26} {:>10.0} ms {:>10.0} ms {:>8.1}x",
        "write",
        duck_write * 1000.0,
        sqlite_write * 1000.0,
        duck_write / sqlite_write.max(0.0001)
    );
    for (label, duck_time, sqlite_time) in &rows {
        println!(
            "{:<26} {:>10.1} ms {:>10.1} ms {:>8.1}x",
            label,
            duck_time * 1000.0,
            sqlite_time * 1000.0,
            duck_time / sqlite_time.max(0.0001)
        );
    }
    println!();
    println!(
        "{:<26} {:>10.1} MB {:>10.1} MB {:>8.1}x",
        "size on disk",
        duck_size as f64 / 1e6,
        sqlite_size as f64 / 1e6,
        duck_size as f64 / sqlite_size.max(1) as f64
    );
    println!(
        "{:<26} {:>10.1} B  {:>10.1} B",
        "per page view",
        duck_size as f64 / views as f64,
        sqlite_size as f64 / views as f64
    );

    let _ = std::fs::remove_file(&duck_path);
    let _ = std::fs::remove_file(&sqlite_path);
    Ok(())
}

/// How long `f` took, in seconds.
fn time<T>(f: impl FnOnce() -> T) -> f64 {
    let started = Instant::now();
    let _ = f();
    started.elapsed().as_secs_f64()
}

/// The size of a file, in bytes.
fn file_size(path: &PathBuf) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// A temporary path.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sumami-engine-compare-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sumami.data")
}

/// Traffic that looks like a site's: a handful of pages, a few referrers, and a
/// visitor that comes back.
fn generate(views: i64) -> Vec<Hit> {
    let pages = [
        "/",
        "/posts/duckdb-vs-postgres",
        "/posts/self-hosting-on-a-budget",
        "/docs/install",
        "/tags/rust",
    ];
    let referrers = [
        "news.ycombinator.com",
        "www.google.com",
        "duckduckgo.com",
        "github.com",
        "",
    ];
    let browsers = ["Chrome", "Safari", "Firefox", "Edge"];
    let systems = ["Windows", "macOS", "iOS", "Android"];
    let countries = ["US", "CN", "DE", "GB", "JP"];
    let base = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();

    let mut out = Vec::with_capacity(views as usize);
    for i in 0..views {
        let session = i / 3;
        let at = base + Duration::seconds(i * 37 % (30 * 86_400));
        out.push(Hit {
            website_id: "w1".to_string(),
            session_id: format!("session-{session:08x}"),
            visitor_id: format!("visitor-{:08x}", session % 40_000),
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
