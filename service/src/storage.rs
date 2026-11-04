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
use uuid::Uuid;

use crate::models::{
    BreakdownRow, EventRow, MetricsSummary, SessionRow, Website,
};

/// A time window (start inclusive, end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub id: Uuid,
    /// The website it belongs to.
    pub website_id: String,
    /// The rotating visitor id.
    pub visitor_id: Option<Uuid>,
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
            id: hit.session_id,
            website_id: hit.website_id.clone(),
            visitor_id: Some(hit.visitor_id),
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

/// One event of the session log: the fields a session row is built from.
#[derive(Debug, Clone)]
pub struct SessionEvent {
    /// The row id — the tiebreaker when two events share a timestamp.
    pub id: i64,
    /// The session the event belongs to.
    pub session_id: Uuid,
    /// The rotating visitor id.
    pub visitor_id: Option<Uuid>,
    /// When it happened.
    pub created_at: NaiveDateTime,
    /// The page.
    pub url: Option<String>,
    /// The page title.
    pub title: Option<String>,
    /// The referring host.
    pub referrer: Option<String>,
    /// The referring path.
    pub referrer_path: Option<String>,
    /// The hostname of the site.
    pub hostname: Option<String>,
    /// The browser family.
    pub browser: Option<String>,
    /// The operating system.
    pub os: Option<String>,
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
}

/// How many events the session log reads.
///
/// The session log is a **log**: it shows the most recent sessions, not a
/// summary of the whole window. Aggregating every event of a month to show the
/// last fifty sessions is wasted work — the answer only depends on the last
/// few thousand events.
///
/// So the log is derived from the most recent events, and the cost of the log is
/// the cost of that scan, not the cost of the table. Fifty sessions need about
/// fifty events each, so the budget is fifty times the number of rows asked
/// for, with a floor and a ceiling.
pub fn session_scan_limit(limit: i64) -> i64 {
    (limit.max(1) * 50).clamp(500, 200_000)
}

/// Group the most recent events into the rows of the session log.
///
/// The rules are the same on every backend, because this is where they live:
///
///   * the **last** page of the session is the `url` and the `title`;
///   * the **first** page is the `entry_url`;
///   * the referrer is the one the session **started** with;
///   * the visitor's environment (browser, country, screen, …) is the one of the
///     **newest** event that carries it — those do not change within a session;
///   * a session that is only half in the scan is reported with the views it has
///     in the scan, which is the price of a bounded scan.
pub fn group_sessions(
    website_id: &str,
    events: &[SessionEvent],
    limit: i64,
    offset: i64,
) -> Vec<SessionRow> {
    // The events arrive newest first; the grouping needs them oldest first.
    let mut by_session: std::collections::HashMap<Uuid, Vec<&SessionEvent>> =
        std::collections::HashMap::new();
    for event in events {
        by_session
            .entry(event.session_id)
            .or_default()
            .push(event);
    }

    let mut rows: Vec<SessionRow> = by_session
        .into_iter()
        .map(|(session_id, mut group)| {
            group.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            let first = group.first().expect("a group is never empty");
            let last = group.last().expect("a group is never empty");

            let newest_with = |pick: fn(&SessionEvent) -> Option<&String>| -> Option<String> {
                group
                    .iter()
                    .rev()
                    .find_map(|event| pick(event).cloned())
            };
            let oldest_with = |pick: fn(&SessionEvent) -> Option<&String>| -> Option<String> {
                group.iter().find_map(|event| pick(event).cloned())
            };

            SessionRow {
                id: session_id.to_string(),
                website_id: website_id.to_string(),
                created_at: first.created_at,
                updated_at: last.created_at,
                hostname: newest_with(|event| event.hostname.as_ref()),
                url: last.url.clone(),
                title: newest_with(|event| event.title.as_ref()),
                referrer: oldest_with(|event| event.referrer.as_ref()),
                referrer_path: oldest_with(|event| event.referrer_path.as_ref()),
                entry_url: first.url.clone(),
                browser: newest_with(|event| event.browser.as_ref()),
                os: newest_with(|event| event.os.as_ref()),
                device: newest_with(|event| event.device.as_ref()),
                country: newest_with(|event| event.country.as_ref()),
                region: newest_with(|event| event.region.as_ref()),
                city: newest_with(|event| event.city.as_ref()),
                language: newest_with(|event| event.language.as_ref()),
                screen: newest_with(|event| event.screen.as_ref()),
                views: group.len() as i64,
                duration: (last.created_at - first.created_at).num_seconds(),
            }
        })
        .collect();

    // The newest session first; the id breaks the tie, so the order never
    // depends on the order the rows came back in.
    rows.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    let offset = offset.max(0) as usize;
    rows.into_iter().skip(offset).take(limit.max(0) as usize).collect()
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

    /// The most recent events of a window, newest first.
    ///
    /// The session log is built from these — see [`group_sessions`]. A backend
    /// only has to read the last few thousand rows, not the whole window.
    fn recent_events(&self, query: &ReportQuery, limit: i64) -> Result<Vec<SessionEvent>>;

    /// Persist a batch of hits.
    fn write_batch(&self, hits: &[crate::models::Hit]) -> Result<()>;

    /// Persist a batch of revenue records.
    fn write_revenue(&self, records: &[crate::models::Revenue]) -> Result<()>;

    /// The revenue of a website over a window.
    fn revenue(&self, query: &ReportQuery) -> Result<Vec<crate::models::Revenue>>;

    /// Delete data that is older than the retention window of its website.
    ///
    /// Returns how many **events** and how many **revenue records** were removed.
    /// The two numbers mean the same thing on every backend.
    fn prune(&self, website: Option<&str>) -> Result<(u64, u64)>;

    /// Delete everything older than a moment. Used by the archival job once the cold
    /// tier holds the data.
    ///
    /// Returns how many **events** and how many **revenue records** were removed.
    fn delete_before(
        &self,
        before: NaiveDateTime,
        website: Option<&str>,
    ) -> Result<(u64, u64)>;

    /// Delete a website and all of its data. Returns the number of rows removed.
    fn erase(&self, website: &str) -> Result<u64>;

    /// What the backend reports about itself.
    fn stats(&self) -> Result<StorageStats>;
}
