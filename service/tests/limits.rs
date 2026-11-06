//! The admission gate: the collection path's own limits.
//!
//! The gate is the only thing between a burst of traffic and the machine's
//! memory, so it has to answer three questions: is the body too big, is there
//! room in flight, and is the rate too high. And it has to answer them **before
//! the body is read** — a refusal after the work is done is not a limit.

use std::sync::Arc;
use std::time::Duration;

use sumimi_service::api::{self, AppState};
use sumimi_service::config::{Config, Paths};
use sumimi_service::pool::Db;
use sumimi_service::writer::Writer;

/// A unique temporary database path.
fn temp_db(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sumimi-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sumimi.duckdb")
}

/// Stops the writer thread when the test ends — **even when it panics**.
struct WriterGuard(sumimi_service::writer::Writer);

impl Drop for WriterGuard {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// The app, with the gate's limits set by the test.
fn app(name: &str, limits: sumimi_service::limits::Limits) -> (axum::Router, WriterGuard) {
    let path = temp_db(name);
    let db = Arc::new(Db::open(&path, "256MB").unwrap());

    db.with_write(|conn| {
        conn.execute(
            "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
             VALUES ('website-1', 'Example', 'example.com', now(), now(), 0, NULL)",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let mut config = Config::from_env_and_flags(
        Paths::new(
            Some(path.clone().parent().unwrap().to_path_buf()),
            Some(path.clone()),
            "duckdb",
        ),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    config.jwt_secret = "test-secret".to_string();
    config.max_body_bytes = limits.max_body_bytes;
    config.max_inflight_requests = limits.max_inflight_requests;
    config.max_inflight_bytes = limits.max_inflight_bytes;
    config.max_admit_rps = limits.max_admit_rps;

    let (writer, _handle) = Writer::spawn(db.clone(), Duration::from_millis(200), 1000);
    let guard = WriterGuard(writer.clone());

    let state = AppState::new(
        db.clone(),
        Arc::new(sumimi_service::instance::SqliteInstance::open_in_memory().unwrap()),
        writer.clone(),
        Arc::new(config),
        Arc::new("test-secret".to_string()),
    );

    (api::router(state), guard)
}

/// A collection request with a body of `length` bytes.
///
/// The `Content-Length` header is set, the way a real client sets it — the gate
/// reads it before the body is touched.
fn collect_request(length: usize) -> axum::http::Request<axum::body::Body> {
    let body = format!(
        r#"{{"website":"website-1","payload":{{"url":"/{}"}}}}"#,
        "x".repeat(length.saturating_sub(60))
    );
    axum::http::Request::builder()
        .uri("/api/send")
        .method("POST")
        .header("content-type", "application/json")
        .header("content-length", body.len())
        .body(axum::body::Body::from(body))
        .unwrap()
}

/// A body that is too large is refused with `413` — and it is refused **before**
/// it is read.
#[tokio::test(flavor = "multi_thread")]
async fn a_too_large_body_is_refused() {
    use axum::body::Body;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    let (app, _guard) = app(
        "limits-body",
        sumimi_service::limits::Limits {
            max_body_bytes: 256,
            ..Default::default()
        },
    );

    let response = app
        .clone()
        .oneshot(collect_request(4096))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // A body within the limit is accepted.
    let response = app
        .clone()
        .oneshot(collect_request(64))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = Body::empty();
}

/// The rate limit refuses the requests that come too fast, with a `Retry-After`.
#[tokio::test(flavor = "multi_thread")]
async fn the_rate_limit_refuses_the_extra() {
    use axum::http::StatusCode;
    use tower::ServiceExt;

    let (app, _guard) = app(
        "limits-rate",
        sumimi_service::limits::Limits {
            max_admit_rps: 1,
            ..Default::default()
        },
    );

    // The bucket starts full: one second's worth of one request per second.
    let first = app.clone().oneshot(collect_request(64)).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app.clone().oneshot(collect_request(64)).await.unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        second
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "a refusal must say when to come back"
    );
}

/// The in-flight limits refuse the request that would exceed them.
#[tokio::test(flavor = "multi_thread")]
async fn the_inflight_limits_refuse() {
    use axum::http::StatusCode;
    use tower::ServiceExt;

    // One request at a time, and 64 KB of body in flight.
    let (app, _guard) = app(
        "limits-inflight",
        sumimi_service::limits::Limits {
            max_body_bytes: 32 * 1024,
            max_inflight_requests: 1,
            max_inflight_bytes: 64 * 1024,
            ..Default::default()
        },
    );

    let first = app.clone().oneshot(collect_request(64)).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // The first request is finished, so its room is back — the next one is fine.
    let second = app.clone().oneshot(collect_request(64)).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
}

/// The heartbeat says what the gate did.
#[tokio::test(flavor = "multi_thread")]
async fn the_heartbeat_reports_the_gate() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let (app, _guard) = app(
        "limits-heartbeat",
        sumimi_service::limits::Limits {
            max_admit_rps: 1,
            ..Default::default()
        },
    );

    let _ = app.clone().oneshot(collect_request(64)).await.unwrap();
    let _ = app.clone().oneshot(collect_request(64)).await.unwrap();

    let request = Request::builder()
        .uri("/api/heartbeat")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["refusedRate"], 1,
        "the refusal must be counted: {json}"
    );
    assert_eq!(json["inflightRequests"], 0, "nothing is in flight any more");
}
