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

    /// Pick an item from a weighted list (the weights are relative).
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

/// The pages of the demo site, with a Zipf-ish traffic distribution.
const PAGES: &[(&str, u64)] = &[
    ("/", 1000),
    ("/blog/analytics-without-cookies", 420),
    ("/blog/duckdb-vs-postgres", 260),
    ("/blog/self-hosting-on-a-budget-vps", 210),
    ("/blog/rust-for-backend-people", 180),
    ("/docs", 150),
    ("/docs/install", 120),
    ("/docs/api", 90),
    ("/pricing", 80),
    ("/download", 70),
    ("/blog", 65),
    ("/changelog", 40),
    ("/about", 30),
    ("/tags/rust", 25),
    ("/tags/analytics", 20),
    ("/contact", 12),
    ("/feed.xml", 10),
];

/// The titles of the pages.
fn title_for(path: &str) -> &'static str {
    match path {
        "/" => "Home",
        "/blog" => "Blog",
        "/blog/analytics-without-cookies" => "Analytics without cookies",
        "/blog/duckdb-vs-postgres" => "DuckDB vs Postgres",
        "/blog/self-hosting-on-a-budget-vps" => "Self-hosting on a budget VPS",
        "/blog/rust-for-backend-people" => "Rust for backend people",
        "/docs" => "Documentation",
        "/docs/install" => "Installation",
        "/docs/api" => "API reference",
        "/pricing" => "Pricing",
        "/download" => "Download",
        "/changelog" => "Changelog",
        "/about" => "About",
        "/tags/rust" => "Posts tagged Rust",
        "/tags/analytics" => "Posts tagged analytics",
        "/contact" => "Contact",
        "/feed.xml" => "RSS feed",
        _ => "Page",
    }
}

/// Where the traffic comes from.
const REFERRERS: &[(&str, u64)] = &[
    ("", 320),
    ("https://www.google.com/", 240),
    ("https://duckduckgo.com/", 120),
    ("https://news.ycombinator.com/", 95),
    ("https://github.com/", 70),
    ("https://x.com/", 45),
    ("https://www.bing.com/", 40),
    ("https://reddit.com/r/rust", 30),
    ("https://lobste.rs/", 25),
    ("https://newsletter.example.com/", 15),
];

/// The countries.
const COUNTRIES: &[(&str, u64)] = &[
    ("US", 300),
    ("CN", 150),
    ("DE", 90),
    ("GB", 85),
    ("JP", 60),
    ("IN", 55),
    ("FR", 40),
    ("BR", 35),
    ("CA", 30),
    ("AU", 25),
    ("NL", 18),
    ("SG", 12),
];

/// The browsers.
const BROWSERS: &[(&str, u64)] = &[
    ("Chrome", 540),
    ("Safari", 220),
    ("Firefox", 90),
    ("Edge", 80),
    ("Samsung Internet", 35),
    ("Opera", 25),
];

/// The operating systems.
const OPERATING_SYSTEMS: &[(&str, u64)] = &[
    ("Windows", 330),
    ("macOS", 300),
    ("iOS", 170),
    ("Android", 130),
    ("Linux", 70),
];

/// The device classes.
const DEVICES: &[(&str, u64)] = &[("desktop", 620), ("mobile", 330), ("tablet", 50)];

/// The interface languages.
const LANGUAGES: &[(&str, u64)] = &[
    ("en-US", 440),
    ("zh-CN", 150),
    ("en-GB", 85),
    ("ja-JP", 60),
    ("de-DE", 50),
    ("fr-FR", 40),
    ("es-ES", 30),
    ("pt-BR", 25),
];

/// The screen sizes.
const SCREENS: &[(&str, u64)] = &[
    ("1920x1080", 300),
    ("1536x864", 200),
    ("1366x768", 160),
    ("1280x720", 90),
    ("390x844", 150),
    ("412x915", 60),
    ("768x1024", 40),
];

/// The custom events a demo visitor can fire.
const EVENTS: &[(&str, u64)] = &[
    ("newsletter_subscribe", 100),
    ("signup", 60),
    ("download", 40),
    ("demo_request", 20),
];

/// Generate the demo data.
pub fn seed(conn: &Connection, options: &SeedOptions) -> Result<(String, u64, u64)> {
    let mut rng = Rng::new(0x5EED_1234_ABCD_0001);

    let website_id = ensure_website(conn, &options.website, &options.domain)?;
    let now = chrono::Utc::now().naive_utc();
    let mut sessions = 0u64;
    let mut events = 0u64;

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

            for _ in 0..visitors {
                let (session, event_count) = make_session(&mut rng, &website_id, day_start, &pool);
                insert_session(conn, &website_id, &session)?;
                sessions += 1;

                for event in event_count {
                    insert_event(conn, &event, &website_id, &session)?;
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

    Ok((website_id, sessions, events))
}

/// A synthetic session.
struct SeedSession {
    id: String,
    visitor_id: String,
    created_at: chrono::NaiveDateTime,
    updated_at: chrono::NaiveDateTime,
    hostname: String,
    url: String,
    title: String,
    referrer: Option<String>,
    entry_url: String,
    browser: String,
    os: String,
    device: String,
    country: String,
    language: String,
    screen: String,
    views: i64,
    duration: i64,
}

/// A synthetic custom event.
struct SeedEvent {
    name: String,
    created_at: chrono::NaiveDateTime,
    url: String,
    data: String,
}

/// Build one synthetic session (and the events it fired).
fn make_session(
    rng: &mut Rng,
    website_id: &str,
    day_start: chrono::NaiveDateTime,
    pool: &[String],
) -> (SeedSession, Vec<SeedEvent>) {
    // The visitor id rotates every day, like the server's daily bucket.
    let day = day_start.format("%Y%m%d").to_string();
    let visitor_id = pool[rng.below(pool.len())].clone();

    let path = rng.weighted(PAGES);
    let referrer = rng.weighted(REFERRERS);

    // Most visitors look at one page; some browse a little.
    let views = match rng.below(100) {
        0..=44 => 1,
        45..=69 => 2,
        70..=84 => 3,
        85..=94 => 4,
        _ => rng.between(5, 9),
    };

    // Sessions last a little longer the more pages they have.
    let duration = views * rng.between(8, 55) + rng.between(0, 25);

    let start = day_start + chrono::Duration::seconds(rng.between(0, 86_399));
    let created_at = start;
    let updated_at = start + chrono::Duration::seconds(duration);

    let device = rng.weighted(DEVICES);
    let (browser, os, language, screen) = match device {
        "mobile" => {
            let browser = if rng.chance(55) { "Safari" } else { "Chrome" };
            let os = if rng.chance(55) { "iOS" } else { "Android" };
            let screen = if os == "iOS" { "390x844" } else { "412x915" };
            (browser, os, language(rng), screen)
        }
        "tablet" => ("Safari", "iPadOS", language(rng), "768x1024"),
        _ => {
            let browser = rng.weighted(BROWSERS);
            let os = rng.weighted(OPERATING_SYSTEMS);
            let screen = rng.weighted(SCREENS);
            (browser, os, language(rng), screen)
        }
    };

    let session = SeedSession {
        id: Uuid::new_v4().to_string(),
        visitor_id: format!("{day}-{visitor_id}"),
        created_at,
        updated_at,
        hostname: website_id.to_string(),
        url: path.to_string(),
        title: title_for(path).to_string(),
        referrer: if referrer.is_empty() {
            None
        } else {
            Some(referrer.to_string())
        },
        entry_url: path.to_string(),
        browser: browser.to_string(),
        os: os.to_string(),
        device: device.to_string(),
        country: rng.weighted(COUNTRIES).to_string(),
        language: language.to_string(),
        screen: screen.to_string(),
        views,
        duration,
    };

    // Some sessions fire a custom event.
    let mut events = Vec::new();
    if rng.chance(18) {
        let name = rng.weighted(EVENTS);
        events.push(SeedEvent {
            name: name.to_string(),
            created_at: created_at + chrono::Duration::seconds(rng.between(5, 300)),
            url: path.to_string(),
            data: "{\"source\":\"demo\"}".to_string(),
        });
    }

    (session, events)
}

/// Pick an interface language.
fn language(rng: &mut Rng) -> String {
    rng.weighted(LANGUAGES).to_string()
}

/// Insert one session.
fn insert_session(conn: &Connection, website_id: &str, session: &SeedSession) -> Result<()> {
    conn.execute(
        r#"
INSERT INTO session (
    id, website_id, visitor_id, created_at, updated_at, hostname, url, title, referrer, entry_url,
    browser, browser_version, os, os_version, device, country, region, city, language, screen,
    data, views, duration
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)
"#,
        duckdb::params![
            session.id,
            website_id,
            session.visitor_id,
            session.created_at,
            session.updated_at,
            session.hostname,
            session.url,
            session.title,
            session.referrer,
            session.entry_url,
            session.browser,
            "1.0",
            session.os,
            "1.0",
            session.device,
            session.country,
            Option::<String>::None,
            Option::<String>::None,
            session.language,
            session.screen,
            session.views,
            session.duration,
        ],
    )?;
    Ok(())
}

/// Insert one custom event.
fn insert_event(
    conn: &Connection,
    event: &SeedEvent,
    website_id: &str,
    session: &SeedSession,
) -> Result<()> {
    conn.execute(
        "INSERT INTO event (id, website_id, session_id, created_at, name, url, hostname, data)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        duckdb::params![
            Uuid::new_v4().to_string(),
            website_id,
            session.id,
            event.created_at,
            event.name,
            event.url,
            session.hostname,
            event.data,
        ],
    )?;
    Ok(())
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
         VALUES (?, ?, ?, ?, ?, 0, NULL)",
        duckdb::params![id, name, domain, now, now],
    )?;
    Ok(id)
}
