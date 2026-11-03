//! Static assets: the tracker, the dashboard shell and the frontend files.
//!
//! The backend deliberately serves a **very small HTML document** that only references
//! the frontend JavaScript. The frontend can therefore live on a CDN (or be replaced by
//! a newer build) without touching the backend binary — and the binary itself stays tiny
//! because the frontend is never embedded in it.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::api::AppState;

/// The tracker, embedded in the binary so it is always available at `/sumami.js`.
pub const TRACKER_JS: &str = include_str!("../../js/sumami.js");

/// The dashboard shell. Deliberately tiny: everything else lives in the frontend JS.
const APP_SHELL: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sumami</title>
<link rel="stylesheet" href="{base}/sumami.css">
</head>
<body>
<div id="app"><noscript>Sumami needs JavaScript for the dashboard. The API works without it.</noscript></div>
<script src="{base}/sumami-app.js" defer></script>
</body>
</html>
"#;

/// `GET /` — the dashboard shell.
pub async fn index(State(state): State<AppState>) -> Response {
    // The frontend lives under `/app` unless a CDN (or another origin) is configured.
    let base = state.config.app_url.clone().unwrap_or_else(|| "/app".to_string());
    let html = APP_SHELL.replace("{base}", &base);

    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

/// `GET /sumami.js` — the tracker.
pub async fn tracker() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        TRACKER_JS,
    )
        .into_response()
}

/// `GET /app/{*path}` — the frontend files (served from the `js/` directory).
pub async fn app_asset(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    // Never escape the asset directory.
    if path.contains("..") {
        return not_found().await;
    }

    let file = state.config.js_dir.join(&path);
    let file = match tokio::fs::canonicalize(&file).await {
        Ok(path) => path,
        Err(_) => return not_found().await,
    };
    let root = match tokio::fs::canonicalize(&state.config.js_dir).await {
        Ok(path) => path,
        Err(_) => return not_found().await,
    };
    if !file.starts_with(&root) {
        return not_found().await;
    }

    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let mime = mime_for(&path);
            (
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, "public, max-age=60"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => not_found().await,
    }
}

/// `GET /docs` — the documentation index.
pub async fn docs_index(State(state): State<AppState>) -> Response {
    let file = state.config.docs_dir.join("index.html");
    match tokio::fs::read(&file).await {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => not_found().await,
    }
}

/// `GET /docs/{*path}` — the documentation site.
pub async fn docs_asset(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    if path.contains("..") {
        return not_found().await;
    }

    let mut file = state.config.docs_dir.join(&path);
    if file.is_dir() {
        file = file.join("index.html");
    }

    let bytes = match tokio::fs::read(&file).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found().await,
    };

    let mime = mime_for(&path);
    (
        [(header::CONTENT_TYPE, mime)],
        bytes,
    )
        .into_response()
}

/// The 404 page.
pub async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"not found","status":404}"#,
    )
        .into_response()
}

/// Guess the content type from the file extension.
fn mime_for(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".js") || lower.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".html") || lower.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".ico") {
        "image/x-icon"
    } else if lower.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}
