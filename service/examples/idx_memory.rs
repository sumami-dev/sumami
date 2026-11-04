//! How much memory does the primary key's index need?
//!
//! DuckDB keeps an index **in memory**, always. The ART index of a primary key
//! holds one entry per row — so a table with hundreds of millions of rows needs
//! an index that does not fit on a small VPS, no matter how small the data is.
//!
//! This writes rows through the **same Appender path the server uses** and prints
//! the peak resident memory, so the two schemas can be compared:
//!
//! ```bash
//! /usr/bin/time -l cargo run --release --example idx_memory -p sumami-service -- 20000000 pk
//! /usr/bin/time -l cargo run --release --example idx_memory -p sumami-service -- 20000000 nopk
//! ```
//!
//! The third argument picks the write path: `appender` (the server) or `bulk`
//! (one big `INSERT ... SELECT`, which is what the migration and the seed do).
//! The fourth is the DuckDB memory limit.

use std::time::Instant;

use duckdb::Connection;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let rows: i64 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(20_000_000);
    let with_pk = args.get(2).map(|v| v == "pk").unwrap_or(false);
    let bulk = args.get(3).map(|v| v == "bulk").unwrap_or(false);
    let limit = args.get(4).cloned().unwrap_or_else(|| "2GB".to_string());

    let path = std::env::temp_dir().join(format!(
        "sumami-idx-memory-{rows}-{}.duckdb",
        if with_pk { "pk" } else { "nopk" }
    ));
    let _ = std::fs::remove_file(&path);

    let conn = Connection::open(&path)?;
    conn.execute_batch(&format!("SET memory_limit='{limit}'; SET threads=2;"))?;

    let ddl = if with_pk {
        sumami_service::db::EVENT_COLUMNS.replace(
            "id              BIGINT NOT NULL",
            "id              BIGINT PRIMARY KEY",
        )
    } else {
        sumami_service::db::EVENT_COLUMNS.to_string()
    };
    conn.execute_batch(&format!("CREATE TABLE event ({ddl});"))?;

    println!(
        "{} rows, {} the primary key, {} path, {limit} limit",
        rows,
        if with_pk { "with" } else { "without" },
        if bulk { "bulk" } else { "appender" }
    );

    let started = Instant::now();

    if bulk {
        // One statement: this is what the migration and the seed do.
        let result = conn.execute_batch(&format!(
            "INSERT INTO event
             SELECT
               i,
               'w1',
               'session-' || (i / 3)::VARCHAR,
               'visitor-' || (i % 300000)::VARCHAR,
               TIMESTAMP '2024-01-01' + to_seconds((i * 7919) % 31536000),
               '/blog/post-1',
               'utm_source=twitter&utm_medium=cpc&utm_campaign=spring&utm_content=cta&utm_term=term',
               'A title',
               'news.ycombinator.com',
               '/item',
               '?q=search',
               'example.com',
               'Chrome',
               '120',
               'macOS',
               '10',
               'desktop',
               '1920x1080',
               'en-US',
               'US',
               NULL,
               NULL,
               'twitter',
               'social',
               'spring',
               'cta',
               'term',
               NULL,
               NULL,
               NULL,
               NULL,
               NULL,
               NULL,
               1200.0,
               80.0,
               0.02,
               700.0,
               90.0,
               'pageview',
               NULL,
               NULL,
               NULL
             FROM generate_series(1, {rows}) t(i);"
        ));
        match result {
            Ok(_) => println!("  bulk insert ok"),
            Err(err) => {
                println!("  bulk insert FAILED: {err}");
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
        }
        let seconds = started.elapsed().as_secs_f64();
        conn.execute_batch("CHECKPOINT")?;
        let size = std::fs::metadata(&path)?.len();
        println!("  wrote in {seconds:.1}s, file {:.1} MB", size as f64 / 1e6);
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }

    // The server's own write path: the Appender, in batches.
    let mut id = 0i64;
    while id < rows {
        let mut appender = conn.appender("event")?;
        let batch = 50_000.min(rows - id);
        for _ in 0..batch {
            appender.append_row(duckdb::params![
                id,
                "w1",
                format!("session-{}", id / 3),
                format!("visitor-{:08x}", id % 300_000),
                chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()
                    + chrono::Duration::seconds((id * 7919) % 31_536_000),
                "/blog/post-1",
                "utm_source=twitter&utm_medium=cpc&utm_campaign=spring&utm_content=cta&utm_term=term",
                "A title",
                "news.ycombinator.com",
                "/item",
                "?q=search",
                "example.com",
                "Chrome",
                "120",
                "macOS",
                "10",
                "desktop",
                "1920x1080",
                "en-US",
                "US",
                Option::<String>::None,
                Option::<String>::None,
                "twitter",
                "social",
                "spring",
                "cta",
                "term",
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                1200.0f64,
                80.0f64,
                0.02f64,
                700.0f64,
                90.0f64,
                "pageview",
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
            ])?;
            id += 1;
        }
        appender.flush()?;
    }
    let seconds = started.elapsed().as_secs_f64();

    conn.execute_batch("CHECKPOINT")?;
    let size = std::fs::metadata(&path)?.len();
    println!("  wrote in {seconds:.1}s, file {:.1} MB", size as f64 / 1e6);

    let _ = std::fs::remove_file(&path);
    Ok(())
}
