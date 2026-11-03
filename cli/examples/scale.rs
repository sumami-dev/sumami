//! Scale benchmark: how big is the data, and is a small memory limit enough?
//!
//! ```bash
//! cargo run --release -p sumami-cli --example scale -- <rows> <memory-limit>
//! ```
//!
//! Builds a synthetic table with the real schema, then runs the **actual dashboard
//! queries** (the shared report layer) against it and times them.

use std::time::Instant;

use duckdb::Connection;
use sumami_service::reports::{self, ReportQuery, TimeRange};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let rows: i64 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(10_000_000);
    let memory_limit = args.get(2).cloned().unwrap_or_else(|| "128MB".to_string());
    // "realistic" gives every row its own URL, referrer and title — the cardinality a
    // real site has. The default is the low-cardinality case (a handful of pages).
    let realistic = args.get(3).map(|v| v == "realistic").unwrap_or(false);
    // "lean" drops the page title, keeps only the referrer's host and drops the version
    // columns — the three fields a long-term archive does not need.
    let lean = args.get(3).map(|v| v == "lean").unwrap_or(false);
    let path = std::env::temp_dir().join("sumami-scale.duckdb");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(std::env::temp_dir().join("sumami-scale.duckdb.wal"));

    let conn = Connection::open(&path)?;
    conn.execute_batch(&format!("SET memory_limit='{memory_limit}'"))?;
    conn.execute_batch("SET checkpoint_threshold='256MB'")?;
    conn.execute_batch("SET threads=4")?;

    conn.execute_batch(
        "CREATE TABLE session (
            id VARCHAR, website_id VARCHAR, visitor_id VARCHAR,
            created_at TIMESTAMP, updated_at TIMESTAMP,
            hostname VARCHAR, url VARCHAR, title VARCHAR, referrer VARCHAR, entry_url VARCHAR,
            browser VARCHAR, browser_version VARCHAR, os VARCHAR, os_version VARCHAR,
            device VARCHAR, country VARCHAR, region VARCHAR, city VARCHAR,
            language VARCHAR, screen VARCHAR, data VARCHAR,
            views INTEGER, duration INTEGER);",
    )?;

    let (url_expr, title_expr, referrer_expr) = if lean {
        (
            "'/blog/post-' || (i % 500000)::VARCHAR || '-slug-' || (i % 977)::VARCHAR",
            "NULL",
            "list_extract(['www.google.com','duckduckgo.com','news.ycombinator.com','github.com','x.com','www.bing.com','reddit.com'], 1 + (i % 7)::INT)",
        )
    } else if realistic {
        (
            "'/blog/post-' || (i % 500000)::VARCHAR || '-slug-' || (i % 977)::VARCHAR",
            "'A blog post about topic number ' || (i % 100000)::VARCHAR || ' — with a longer title'",
            "'https://www.google.com/search?q=some+search+term+number+' || (i % 2000000)::VARCHAR",
        )
    } else {
        (
            "list_extract(['/','/blog/one','/blog/two','/blog/three','/docs','/docs/install','/pricing','/download','/blog/four','/blog/five','/about','/changelog','/tags/rust','/tags/duckdb','/feed.xml'], 1 + (i % 15)::INT)",
            "'Blog post ' || (i % 15)::VARCHAR",
            "list_extract(['https://www.google.com/','https://duckduckgo.com/','https://news.ycombinator.com/','https://github.com/','https://x.com/','https://www.bing.com/','https://reddit.com/'], 1 + (i % 7)::INT)",
        )
    };

    println!(
        "building {rows} sessions{}…",
        if lean {
            " (lean schema)"
        } else if realistic {
            " (realistic cardinality)"
        } else {
            ""
        }
    );
    let start = Instant::now();
    conn.execute_batch(&format!(
        r#"
INSERT INTO session
SELECT
  'session-' || i::VARCHAR,
  'w1',
  'visitor-' || (i % 3000000)::VARCHAR,
  TIMESTAMP '2024-01-01' + to_seconds((i * 7919) % 31536000),
  TIMESTAMP '2024-01-01' + to_seconds((i * 7919) % 31536000) + INTERVAL 1 MINUTE,
  'example.com',
  {url_expr},
  {title_expr},
  {referrer_expr},
  '/',
  list_extract(['Chrome','Safari','Firefox','Edge'], 1 + (i % 4)::INT),
  '120',
  list_extract(['Windows','macOS','iOS','Android','Linux'], 1 + (i % 5)::INT),
  '10',
  list_extract(['desktop','mobile','tablet'], 1 + (i % 3)::INT),
  list_extract(['US','CN','DE','GB','JP','IN','FR','BR'], 1 + (i % 8)::INT),
  NULL, NULL,
  list_extract(['en-US','zh-CN','en-GB','ja-JP'], 1 + (i % 4)::INT),
  list_extract(['1920x1080','1536x864','390x844'], 1 + (i % 3)::INT),
  NULL,
  1 + (i % 4)::INT,
  20 + (i % 400)::INT
FROM generate_series(1, {rows}) t(i);
"#
    ))?;
    conn.execute_batch("CHECKPOINT")?;
    println!("  built in {:.1}s", start.elapsed().as_secs_f64());

    let file = std::fs::metadata(&path)?.len();
    let per_row = file as f64 / rows as f64;
    println!("\n=== 数据体积 ===");
    println!("  文件大小        {:.1} MB", file as f64 / 1048576.0);
    println!("  每行            {per_row:.0} 字节（压缩后，含索引与元数据）");
    println!("  每条 session 大约对应 1–4 次浏览");

    // The dashboard's time window: the last 30 days of the synthetic year.
    let range = TimeRange {
        start: chrono::NaiveDate::from_ymd_opt(2024, 12, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        end: chrono::NaiveDate::from_ymd_opt(2024, 12, 31)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
    };
    let mut query = ReportQuery::new("w1", range);
    query.limit = 50;

    println!("\n=== 仪表盘查询耗时（30 天窗口，{} 行全表）===", rows);
    let mut timed = |label: &str, f: &dyn Fn() -> anyhow::Result<String>| {
        let start = Instant::now();
        let out = f().unwrap_or_else(|e| format!("<{e}>"));
        println!("  {label:22} {:>7.1} ms   {out}", start.elapsed().as_secs_f64() * 1000.0);
    };

    timed("summary", &|| {
        let m = reports::summary(&conn, &query)?;
        Ok(format!("{} 次浏览 / {} 访客", m.pageviews, m.visitors))
    });
    timed("timeseries", &|| {
        let rows = reports::timeseries(&conn, &query)?;
        Ok(format!("{} 个数据点", rows.len()))
    });
    timed("pages (top 50)", &|| {
        let rows = reports::breakdown(&conn, &query, reports::Dimension::Url)?;
        Ok(format!("{} 行", rows.len()))
    });
    timed("referrers", &|| {
        let rows = reports::breakdown(&conn, &query, reports::Dimension::Referrer)?;
        Ok(format!("{} 行", rows.len()))
    });
    timed("browsers", &|| {
        let rows = reports::breakdown(&conn, &query, reports::Dimension::Browser)?;
        Ok(format!("{} 行", rows.len()))
    });
    timed("sessions (50)", &|| {
        let rows = reports::sessions(&conn, &query)?;
        Ok(format!("{} 行", rows.len()))
    });

    // Extrapolation to the user's scenario.
    println!("\n=== 外推 ===");
    let per_visit = per_row / 2.5; // ~2.5 page views per session
    let visits_per_day = 10_000_000f64;
    let sessions_per_day = visits_per_day / 2.5;
    let daily_bytes = sessions_per_day * per_row;
    println!("  10M 浏览/天 ≈ {:.1}M session/天", sessions_per_day / 1e6);
    println!("  每天约            {:.1} GB", daily_bytes / 1e9);
    println!("  1 年              {:.0} GB", daily_bytes * 365.0 / 1e9);
    println!("  10 年             {:.0} GB", daily_bytes * 3650.0 / 1e9);
    println!("  20GB 磁盘能存     {:.0} 天", 20e9 / daily_bytes);
    let _ = per_visit;

    Ok(())
}
