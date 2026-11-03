//! The storage abstraction.
//!
//! Sumami's reports are defined once and implemented once **per backend**. This module
//! holds the trait and the query types that every backend speaks; the SQL itself lives
//! in the backend, because SQL is not portable across engines (timestamps, date
//! bucketing, distinct counting and upsert syntax all differ).
//!
//! The DuckDB backend is in [`crate::reports`]. The SQLite and PostgreSQL backends
//! live in the private `sumami-pro` repository, under `pro/storage/`.
//!
//! A backend must produce **exactly** the same numbers as the others — that invariant
//! is what the cross-backend tests assert.

use anyhow::Result;
use chrono::NaiveDateTime;

use crate::models::{
    BreakdownRow, EventRow, MetricsSummary, SessionRow, Website,
};

/// A time window (start inclusive, end exclusive).
#[derive(Debug, Clone, Copy)]
pub struct TimeRange {
    /// Start of the window.
    pub start: NaiveDateTime,
    /// End of the window.
    pub end: NaiveDateTime,
}

impl TimeRange {
    /// The window immediately before this one, of the same length.
    pub fn previous(&self) -> TimeRange {
        let span = self.end - self.start;
        TimeRange {
            start: self.start - span,
            end: self.start,
        }
    }

    /// The default window: the last 7 days.
    pub fn last_days(days: i64) -> TimeRange {
        let end = crate::db::now();
        TimeRange {
            start: end - chrono::Duration::days(days),
            end,
        }
    }
}

/// The dimension filters (Umami-compatible query parameters).
#[derive(Debug, Clone, Default)]
pub struct Dimensions {
    /// Only this page.
    pub url: Option<String>,
    /// Only this referrer.
    pub referrer: Option<String>,
    /// Only this browser.
    pub browser: Option<String>,
    /// Only this operating system.
    pub os: Option<String>,
    /// Only this device class.
    pub device: Option<String>,
    /// Only this country.
    pub country: Option<String>,
}

/// Everything a report needs.
#[derive(Debug, Clone)]
pub struct ReportQuery {
    /// The website the report is about.
    pub website_id: String,
    /// The time window.
    pub range: TimeRange,
    /// The dimension filters.
    pub dimensions: Dimensions,
    /// How many rows to return.
    pub limit: i64,
    /// How many rows to skip.
    pub offset: i64,
}

impl ReportQuery {
    /// A query for one website and one window.
    pub fn new(website_id: &str, range: TimeRange) -> Self {
        Self {
            website_id: website_id.to_string(),
            range,
            dimensions: Dimensions::default(),
            limit: 50,
            offset: 0,
        }
    }
}

/// The dimensions a breakdown can be computed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    /// The page path.
    Url,
    /// The referring page.
    Referrer,
    /// The browser family.
    Browser,
    /// The operating system family.
    Os,
    /// The device class.
    Device,
    /// The country.
    Country,
    /// The interface language.
    Language,
    /// The viewport size.
    Screen,
}

impl Dimension {
    /// The column in the `session` table.
    pub fn column(self) -> &'static str {
        match self {
            Dimension::Url => "url",
            Dimension::Referrer => "referrer",
            Dimension::Browser => "browser",
            Dimension::Os => "os",
            Dimension::Device => "device",
            Dimension::Country => "country",
            Dimension::Language => "language",
            Dimension::Screen => "screen",
        }
    }

    /// How the rows of a breakdown are counted.
    ///
    /// Content dimensions (pages, referrers) count views; audience dimensions
    /// (browser, OS, device, country, language, screen) count unique visitors.
    pub fn aggregate(self) -> &'static str {
        match self {
            Dimension::Url | Dimension::Referrer => "views",
            _ => "visitors",
        }
    }

    /// Every dimension, in the order the API exposes them.
    pub fn all() -> [Dimension; 8] {
        [
            Dimension::Url,
            Dimension::Referrer,
            Dimension::Browser,
            Dimension::Os,
            Dimension::Device,
            Dimension::Country,
            Dimension::Language,
            Dimension::Screen,
        ]
    }
}

/// The raw metric values for a window.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct SummaryMetrics {
    /// Total page views.
    pub pageviews: i64,
    /// Unique visitors (daily-rotating id).
    pub visitors: i64,
    /// Number of sessions.
    pub views: i64,
    /// Sessions with a single page view.
    pub bounces: i64,
    /// Average session duration in seconds.
    pub visit_duration: f64,
}

/// What a backend reports about itself.
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    /// The backend's name (`duckdb`, `sqlite`, `postgres`).
    pub backend: String,
    /// How much memory the engine currently holds, in bytes.
    pub memory_bytes: i64,
    /// The engine's memory ceiling, as reported by the engine itself.
    pub memory_limit: String,
    /// How many threads the engine may use.
    pub threads: i64,
    /// The size of the database on disk, in bytes.
    pub size_bytes: i64,
    /// How many websites are stored.
    pub websites: i64,
}

/// A stored user (the admin account).
#[derive(Debug, Clone)]
pub struct UserRecord {
    /// The user's id.
    pub id: String,
    /// The username.
    pub username: String,
    /// The role (`admin`).
    pub role: String,
    /// When the account was created.
    pub created_at: NaiveDateTime,
    /// The Argon2 hash of the password.
    pub password_hash: String,
}

/// A change to a website.
#[derive(Debug, Clone, Default)]
pub struct WebsitePatch {
    /// A new name.
    pub name: Option<String>,
    /// A new domain.
    pub domain: Option<String>,
    /// A new retention window, in days.
    pub retention_days: Option<i32>,
    /// A new country header.
    pub country_header: Option<String>,
}

/// One session, aggregated from the hits of a batch.
#[derive(Debug, Clone)]
pub struct SessionAggregate {
    /// The session id.
    pub id: String,
    /// The website it belongs to.
    pub website_id: String,
    /// The rotating visitor id.
    pub visitor_id: Option<String>,
    /// When the session started.
    pub created_at: NaiveDateTime,
    /// When the session was last seen.
    pub updated_at: NaiveDateTime,
    /// The hostname of the site.
    pub hostname: Option<String>,
    /// The last URL.
    pub url: Option<String>,
    /// The page title.
    pub title: Option<String>,
    /// The referring host.
    pub referrer: Option<String>,
    /// The first URL.
    pub entry_url: Option<String>,
    /// The browser family.
    pub browser: Option<String>,
    /// The browser version.
    pub browser_version: Option<String>,
    /// The operating system.
    pub os: Option<String>,
    /// The operating system version.
    pub os_version: Option<String>,
    /// The device class.
    pub device: Option<String>,
    /// The country.
    pub country: Option<String>,
    /// The region.
    pub region: Option<String>,
    /// The city.
    pub city: Option<String>,
    /// The interface language.
    pub language: Option<String>,
    /// The viewport size.
    pub screen: Option<String>,
    /// Free-form data attached to the session.
    pub data: Option<String>,
    /// How many page views the session had.
    pub views: i64,
}

impl SessionAggregate {
    /// Start from a hit.
    fn from(hit: &crate::models::Hit) -> Self {
        Self {
            id: hit.session_id.clone(),
            website_id: hit.website_id.clone(),
            visitor_id: Some(hit.visitor_id.clone()),
            created_at: hit.created_at,
            updated_at: hit.created_at,
            hostname: hit.hostname.clone(),
            url: hit.url.clone(),
            title: hit.title.clone(),
            referrer: hit.referrer.clone(),
            entry_url: hit.url.clone(),
            browser: hit.browser.clone(),
            browser_version: hit.browser_version.clone(),
            os: hit.os.clone(),
            os_version: hit.os_version.clone(),
            device: hit.device.clone(),
            country: hit.country.clone(),
            region: hit.region.clone(),
            city: hit.city.clone(),
            language: hit.language.clone(),
            screen: hit.screen.clone(),
            data: hit.data.clone(),
            // The first pageview of a session counts as one view.
            views: if hit.kind == crate::models::HitKind::Pageview {
                1
            } else {
                0
            },
        }
    }

    /// Fold another hit of the same session into it.
    fn merge(&mut self, hit: &crate::models::Hit) {
        self.updated_at = hit.created_at;
        self.views += 1;
        if hit.url.is_some() {
            self.url = hit.url.clone();
        }
        if hit.title.is_some() {
            self.title = hit.title.clone();
        }
        if hit.referrer.is_some() {
            self.referrer = hit.referrer.clone();
        }
        if hit.screen.is_some() {
            self.screen = hit.screen.clone();
        }
        if hit.data.is_some() {
            self.data = hit.data.clone();
        }
    }
}

/// A batch of hits, grouped the way every backend wants it.
pub struct Batch<'a> {
    /// The sessions to upsert.
    pub sessions: Vec<SessionAggregate>,
    /// The custom events to insert.
    pub events: Vec<&'a crate::models::Hit>,
}

/// Group a batch of hits into sessions and events.
///
/// The grouping is backend-agnostic — every backend upserts the same sessions and
/// inserts the same events, only the SQL differs.
pub fn aggregate_batch<'a>(hits: &'a [crate::models::Hit]) -> Batch<'a> {
    let mut sessions: Vec<SessionAggregate> = Vec::new();
    let mut events: Vec<&crate::models::Hit> = Vec::new();

    for hit in hits {
        match hit.kind {
            crate::models::HitKind::Pageview => {
                match sessions.iter_mut().find(|s| s.id == hit.session_id) {
                    Some(existing) => existing.merge(hit),
                    None => sessions.push(SessionAggregate::from(hit)),
                }
            }
            crate::models::HitKind::Event | crate::models::HitKind::Screenview => events.push(hit),
            crate::models::HitKind::Identify => {
                if let Some(existing) = sessions.iter_mut().find(|s| s.id == hit.session_id) {
                    existing.data = hit.data.clone();
                } else {
                    let mut aggregate = SessionAggregate::from(hit);
                    aggregate.views = 0;
                    sessions.push(aggregate);
                }
            }
        }
    }

    Batch { sessions, events }
}

/// A storage backend.
pub trait Storage: Send + Sync {
    /// The backend's name.
    fn backend(&self) -> &'static str;

    /// List the websites.
    fn websites(&self) -> Result<Vec<Website>>;

    /// Load one website.
    fn website(&self, id: &str) -> Result<Option<Website>>;

    /// Create a website.
    fn create_website(
        &self,
        name: &str,
        domain: Option<&str>,
        retention_days: i32,
        country_header: Option<&str>,
    ) -> Result<Website>;

    /// Update a website.
    fn update_website(&self, id: &str, patch: &WebsitePatch) -> Result<Website>;

    /// Delete a website and all of its data. Returns whether it existed.
    fn delete_website(&self, id: &str) -> Result<bool>;

    /// The summary metrics for a window.
    fn summary(&self, query: &ReportQuery) -> Result<SummaryMetrics>;

    /// The summary metrics plus the period-over-period change.
    fn summary_with_change(&self, query: &ReportQuery) -> Result<MetricsSummary>;

    /// Break a website down over one dimension.
    fn breakdown(&self, query: &ReportQuery, dimension: Dimension) -> Result<Vec<BreakdownRow>>;

    /// Page views over time.
    fn timeseries(&self, query: &ReportQuery) -> Result<Vec<BreakdownRow>>;

    /// Custom events, grouped by name.
    fn events(&self, query: &ReportQuery) -> Result<Vec<BreakdownRow>>;

    /// The raw event log.
    fn event_log(&self, query: &ReportQuery) -> Result<Vec<EventRow>>;

    /// The session log.
    fn sessions(&self, query: &ReportQuery) -> Result<Vec<SessionRow>>;

    /// Persist a batch of hits.
    fn write_batch(&self, hits: &[crate::models::Hit]) -> Result<()>;

    /// Delete data that is older than the retention window.
    fn prune(&self, website: Option<&str>) -> Result<(u64, u64)>;

    /// Delete everything older than a moment. Used by the archival job once the cold
    /// tier holds the data.
    fn delete_before(
        &self,
        before: NaiveDateTime,
        website: Option<&str>,
    ) -> Result<(u64, u64)>;

    /// Delete a website and all of its data. Returns the number of rows removed.
    fn erase(&self, website: &str) -> Result<u64>;

    /// Look up a user by name.
    fn user(&self, username: &str) -> Result<Option<UserRecord>>;

    /// Create or update a user's password.
    fn set_user_password(&self, username: &str, password_hash: &str) -> Result<UserRecord>;

    /// What the backend reports about itself.
    fn stats(&self) -> Result<StorageStats>;
}
