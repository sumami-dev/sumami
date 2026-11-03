//! Integration tests for the write pipeline and the HTTP API.

use std::sync::Arc;
use std::time::Duration;

use sumami_service::models::{Hit, HitKind};
use sumami_service::pool::Db;
use sumami_service::writer::Writer;

/// A unique temporary database path.
fn temp_db(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sumami-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sumami.duckdb")
}

/// A synthetic hit.
fn hit(website_id: &str, url: &str) -> Hit {
    Hit {
        website_id: website_id.to_string(),
        session_id: "session-1".to_string(),
        visitor_id: "visitor-1".to_string(),
        retention_days: 0,
        kind: HitKind::Pageview,
        created_at: chrono::Utc::now().naive_utc(),
        url: Some(url.to_string()),
        referrer: Some("https://news.ycombinator.com/".to_string()),
        hostname: Some("example.com".to_string()),
        title: Some("Example".to_string()),
        name: None,
        data: None,
        language: Some("en-US".to_string()),
        screen: Some("1920x1080".to_string()),
        browser: Some("Chrome".to_string()),
        browser_version: Some("120".to_string()),
        os: Some("macOS".to_string()),
        os_version: Some("10.15".to_string()),
        device: Some("desktop".to_string()),
        country: Some("DE".to_string()),
        region: None,
        city: None,
    }
}

/// The writer must persist queued hits.
#[tokio::test(flavor = "multi_thread")]
async fn writer_flushes_queued_hits() {
    let path = temp_db("writer");
    let db = Arc::new(Db::open(&path, "256MB").unwrap());

    let (writer, _handle) = Writer::spawn(db.clone(), Duration::from_millis(200), 1000);
    writer.push(hit("website-1", "/one"));
    writer.push(hit("website-1", "/two"));

    let mut sessions = 0;
    for _ in 0..100 {
        sessions = db
            .with_read(|conn| {
                let mut stmt = conn.prepare("SELECT count(*) FROM session")?;
                let mut rows = stmt.query([])?;
                match rows.next()? {
                    Some(row) => Ok(row.get::<_, i64>(0)?),
                    None => Ok(0),
                }
            })
            .unwrap();
        if sessions > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(sessions, 1, "the writer should have flushed the batch");

    // The session keeps the last url and counts both views.
    let (url, views) = db
        .with_read(|conn| {
            let mut stmt = conn.prepare("SELECT url, views FROM session")?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
                None => Ok((None, 0)),
            }
        })
        .unwrap();

    assert_eq!(url.as_deref(), Some("/two"));
    assert_eq!(views, 2);

    writer.shutdown();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Custom events are stored in the event table.
#[tokio::test(flavor = "multi_thread")]
async fn writer_stores_custom_events() {
    let path = temp_db("events");
    let db = Arc::new(Db::open(&path, "256MB").unwrap());

    let (writer, _handle) = Writer::spawn(db.clone(), Duration::from_millis(200), 1000);

    let mut event = hit("website-1", "/signup");
    event.kind = HitKind::Event;
    event.name = Some("signup".to_string());
    event.data = Some("{\"plan\":\"pro\"}".to_string());
    writer.push(event);

    let mut events = 0;
    for _ in 0..100 {
        events = db
            .with_read(|conn| {
                let mut stmt = conn.prepare("SELECT count(*) FROM event")?;
                let mut rows = stmt.query([])?;
                match rows.next()? {
                    Some(row) => Ok(row.get::<_, i64>(0)?),
                    None => Ok(0),
                }
            })
            .unwrap();
        if events > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(events, 1);

    writer.shutdown();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
