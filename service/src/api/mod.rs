//! The HTTP API.
//!
//! Layout mirrors the Umami v2 API so the existing dashboard (and the Sumami frontend)
//! can talk to it unchanged.

pub mod auth;
pub mod collect;
pub mod health;
pub mod sessions;
pub mod stats;
pub mod websites;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::routing::{get, post};
use axum::Router;

use crate::config::Config;
use crate::error::ApiError;
use crate::pool::Db;
use crate::writer::Writer;

/// Metadata we need on the hot path (collection).
#[derive(Debug, Clone)]
pub struct WebsiteMeta {
    pub id: String,
    pub domain: Option<String>,
    pub retention_days: i32,
    pub country_header: Option<String>,
}

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// The database handle (one writer, pooled readers).
    pub db: Arc<Db>,
    /// The single writer.
    pub writer: Writer,
    /// Runtime configuration.
    pub config: Arc<Config>,
    /// Secret used to sign JWTs and to derive session ids.
    pub jwt_secret: Arc<String>,
    /// Process start time.
    pub started_at: Instant,
    /// Cache of website metadata (id or domain -> meta).
    website_cache: Arc<RwLock<HashMap<String, WebsiteMeta>>>,
    /// When the cache was last refreshed (unix seconds).
    cache_stamp: Arc<AtomicU64>,
}

impl AppState {
    /// Build the state.
    pub fn new(
        db: Arc<Db>,
        writer: Writer,
        config: Arc<Config>,
        jwt_secret: Arc<String>,
    ) -> Self {
        Self {
            db,
            writer,
            config,
            jwt_secret,
            started_at: Instant::now(),
            website_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_stamp: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Look up a website by id or domain, with a short-lived cache.
    ///
    /// The cache keeps the collection path off the database: a hit only needs the
    /// website's retention policy and geo header.
    pub fn website_meta(&self, key: &str) -> Result<WebsiteMeta, ApiError> {
        let now = chrono::Utc::now().timestamp() as u64;
        let stamp = self.cache_stamp.load(Ordering::Relaxed);
        if now.saturating_sub(stamp) > 30 {
            self.refresh_website_cache();
        }

        if let Some(meta) = self
            .website_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
        {
            return Ok(meta.clone());
        }

        // A website created seconds ago may not be cached yet.
        self.refresh_website_cache();
        self.website_cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
            .ok_or_else(|| ApiError::BadRequest(format!("unknown website: {key}")))
    }

    /// Reload the website cache.
    fn refresh_website_cache(&self) {
        let websites = self
            .db
            .with_read(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, domain, retention_days, country_header FROM website",
                )?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(WebsiteMeta {
                        id: row.get(0)?,
                        domain: row.get(1)?,
                        retention_days: row.get(2)?,
                        country_header: row.get(3)?,
                    });
                }
                Ok(out)
            })
            .unwrap_or_default();

        let mut cache = self.website_cache.write().unwrap_or_else(|e| e.into_inner());
        cache.clear();
        for website in websites {
            cache.insert(website.id.clone(), website.clone());
            if let Some(domain) = website.domain.as_ref() {
                cache.insert(domain.to_lowercase(), website);
            }
        }
        drop(cache);
        self.cache_stamp
            .store(chrono::Utc::now().timestamp() as u64, Ordering::Relaxed);
    }
}

/// Build the router.
pub fn router(state: AppState) -> Router {
    Router::new()
        // --- collection (public) ---
        .route("/api/send", post(collect::send))
        .route("/api/collect", post(collect::collect))
        .route("/api/heartbeat", get(health::heartbeat))
        // --- auth ---
        .route("/api/login", post(auth::login))
        .route("/api/auth/refresh", post(auth::refresh))
        .route("/api/me", get(auth::me))
        // --- websites ---
        .route(
            "/api/websites",
            get(websites::list).post(websites::create),
        )
        .route(
            "/api/websites/{id}",
            get(websites::get)
                .put(websites::update)
                .delete(websites::delete),
        )
        // --- stats ---
        .route("/api/websites/{id}/stats", get(stats::summary))
        .route("/api/websites/{id}/stats/metrics", get(stats::metrics))
        .route(
            "/api/websites/{id}/stats/timeseries",
            get(stats::timeseries),
        )
        .route("/api/websites/{id}/stats/pages", get(stats::pages))
        .route(
            "/api/websites/{id}/stats/referrers",
            get(stats::referrers),
        )
        .route("/api/websites/{id}/stats/browsers", get(stats::browsers))
        .route("/api/websites/{id}/stats/os", get(stats::os))
        .route("/api/websites/{id}/stats/devices", get(stats::devices))
        .route(
            "/api/websites/{id}/stats/countries",
            get(stats::countries),
        )
        .route(
            "/api/websites/{id}/stats/languages",
            get(stats::languages),
        )
        .route("/api/websites/{id}/stats/screens", get(stats::screens))
        .route("/api/websites/{id}/stats/events", get(stats::events))
        .route("/api/websites/{id}/stats/urls", get(stats::urls))
        // --- raw data ---
        .route("/api/websites/{id}/sessions", get(sessions::list))
        .route("/api/websites/{id}/events", get(sessions::events))
        // --- static ---
        .route("/sumami.js", get(crate::static_files::tracker))
        .route("/app/{*path}", get(crate::static_files::app_asset))
        .route("/docs/{*path}", get(crate::static_files::docs_asset))
        .route("/docs", get(crate::static_files::docs_index))
        .route("/", get(crate::static_files::index))
        .fallback(get(crate::static_files::not_found))
        .with_state(state)
}
