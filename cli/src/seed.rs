//! Generate synthetic demo data.
//!
//! The data is entirely invented — invented domains, invented visitors, invented
//! traffic shapes. It exists so the dashboard can be evaluated (and demoed) without
//! pointing a real site at it.
//!
//! ```bash
//! sumami seed --db ./sumami.duckdb --days 30
//! ```
//!
//! Seeding writes to the database file directly, so the server has to be stopped
//! while it runs (DuckDB allows a single writer per file).

use anyhow::Result;
use chrono::Datelike;
use duckdb::Connection;
use uuid::Uuid;

/// How much data to generate.
#[derive(Debug, Clone)]
pub struct SeedOptions {
    /// The website to seed (created when missing).
    pub website: String,
    /// The domain of the website.
    pub domain: String,
    /// How many days of history.
    pub days: i64,
    /// Roughly how many visitors per day.
    pub visitors_per_day: i64,
}

/// A tiny deterministic PRNG, so a seed run is reproducible.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// The next random number.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    /// A number in `low..=high`.
    fn between(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        low + (self.next() % ((high - low + 1) as u64)) as i64
    }

    /// `true` with probability `percent`/100.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    /// Pick an item from a weighted list.
    fn weighted(&mut self, items: &[(&'static str, u64)]) -> &'static str {
        let total: u64 = items.iter().map(|(_, weight)| weight).sum();
        let mut roll = self.next() % total;
        for (value, weight) in items {
            if roll < *weight {
                return value;
            }
            roll -= weight;
        }
        items[0].0
    }
}

const URLS: &[(&str, u64)] = &[
    ("/", 1000),
    ("/blog/analytics-without-cookies", 420),
    ("/blog/duckdb-vs-postgres", 260),
    ("/blog/self-hosting-on-a-budget-vps", 210),
    ("/blog/rust-for-backend-people", 180),
    ("/docs", 150),
    ("/docs/install", 120),
    ("/pricing", 80),
    ("/download", 70),
    ("/changelog", 40),
];

const REFERRERS: &[(&str, u64)] = &[
    ("", 320),
    ("https://www.google.com/", 240),
    ("https://duckduckgo.com/", 120),
    ("https://news.ycombinator.com/", 95),
    ("https://github.com/", 70),
    ("https://x.com/", 45),
];

const BROWSERS: &[(&str, u64)] = &[
    ("Chrome", 540),
    ("Safari", 220),
    ("Firefox", 90),
    ("Edge", 80),
];

const SYSTEMS: &[(&str, u64)] = &[
    ("Windows", 330),
    ("macOS", 300),
    ("iOS", 170),
    ("Android", 130),
];

const DEVICES: &[(&str, u64)] = &[("desktop", 620), ("mobile", 330), ("tablet", 50)];

const COUNTRIES: &[(&str, u64)] = &[
    ("US", 300),
    ("CN", 150),
    ("DE", 90),
    ("GB", 85),
    ("JP", 60),
    ("IN", 55),
    ("FR", 40),
    ("BR", 35),
];

const LANGUAGES: &[(&str, u64)] = &[
    ("en-US", 440),
    ("zh-CN", 150),
    ("en-GB", 85),
    ("ja-JP", 60),
    ("de-DE", 50),
];

const SCREENS: &[(&str, u64)] = &[
    ("1920x1080", 300),
    ("1536x864", 200),
    ("1366x768", 160),
    ("390x844", 150),
    ("412x915", 60),
    ("768x1024", 40),
];

const SOURCES: &[(&str, u64)] = &[
    ("twitter", 40),
    ("newsletter", 35),
    ("google", 20),
    ("hn", 15),
];

/// The columns of the `event` table.
const EVENT_INSERT: &str = r#"INSERT INTO event (
    id, website_id, session_id, visitor_id, created_at,
    url, url_query, title, referrer, referrer_path, referrer_query, hostname,
    browser, browser_version, os, os_version, device, screen, language,
    country, region, city,
    utm_source, utm_medium, utm_campaign, utm_content, utm_term,
    gclid, fbclid, msclkid, ttclid, lifatid, twclid,
    lcp, inp, cls, fcp, ttfb,
    event_type, event_name, tag, data
) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42)"#;

/// Generate the demo data.
///
/// Returns the website id, the number of page views and the number of custom events.
pub fn seed(conn: &Connection, options: &SeedOptions) -> Result<(String, u64, u64)> {
    let mut rng = Rng::new(0x5EED_1234_ABCD_0001);

    let website_id = ensure_website(conn, &options.website, &options.domain)?;
    let now = chrono::Utc::now().naive_utc();
    let mut views = 0u64;
    let mut events = 0u64;
    let mut next_id: i64 = conn.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM event",
        [],
        |row| row.get(0),
    )?;

    conn.execute_batch("BEGIN TRANSACTION")?;

    let result = (|| -> Result<()> {
        for day in (0..options.days).rev() {
            let day_start = now - chrono::Duration::days(day);
            let weekday = day_start.weekday().num_days_from_monday() as i64;

            // Weekends are quieter, and traffic grows a little every week.
            let weekend = if weekday >= 5 { 0.62 } else { 1.0 };
            let growth = 1.0 + (options.days - day) as f64 * 0.004;
            let visitors = (options.visitors_per_day as f64 * weekend * growth) as i64;

            // A pool of visitors for the day: some of them come back, so the
            // "visitors" count is lower than the "sessions" count, like a real site.
            let pool: Vec<String> = (0..visitors)
                .map(|_| format!("seed-visitor-{:012x}", rng.next() % 0xFFFF_FFFF_FFFF))
                .collect();

            for index in 0..visitors {
                let session_id = format!("seed-session-{day}-{index}");
                let visitor_id = pool[rng.below(pool.len())].clone();
                let started = day_start + chrono::Duration::seconds(rng.between(0, 86_399));

                // A session is one to four page views.
                let page_views = match rng.below(100) {
                    0..=44 => 1,
                    45..=69 => 2,
                    70..=84 => 3,
                    _ => 4,
                };

                let referrer = rng.weighted(REFERRERS);
                let (referrer_host, referrer_path, referrer_query) = split_referrer(referrer);
                let browser = rng.weighted(BROWSERS);
                let os = rng.weighted(SYSTEMS);
                let device = rng.weighted(DEVICES);
                let country = rng.weighted(COUNTRIES);
                let language = rng.weighted(LANGUAGES);
                let screen = rng.weighted(SCREENS);
                let source = rng.weighted(SOURCES);

                let mut last_url = "/".to_string();
                for view in 0..page_views {
                    let url = rng.weighted(URLS).to_string();
                    last_url = url.clone();
                    let at = started + chrono::Duration::seconds(view * 35);

                    let url_query = format!(
                        "utm_source={}&utm_medium=cpc&utm_campaign=spring&utm_content=cta-{}&utm_term=analytics",
                        source,
                        rng.below(5)
                    );
                    let gclid = if rng.chance(9) {
                        Some(format!("gclid-{:08x}", rng.next() as u32))
                    } else {
                        None
                    };
                    let fbclid = if rng.chance(11) {
                        Some(format!("fbclid-{:08x}", rng.next() as u32))
                    } else {
                        None
                    };

                    conn.execute(
                        EVENT_INSERT,
                        duckdb::params![
                            next_id,
                            website_id,
                            session_id,
                            visitor_id,
                            at,
                            url,
                            url_query,
                            title_for(&url),
                            referrer_host,
                            referrer_path,
                            referrer_query,
                            "example.com",
                            browser,
                            "120",
                            os,
                            "10",
                            device,
                            screen,
                            language,
                            country,
                            Option::<String>::None,
                            Option::<String>::None,
                            source,
                            "cpc",
                            "spring",
                            format!("cta-{}", rng.below(5)),
                            "analytics",
                            gclid,
                            fbclid,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            600.0 + rng.between(0, 1800) as f64,
                            30.0 + rng.between(0, 260) as f64,
                            rng.between(0, 120) as f64 / 1000.0,
                            400.0 + rng.between(0, 900) as f64,
                            60.0 + rng.between(0, 220) as f64,
                            "pageview",
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                        ],
                    )?;
                    next_id += 1;
                    views += 1;
                }

                // Every third session fires a custom event.
                if rng.chance(33) {
                    conn.execute(
                        EVENT_INSERT,
                        duckdb::params![
                            next_id,
                            website_id,
                            session_id,
                            visitor_id,
                            started,
                            last_url,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            "example.com",
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            Option::<String>::None,
                            "event",
                            "signup",
                            "cta",
                            "{\"plan\":\"pro\"}",
                        ],
                    )?;
                    next_id += 1;
                    events += 1;
                }
            }
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(err);
        }
    }

    Ok((website_id, views, events))
}

/// The title of a page.
fn title_for(path: &str) -> String {
    match path {
        "/" => "Home".to_string(),
        "/blog/analytics-without-cookies" => "Analytics without cookies".to_string(),
        "/blog/duckdb-vs-postgres" => "DuckDB vs Postgres".to_string(),
        "/blog/self-hosting-on-a-budget-vps" => "Self-hosting on a budget VPS".to_string(),
        "/blog/rust-for-backend-people" => "Rust for backend people".to_string(),
        "/docs" => "Documentation".to_string(),
        "/docs/install" => "Installation".to_string(),
        "/pricing" => "Pricing".to_string(),
        "/download" => "Download".to_string(),
        "/changelog" => "Changelog".to_string(),
        other => format!("Page {other}"),
    }
}

/// Split a referrer into its host, path and query.
fn split_referrer(referrer: &str) -> (Option<String>, Option<String>, Option<String>) {
    if referrer.is_empty() {
        return (None, None, None);
    }
    let without_scheme = referrer
        .strip_prefix("https://")
        .or_else(|| referrer.strip_prefix("http://"))
        .unwrap_or(referrer);
    match without_scheme.split_once('/') {
        Some((host, rest)) => match rest.split_once('?') {
            Some((path, query)) => (
                Some(host.to_string()),
                Some(path.to_string()),
                Some(query.to_string()),
            ),
            None => (Some(host.to_string()), Some(rest.to_string()), None),
        },
        None => (Some(without_scheme.to_string()), None, None),
    }
}

/// Find or create the website.
fn ensure_website(conn: &Connection, name: &str, domain: &str) -> Result<String> {
    let existing: Option<String> = {
        let mut stmt = conn.prepare("SELECT id FROM website WHERE name = ? OR domain = ?")?;
        let mut rows = stmt.query(duckdb::params![name, domain])?;
        match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        }
    };

    if let Some(id) = existing {
        return Ok(id);
    }

    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().naive_utc();
    conn.execute(
        "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL)",
        duckdb::params![id, name, domain, now, now],
    )?;
    Ok(id)
}
