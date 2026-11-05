//! The traffic builder: the rules the tests and the benchmarks share.
//!
//! Every test that puts events in by hand used to write the same twenty lines
//! of struct literal. They all went through one builder instead — and this is
//! what that builder promises.

use sumimi_service::models::HitKind;
use sumimi_service::testing::HitBuilder;

/// A page view, with only what the caller named.
#[test]
fn a_builder_names_only_what_the_caller_cares_about() {
    let hit = HitBuilder::new("w1", "s1", "v1").url("/one").build();

    assert_eq!(hit.website_id, "w1");
    assert_eq!(hit.url.as_deref(), Some("/one"));

    // Everything else keeps the defaults it has everywhere else.
    assert_eq!(hit.title, None);
    assert_eq!(hit.referrer, None);
    assert_eq!(hit.country, None);
    assert_eq!(hit.visit_id, None);
    assert_eq!(hit.retention_days, 0);
    assert_eq!(hit.kind, HitKind::Pageview);
}

/// The same text always means the same session.
#[test]
fn the_same_name_means_the_same_session() {
    let one = HitBuilder::new("w1", "s1", "v1").build();
    let two = HitBuilder::new("w1", "s1", "v1").build();
    let other_session = HitBuilder::new("w1", "s2", "v1").build();
    let other_visitor = HitBuilder::new("w1", "s1", "v2").build();

    assert_eq!(one.session_id, two.session_id, "the same session");
    assert_eq!(one.visitor_id, two.visitor_id, "the same visitor");
    assert_ne!(one.session_id, other_session.session_id, "a different session");
    assert_ne!(
        one.visitor_id, other_visitor.visitor_id,
        "a different visitor"
    );
}

/// An empty text means the field is not there.
///
/// This is the rule the collector applies to the fields it reads off the wire,
/// so a caller that has nothing to say says nothing.
#[test]
fn an_empty_text_means_the_field_is_not_there() {
    let hit = HitBuilder::new("w1", "s1", "v1")
        .url("/one")
        .title("")
        .referrer(String::new())
        .build();

    assert_eq!(hit.url.as_deref(), Some("/one"));
    assert_eq!(hit.title, None);
    assert_eq!(hit.referrer, None);
}

/// The moment it happened.
#[test]
fn the_moment_is_the_one_that_was_asked_for() {
    let at = chrono::NaiveDate::from_ymd_opt(2026, 3, 1)
        .unwrap()
        .and_hms_opt(12, 30, 0)
        .unwrap();

    let hit = HitBuilder::new("w1", "s1", "v1").at(at).build();
    assert_eq!(hit.created_at, at);
}

/// The kind of hit it is.
#[test]
fn the_kind_is_the_one_that_was_asked_for() {
    let hit = HitBuilder::new("w1", "s1", "v1")
        .kind(HitKind::Event)
        .name("signup")
        .build();

    assert_eq!(hit.kind, HitKind::Event);
    assert_eq!(hit.name.as_deref(), Some("signup"));
}

/// The visit, when there is one.
#[test]
fn the_visit_is_named_like_the_session() {
    let hit = HitBuilder::new("w1", "s1", "v1").visit("visit-1").build();
    assert!(hit.visit_id.is_some());

    let same = HitBuilder::new("w1", "s1", "v1").visit("visit-1").build();
    assert_eq!(hit.visit_id, same.visit_id);
}

/// The numbers are numbers.
#[test]
fn the_numbers_are_stored_as_numbers() {
    let hit = HitBuilder::new("w1", "s1", "v1")
        .latitude(52.52)
        .longitude(13.405)
        .lcp(600.0)
        .build();

    assert_eq!(hit.latitude, Some(52.52));
    assert_eq!(hit.longitude, Some(13.405));
    assert_eq!(hit.lcp, Some(600.0));
}
