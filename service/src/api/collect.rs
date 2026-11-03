//! Collection endpoints: `POST /api/send` (Umami v2) and `POST /api/collect` (legacy).

use axum::extract::{ConnectInfo, Extension, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use std::net::SocketAddr;

use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{Hit, HitKind, RawPayload, SendRequest};
use crate::ua;

/// Session time bucket (30 minutes), like Umami's session timeout.
const SESSION_BUCKET_SECONDS: i64 = 30 * 60;

/// Umami v2 collection endpoint.
pub async fn send(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    body: Result<Json<SendRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = body.map_err(|_| ApiError::BadRequest("invalid json body".into()))?;
    let raw = request.into_hit();

    let website_key = raw
        .str("website")
        .ok_or_else(|| ApiError::BadRequest("missing website".into()))?;

    // Umami v2 sends pageviews as `type: "event"` *without* a name; a custom event
    // carries a name. Both shapes are accepted here.
    let kind = resolve_kind(raw.kind.as_deref(), raw.str("name").is_some());

    let hit = build_hit(&state, &headers, peer.as_ref().map(|p| &p.0), &website_key, kind, &raw)?;
    state.writer.push(hit);

    Ok(StatusCode::OK)
}

/// Legacy Umami v1 collection endpoint (query parameters).
pub async fn collect(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Query(params): Query<CollectParams>,
) -> Result<StatusCode, ApiError> {
    let website_key = params
        .id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing website id".into()))?;

    let kind = resolve_kind(params.r#type.as_deref(), params.name.is_some());

    let mut fields = serde_json::Map::new();
    for (key, value) in [
        ("url", params.url),
        ("referrer", params.referrer),
        ("name", params.name),
        ("hostname", params.hostname),
        ("language", params.language),
        ("screen", params.screen),
        ("title", params.title),
        ("sessionId", params.session_id),
    ] {
        if let Some(value) = value {
            fields.insert(key.to_string(), serde_json::Value::String(value));
        }
    }
    fields.insert(
        "website".to_string(),
        serde_json::Value::String(website_key.clone()),
    );

    // Legacy tracker sends `data` as base64-encoded JSON.
    if let Some(data) = params.data {
        if let Ok(decoded) = base64_decode(&data) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&decoded) {
                fields.insert("data".to_string(), value);
            }
        }
    }

    let raw = RawPayload {
        fields,
        kind: Some(kind_name(kind).to_string()),
    };

    let hit = build_hit(&state, &headers, peer.as_ref().map(|p| &p.0), &website_key, kind, &raw)?;
    state.writer.push(hit);

    Ok(StatusCode::OK)
}

/// Query parameters accepted by the legacy endpoint.
#[derive(Debug, Deserialize)]
pub struct CollectParams {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub screen: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
}

/// Decide what kind of hit this is.
///
/// `type: "event"` without a name is a pageview (that is how the Umami v2 tracker
/// sends them); with a name it is a custom event.
fn resolve_kind(kind: Option<&str>, has_name: bool) -> HitKind {
    match kind {
        Some("screenview") => HitKind::Screenview,
        Some("identify") => HitKind::Identify,
        Some("event") | None => {
            if has_name {
                HitKind::Event
            } else {
                HitKind::Pageview
            }
        }
        Some(_) => HitKind::Pageview,
    }
}

/// The name of a hit kind.
fn kind_name(kind: HitKind) -> &'static str {
    match kind {
        HitKind::Pageview => "pageview",
        HitKind::Event => "event",
        HitKind::Screenview => "screenview",
        HitKind::Identify => "identify",
    }
}

/// Turn a request into a [`Hit`].
fn build_hit(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<&ConnectInfo<SocketAddr>>,
    website_key: &str,
    kind: HitKind,
    raw: &RawPayload,
) -> Result<Hit, ApiError> {
    let meta = state.website_meta(website_key)?;

    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let info = ua::classify(&user_agent);

    let ip = client_ip(headers, peer);
    let ip_prefix = ua::ip_prefix(&ip);

    // Session id: use the client's when provided (Umami compatibility), otherwise derive
    // a rotating id from the coarse IP prefix + user agent. The raw IP is never stored.
    let session_id = match raw.str("sessionId") {
        Some(client_id) if is_safe_session_id(&client_id) => client_id,
        _ => {
            let bucket = (chrono::Utc::now().timestamp() / SESSION_BUCKET_SECONDS) as u64;
            ua::derive_session_id(
                state.jwt_secret.as_bytes(),
                &ip_prefix,
                &user_agent,
                &meta.id,
                bucket,
            )
        }
    };

    // Visitor id: the same derivation with a daily bucket, so "unique visitors" means
    // unique visitors per day without keeping a durable identifier.
    let visitor_id = ua::derive_session_id(
        state.jwt_secret.as_bytes(),
        &ip_prefix,
        &user_agent,
        &meta.id,
        (chrono::Utc::now().timestamp() / 86_400).max(0) as u64,
    );

    let url = raw.str("url").map(|u| sanitize_url(&u, state.config.keep_query));
    let referrer = raw
        .str("referrer")
        .map(|r| sanitize_url(&r, state.config.keep_query));

    let country_header = meta
        .country_header
        .clone()
        .or_else(|| state.config.country_header.clone());
    let country = country_header
        .as_ref()
        .and_then(|header| headers.get(header.as_str()))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_uppercase())
        .filter(|v| !v.is_empty() && v.len() <= 8);

    Ok(Hit {
        website_id: meta.id,
        session_id,
        visitor_id,
        retention_days: meta.retention_days,
        kind,
        created_at: crate::db::now(),
        url,
        referrer,
        hostname: raw.str("hostname"),
        title: raw.str("title"),
        name: raw.str("name"),
        data: raw.data(),
        language: raw.str("language"),
        screen: raw.str("screen"),
        browser: info.browser,
        browser_version: info.browser_version,
        os: info.os,
        os_version: info.os_version,
        device: info.device,
        country,
        region: None,
        city: None,
    })
}

/// Strip the query string (and fragment) unless explicitly allowed.
fn sanitize_url(url: &str, keep_query: bool) -> String {
    if keep_query {
        return url.to_string();
    }
    let without_fragment = url.split('#').next().unwrap_or(url);
    match without_fragment.find('?') {
        Some(idx) => without_fragment[..idx].to_string(),
        None => without_fragment.to_string(),
    }
}

/// The client IP: prefer the proxy header, fall back to the socket address.
fn client_ip(headers: &HeaderMap, peer: Option<&ConnectInfo<SocketAddr>>) -> String {
    for header in ["x-forwarded-for", "x-real-ip", "cf-connecting-ip"] {
        if let Some(value) = headers.get(header).and_then(|v| v.to_str().ok()) {
            if let Some(first) = value.split(',').next() {
                let ip = first.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }
    }
    match peer {
        Some(ConnectInfo(addr)) => addr.ip().to_string(),
        None => "0.0.0.0".to_string(),
    }
}

/// Accept only ids that look like a UUID or a simple token (never user input verbatim).
fn is_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Decode standard base64 (no external dependency).
fn base64_decode(input: &str) -> Result<Vec<u8>, ()> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, c) in TABLE.iter().enumerate() {
        lookup[*c as usize] = i as u8;
    }

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;

    for byte in input.bytes() {
        if byte == b'=' || byte == b'\n' || byte == b'\r' {
            continue;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return Err(());
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_query_strings() {
        assert_eq!(sanitize_url("/a/b?token=secret", false), "/a/b");
        assert_eq!(sanitize_url("/a/b", false), "/a/b");
        assert_eq!(sanitize_url("/a/b?x=1", true), "/a/b?x=1");
    }

    #[test]
    fn rejects_unsafe_session_ids() {
        assert!(is_safe_session_id("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(!is_safe_session_id("bad id; DROP TABLE"));
        assert!(!is_safe_session_id(""));
    }

    #[test]
    fn decodes_base64() {
        let decoded = base64_decode("eyJhIjoxfQ==").unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "{\"a\":1}");
    }
}
