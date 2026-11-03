//! Wire types: the Umami-compatible payloads we accept and the shapes we return.

use serde::{Deserialize, Serialize};

/// A collected hit, normalised from any of the accepted wire formats.
#[derive(Debug, Clone)]
pub struct Hit {
    pub website_id: String,
    pub session_id: String,
    /// Daily-rotating, non-reversible visitor id (unique visitors per day).
    pub visitor_id: String,
    /// Retention window of the website in days (`0` = keep forever).
    pub retention_days: i32,
    pub kind: HitKind,
    pub created_at: chrono::NaiveDateTime,
    pub url: Option<String>,
    pub referrer: Option<String>,
    pub hostname: Option<String>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub data: Option<String>,
    pub language: Option<String>,
    pub screen: Option<String>,
    pub browser: Option<String>,
    pub browser_version: Option<String>,
    pub os: Option<String>,
    pub os_version: Option<String>,
    pub device: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
}

/// What kind of hit this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// A page view.
    Pageview,
    /// A custom event.
    Event,
    /// A screen view (mobile).
    Screenview,
    /// An identify call (attaches data to the session).
    Identify,
}

impl HitKind {
    /// Parse the Umami `type` discriminator.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "event" => Some(HitKind::Event),
            "screenview" => Some(HitKind::Screenview),
            "identify" => Some(HitKind::Identify),
            _ => None,
        }
    }
}

/// Body of `POST /api/send` (Umami v2 shape).
#[derive(Debug, Deserialize)]
pub struct SendRequest {
    /// Website id or domain. May also live inside `payload`.
    #[serde(default)]
    pub website: Option<String>,
    /// `event`, `screenview` or `identify`.
    #[serde(default)]
    pub r#type: Option<String>,
    /// The hit payload.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

impl SendRequest {
    /// Extract the flat payload fields we care about.
    pub fn into_hit(self) -> RawPayload {
        let mut flat = serde_json::Map::new();
        if let Some(payload) = self.payload {
            if let Some(obj) = payload.as_object() {
                flat = obj.clone();
            }
        }
        if let Some(w) = self.website {
            flat.entry("website".to_string()).or_insert(w.into());
        }
        RawPayload { fields: flat, kind: self.r#type }
    }
}

/// Normalised payload fields (JSON object).
pub struct RawPayload {
    pub fields: serde_json::Map<String, serde_json::Value>,
    pub kind: Option<String>,
}

impl RawPayload {
    /// Read a string field.
    pub fn str(&self, key: &str) -> Option<String> {
        self.fields
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Read the `data` object (custom event properties).
    pub fn data(&self) -> Option<String> {
        let value = self.fields.get("data")?;
        match value {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        }
    }
}

/// A website as returned by the API (camelCase, like Umami).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Website {
    pub id: String,
    pub name: String,
    pub domain: Option<String>,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
    pub retention_days: i32,
    pub country_header: Option<String>,
}

/// Request body for creating a website.
#[derive(Debug, Deserialize)]
pub struct CreateWebsite {
    pub name: String,
    #[serde(default)]
    pub domain: Option<String>,
}

/// Request body for updating a website.
#[derive(Debug, Deserialize)]
pub struct UpdateWebsite {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub retention_days: Option<i32>,
    #[serde(default)]
    pub country_header: Option<String>,
}

/// Login request.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// The user object returned after login.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub username: String,
    pub role: String,
    pub created_at: chrono::NaiveDateTime,
}

/// Login response (`{ token, user }`, like Umami).
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: User,
}

/// A metric with its period-over-period change.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricValue {
    pub value: i64,
    pub change: f64,
}

/// Summary metrics, Umami-shaped.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSummary {
    pub pageviews: MetricValue,
    pub visitors: MetricValue,
    pub views: MetricValue,
    pub bounces: MetricValue,
    pub visit_duration: MetricValue,
}

/// One row of a breakdown.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    /// The dimension value (url, referrer, browser name, ...).
    pub name: String,
    pub value: i64,
    /// Page URL for the `pages` breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Bucket start for the `timeseries` breakdown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// A session row.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: String,
    pub website_id: String,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
    pub hostname: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub referrer: Option<String>,
    pub entry_url: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub device: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub language: Option<String>,
    pub screen: Option<String>,
    pub views: i64,
    pub duration: i64,
}

/// A custom event row.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub id: String,
    pub website_id: String,
    pub session_id: Option<String>,
    pub created_at: chrono::NaiveDateTime,
    pub name: Option<String>,
    pub url: Option<String>,
    pub hostname: Option<String>,
    pub data: Option<String>,
}

/// Heartbeat / health payload.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    pub ok: bool,
    pub version: &'static str,
    pub uptime_seconds: u64,
    pub queued: u64,
    pub dropped: u64,
    pub websites: i64,
    /// What DuckDB's buffer manager currently holds in memory.
    pub duckdb_memory_bytes: i64,
    /// The configured DuckDB memory limit (e.g. `256MB`).
    pub duckdb_memory_limit: String,
    /// The limit DuckDB itself reports (read back from DuckDB, not from our config).
    pub duckdb_memory_limit_effective: String,
    /// How many threads DuckDB may use.
    pub duckdb_threads: i64,
    /// The size of the database file on disk.
    pub database_size_bytes: i64,
}

/// Query parameters shared by the stats endpoints (Umami-compatible names).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsQuery {
    /// Unix milliseconds, inclusive.
    #[serde(default)]
    pub start_at: Option<i64>,
    /// Unix milliseconds, exclusive.
    #[serde(default)]
    pub end_at: Option<i64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub browser: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}
