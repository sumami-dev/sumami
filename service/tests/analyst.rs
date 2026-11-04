//! The assistant: what it understands, and what it answers.
//!
//! The answers are checked against numbers that were put into the database by
//! hand — not against the numbers the assistant computed for itself.

use sumami_service::analyst::{self, Metric};
use sumami_service::models::{Hit, HitKind};
use sumami_service::pool::Db;
use sumami_service::storage::{Dimension, Storage};

/// A unique temporary database path.
fn temp_db(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sumami-analyst-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("sumami.duckdb")
}

/// A fixed "now", so the windows are the same on every run.
fn now() -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp(1_767_225_600, 0)
        .map(|d| d.naive_utc())
        .unwrap_or_default()
}

/// A hit, with the fields the tests care about.
fn hit(website: &str, session: &str, visitor: &str, url: &str, days_ago: i64) -> Hit {
    Hit {
        website_id: website.to_string(),
        session_id: sumami_service::ua::uuid_from_text(session),
        visitor_id: sumami_service::ua::uuid_from_text(visitor),
        retention_days: 0,
        kind: HitKind::Pageview,
        created_at: now() - chrono::Duration::days(days_ago),
        url: Some(url.to_string()),
        referrer: Some("news.ycombinator.com".to_string()),
        browser: Some("Chrome".to_string()),
        os: Some("macOS".to_string()),
        device: Some("desktop".to_string()),
        country: Some("DE".to_string()),
        language: Some("en-US".to_string()),
        screen: Some("1920x1080".to_string()),
        ..Hit::default()
    }
}

/// A database with a website and a known amount of traffic.
fn database(name: &str) -> (Db, String) {
    let path = temp_db(name);
    let db = Db::open(&path, "256MB").unwrap();
    let website = db
        .create_website("Example", Some("example.com"), 0, None)
        .unwrap()
        .id;

    // Two sessions in the last week: one with two page views, one with one.
    let hits = vec![
        hit(&website, "s1", "v1", "/one", 1),
        hit(&website, "s1", "v1", "/two", 1),
        hit(&website, "s2", "v2", "/one", 2),
        // And one in the week before, so there is something to compare with.
        hit(&website, "s3", "v3", "/one", 10),
    ];
    db.write_batch(&hits).unwrap();

    (db, website)
}

// --------------------------------------------------------------------------------------
// Reading the question
// --------------------------------------------------------------------------------------

/// The window comes from the question.
#[test]
fn the_window_is_read_from_the_question() {
    let now = now();

    let intent = analyst::parse("how many visitors last 30 days", now);
    assert_eq!(
        (intent.range.end - intent.range.start).num_days(),
        30,
        "the question asked for 30 days"
    );

    let intent = analyst::parse("how many visitors past 7 days", now);
    assert_eq!((intent.range.end - intent.range.start).num_days(), 7);

    // No window in the question: the last seven days.
    let intent = analyst::parse("how many visitors", now);
    assert_eq!((intent.range.end - intent.range.start).num_days(), 7);

    // The window ends now.
    let intent = analyst::parse("visitors last 30 days", now);
    assert_eq!(intent.range.end, now);
}

/// The metric comes from the question.
#[test]
fn the_metric_is_read_from_the_question() {
    let now = now();

    assert_eq!(analyst::parse("visitors", now).metric, Metric::Visitors);
    assert_eq!(analyst::parse("page views", now).metric, Metric::Pageviews);
    assert_eq!(analyst::parse("sessions", now).metric, Metric::Sessions);
    assert_eq!(analyst::parse("bounce rate", now).metric, Metric::Bounces);
    assert_eq!(
        analyst::parse("average visit duration", now).metric,
        Metric::Duration
    );
}

/// The dimension comes from the question.
#[test]
fn the_dimension_is_read_from_the_question() {
    let now = now();

    assert_eq!(
        analyst::parse("top pages", now).dimension,
        Some(Dimension::Url)
    );
    assert_eq!(
        analyst::parse("where do my visitors come from", now).dimension,
        Some(Dimension::Referrer)
    );
    assert_eq!(
        analyst::parse("which browsers", now).dimension,
        Some(Dimension::Browser)
    );
    assert_eq!(
        analyst::parse("what devices", now).dimension,
        Some(Dimension::Device)
    );
    assert_eq!(
        analyst::parse("which countries", now).dimension,
        Some(Dimension::Country)
    );
}

/// A question that compares with the period before is recognised.
#[test]
fn a_comparison_is_recognised() {
    let now = now();

    assert!(analyst::parse("how many visitors vs last week", now).compare);
    assert!(analyst::parse("why did traffic drop", now).compare);
    assert!(analyst::parse("why did traffic drop", now).why);
    assert!(!analyst::parse("how many visitors", now).compare);
}

/// A question in Chinese is understood, and answered in Chinese.
#[test]
fn a_chinese_question_is_understood() {
    let now = now();

    let intent = analyst::parse("最近 30 天的访客数", now);
    assert!(intent.chinese);
    assert_eq!(intent.metric, Metric::Visitors);
    assert_eq!((intent.range.end - intent.range.start).num_days(), 30);

    let intent = analyst::parse("访问量最多的页面", now);
    assert_eq!(intent.dimension, Some(Dimension::Url));

    let intent = analyst::parse("为什么流量下降了", now);
    assert!(intent.why);
    assert!(intent.compare);
}

// --------------------------------------------------------------------------------------
// Answering
// --------------------------------------------------------------------------------------

/// The answer holds the numbers that are in the database.
#[test]
fn the_answer_holds_the_numbers_of_the_database() {
    let (db, website) = database("numbers");
    let answer = analyst::ask(&db, &website, "how many page views", now()).unwrap();

    let summary = answer.facts.summary.as_ref().unwrap();
    assert_eq!(summary.pageviews.value, 3, "three page views in the window");
    assert_eq!(summary.visitors.value, 2, "two visitors");
    assert_eq!(summary.views.value, 2, "two sessions");
    assert_eq!(summary.bounces.value, 1, "one session with a single view");

    // The words hold the same numbers.
    assert!(
        answer.text.contains("3"),
        "the answer should hold the page views: {}",
        answer.text
    );
}

/// A question about a breakdown answers with the breakdown.
#[test]
fn a_breakdown_question_answers_with_the_breakdown() {
    let (db, website) = database("breakdown");
    let answer = analyst::ask(&db, &website, "top pages", now()).unwrap();

    let rows = answer.facts.breakdown.as_ref().unwrap();
    assert_eq!(rows.len(), 2, "two pages were visited");
    // The most visited page first.
    assert_eq!(rows[0].name, "/one");
    assert_eq!(rows[0].value, 2);
    assert_eq!(rows[1].name, "/two");
    assert_eq!(rows[1].value, 1);

    assert!(answer.text.contains("/one"), "{}", answer.text);
}

/// A question about a comparison answers with the change.
#[test]
fn a_comparison_question_answers_with_the_change() {
    let (db, website) = database("comparison");
    let answer = analyst::ask(&db, &website, "how many page views vs last week", now()).unwrap();

    // The last week has three page views, the week before had one.
    let summary = answer.facts.summary.as_ref().unwrap();
    assert_eq!(summary.pageviews.value, 3);
    assert!(
        summary.pageviews.change > 0.0,
        "the page views went up: {}",
        summary.pageviews.change
    );
    assert!(answer.text.contains('%'), "{}", answer.text);
}

/// A "why" question names the movers.
#[test]
fn a_why_question_names_the_movers() {
    let (db, website) = database("why");
    let answer = analyst::ask(&db, &website, "why did traffic drop", now()).unwrap();

    let movers = answer.facts.movers.as_ref().unwrap();
    assert!(!movers.is_empty(), "there should be movers");
    // The biggest change first.
    assert!(movers[0].change.abs() >= movers[movers.len() - 1].change.abs());
}

/// The assistant answers in the language of the question.
#[test]
fn the_answer_is_in_the_language_of_the_question() {
    let (db, website) = database("language");

    let english = analyst::ask(&db, &website, "how many page views", now()).unwrap();
    assert!(english.text.contains("page views"), "{}", english.text);

    let chinese = analyst::ask(&db, &website, "浏览量是多少", now()).unwrap();
    assert!(chinese.text.contains("浏览"), "{}", chinese.text);
}

/// The answer holds no number that is not in the database.
#[test]
fn the_answer_holds_no_invented_numbers() {
    let (db, website) = database("invented");
    let answer = analyst::ask(&db, &website, "how many page views", now()).unwrap();

    let summary = answer.facts.summary.as_ref().unwrap();
    let duration = summary.visit_duration.value;

    // The numbers the answer is allowed to hold: the five metrics, the parts of
    // the duration, and the length of the period.
    let allowed = [
        summary.pageviews.value.to_string(),
        summary.visitors.value.to_string(),
        summary.views.value.to_string(),
        summary.bounces.value.to_string(),
        duration.to_string(),
        (duration / 60).to_string(),
        (duration % 60).to_string(),
        ((answer.facts.end - answer.facts.start).num_days()).to_string(),
    ];

    for number in numbers(&answer.text) {
        assert!(
            allowed.contains(&number),
            "the answer holds a number that is not in the facts: {number} — {}",
            answer.text
        );
    }
}

/// The numbers in a piece of text.
fn numbers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_ascii_digit() {
            current.push(character);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}
