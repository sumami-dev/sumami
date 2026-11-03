//! The hot/cold tiers must agree.
//!
//! The hot tier is a DuckDB file; the cold tier is a Parquet archive. The same report
//! code runs over both, so a query against the archive has to return exactly what the
//! live database returns — that is the whole point of the tiering.

use std::path::PathBuf;

use sumami_cli::backup;
use sumami_service::pool::Db;
use sumami_service::storage::{ReportQuery, Storage, TimeRange};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sumami-archive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_archive_answers_the_same_as_the_database() {
    let dir = temp_dir("tiering");
    let db_path = dir.join("sumami.duckdb");
    let archive = dir.join("archive");

    let db = Db::open(&db_path, "128MB").unwrap();
    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
             VALUES ('w1', 'Example', 'example.com', now(), now(), 0, NULL)",
            [],
        )?;
        // One set-based insert: 50 000 individual prepares would take minutes.
        conn.execute_batch(
            r#"
INSERT INTO session (
    id, website_id, visitor_id, created_at, updated_at, hostname, url, title, referrer,
    entry_url, browser, browser_version, os, os_version, device, country, region, city,
    language, screen, data, views, duration)
SELECT
  's' || i::VARCHAR,
  'w1',
  'v' || (i % 120)::VARCHAR,
  now()::TIMESTAMP - INTERVAL (i % 20000) MINUTE,
  now()::TIMESTAMP - INTERVAL (i % 20000) MINUTE + INTERVAL 45 SECOND,
  'example.com',
  '/page/' || (i % 7)::VARCHAR,
  NULL,
  'news.ycombinator.com',
  '/',
  'Chrome', '120', 'macOS', '10', 'desktop', 'DE', NULL, NULL,
  'en-US', '1920x1080', NULL,
  2, 45
FROM generate_series(1, 50000) t(i);
"#,
        )?;
        conn.execute(
            "INSERT INTO event (id, website_id, session_id, created_at, name, url, hostname, data)
             VALUES ('e1', 'w1', 's1', now()::TIMESTAMP, 'signup', '/signup', 'example.com', '{}')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    // The window that covers the data we just wrote.
    let range = TimeRange {
        start: chrono::Utc::now().naive_utc() - chrono::Duration::days(2),
        end: chrono::Utc::now().naive_utc() + chrono::Duration::days(1),
    };
    let mut query = ReportQuery::new("w1", range);
    query.limit = 50;

    let hot = db
        .summary(&query)
        .unwrap();

    // Archive it.
    let t1 = std::time::Instant::now();
    let stats = db
        .with_write(|conn| {
            backup::export(
                conn,
                &archive,
                None,
                Some(range.start.date()),
                Some(range.end.date()),
                false,
            )
        })
        .unwrap();
    eprintln!("export took {:.1}s", t1.elapsed().as_secs_f64());
    assert!(stats.files > 0, "the archive should contain files");

    // Read it back and ask the same question.
    let cold_conn = backup::open_archive(&format!("{}/", archive.display())).unwrap();
    let cold = cold_conn.summary(&query).unwrap();

    assert_eq!(hot.pageviews, cold.pageviews, "pageviews differ");
    assert_eq!(hot.visitors, cold.visitors, "visitors differ");
    assert_eq!(hot.views, cold.views, "sessions differ");
    assert_eq!(hot.bounces, cold.bounces, "bounces differ");

    // The archive is smaller than the database it came from (per-day Parquet files
    // with ZSTD beat DuckDB's row-group layout once there is real data in them).
    let archive_size: u64 = std::fs::read_dir(archive.join("sessions"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum();
    let db_size = std::fs::metadata(&db_path).unwrap().len();
    assert!(
        archive_size < db_size,
        "the archive ({archive_size} bytes) should be smaller than the database ({db_size} bytes)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
