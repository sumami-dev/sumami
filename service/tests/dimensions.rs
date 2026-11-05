//! The dimensions: every way the traffic can be broken down.
//!
//! The tests put events in by hand and ask for each breakdown — the numbers
//! have to come out of the database, not out of the code that wrote them.

use sumimi_service::models::{Hit, HitKind};
use sumimi_service::pool::Db;
use sumimi_service::storage::{parse_query_parameters, Dimension, ReportQuery, Storage, TimeRange};

/// A temporary database with one website and a known amount of traffic.
fn database(name: &str) -> (Db, String) {
    let dir = std::env::temp_dir().join(format!("sumimi-dimensions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("sumimi.duckdb"), "256MB").unwrap();

    let website = db
        .create_website("Example", Some("example.com"), 0, None)
        .unwrap()
        .id;

    let now = chrono::Utc::now().naive_utc();
    let hits = vec![
        hit(&website, "/one", "google.com", "DE", "Berlin", "example.com", "tag-a", "a=1&b=2", now),
        hit(&website, "/two", "google.com", "DE", "Berlin", "example.com", "tag-a", "a=1&b=3", now),
        hit(&website, "/one", "news.ycombinator.com", "FR", "Paris", "other.example", "tag-b", "a=2", now),
    ];
    db.write_batch(&hits).unwrap();

    (db, website)
}

/// One page view, with every dimension filled in.
#[allow(clippy::too_many_arguments)]
fn hit(
    website: &str,
    url: &str,
    referrer: &str,
    country: &str,
    city: &str,
    hostname: &str,
    tag: &str,
    query: &str,
    now: chrono::NaiveDateTime,
) -> Hit {
    Hit {
        website_id: website.to_string(),
        session_id: sumimi_service::ua::uuid_from_text("s1"),
        visitor_id: sumimi_service::ua::uuid_from_text(&format!("{tag}-{url}")),
        retention_days: 0,
        kind: HitKind::Pageview,
        created_at: now,
        url: Some(url.to_string()),
        url_query: Some(query.to_string()),
        referrer: Some(referrer.to_string()),
        hostname: Some(hostname.to_string()),
        channel: Some(
            sumimi_service::channels::channel(&sumimi_service::channels::ChannelInput::new(
                Some(referrer),
                Some(query),
                None,
                None,
                Some(hostname),
            ))
            .to_string(),
        ),
        country: Some(country.to_string()),
        region: Some(country.to_string()),
        city: Some(city.to_string()),
        tag: Some(tag.to_string()),
        ..Hit::default()
    }
}

/// The window the tests ask about.
fn window() -> TimeRange {
    TimeRange {
        start: chrono::Utc::now().naive_utc() - chrono::Duration::days(7),
        end: chrono::Utc::now().naive_utc() + chrono::Duration::days(1),
    }
}

/// The query for a website.
fn query(website: &str) -> ReportQuery {
    ReportQuery::new(website, window())
}

/// Every breakdown has to answer.
#[test]
fn every_dimension_answers() {
    let (db, website) = database("dimensions");
    let query = query(&website);

    for dimension in Dimension::all() {
        let rows = db.breakdown(&query, dimension).expect("the breakdown runs");
        assert!(!rows.is_empty(), "{dimension:?} answered with nothing");
    }
}

/// The traffic channels come out of the referrer.
#[test]
fn the_channels_come_out_of_the_referrer() {
    let (db, website) = database("channels");
    let rows = db.breakdown(&query(&website), Dimension::Channel).unwrap();

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert!(names.contains(&"Search"), "{names:?}");
    assert!(names.contains(&"Social"), "{names:?}");

    // Two of the three visits came from a search engine.
    let search = rows.iter().find(|row| row.name == "Search").unwrap();
    assert_eq!(search.value, 2);
}

/// The region, the city and the host are what was stored.
#[test]
fn the_place_and_the_host_are_what_was_stored() {
    let (db, website) = database("places");

    let regions = db.breakdown(&query(&website), Dimension::Region).unwrap();
    assert_eq!(regions.len(), 2);
    assert!(regions.iter().any(|row| row.name == "DE"));

    let cities = db.breakdown(&query(&website), Dimension::City).unwrap();
    assert!(cities.iter().any(|row| row.name == "Berlin"));

    let hosts = db.breakdown(&query(&website), Dimension::Host).unwrap();
    assert_eq!(hosts.len(), 2);
    assert!(hosts.iter().any(|row| row.name == "example.com"));
}

/// The tags are what was stored.
#[test]
fn the_tags_are_what_was_stored() {
    let (db, website) = database("tags");
    let rows = db.breakdown(&query(&website), Dimension::Tag).unwrap();

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert!(names.contains(&"tag-a"), "{names:?}");
    assert!(names.contains(&"tag-b"), "{names:?}");
}

/// The query parameters are the ones of the visited pages.
#[test]
fn the_query_parameters_are_the_ones_of_the_pages() {
    let (db, website) = database("parameters");
    let rows = parse_query_parameters(&db.query_strings(&query(&website)).unwrap());

    // `a` was on every page, `b` on two of them. The count is per name **and**
    // value: `a=1` twice and `a=2` once.
    let a: i64 = rows.iter().filter(|row| row.name == "a").map(|row| row.count).sum();
    assert_eq!(a, 3, "{rows:?}");

    let b: i64 = rows.iter().filter(|row| row.name == "b").map(|row| row.count).sum();
    assert_eq!(b, 2, "{rows:?}");

    // The values are kept apart.
    let values: Vec<&str> = rows
        .iter()
        .filter(|row| row.name == "a")
        .map(|row| row.value.as_str())
        .collect();
    assert!(values.contains(&"1"), "{values:?}");
    assert!(values.contains(&"2"), "{values:?}");
}

/// The session log carries the channel too.
#[test]
fn the_session_log_carries_the_channel() {
    let (db, website) = database("sessions");
    let sessions = db.sessions(&query(&website)).unwrap();
    assert!(!sessions.is_empty());
    assert!(sessions.iter().any(|session| session.channel.is_some()));
}
