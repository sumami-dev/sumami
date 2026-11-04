//! The tracker and the server have to agree on the payload.
//!
//! The tracker is one plain JavaScript file and the server parses its JSON in
//! Rust. Nothing connects the two at compile time: if the tracker renames a
//! field, the server silently stores `NULL` and the column is gone — no error,
//! no warning, just an empty dashboard.
//!
//! So the two ends are checked against each other here: every field the server
//! reads must be a field the tracker sends.

use std::collections::BTreeSet;

/// The tracker source.
fn tracker() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("js/sumami.js");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("reading the tracker {}: {err}", path.display()))
}

/// The body of one function of the tracker.
fn function_body(source: &str, name: &str) -> String {
    let start = source
        .find(&format!("function {name}("))
        .unwrap_or_else(|| panic!("the tracker has no function {name}"));
    let mut depth = 0usize;
    for (index, character) in source[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..start + index].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("the function {name} is not closed");
}

/// The payload keys the tracker sends.
///
/// The keys of the object `basePayload` builds, plus the ones it assigns to it
/// afterwards.
fn tracker_keys() -> BTreeSet<String> {
    let source = tracker();
    let body = function_body(&source, "basePayload");
    let mut keys = BTreeSet::new();

    // The object literal: `website: …`, `url: …`, …
    for line in body.lines() {
        let line = line.trim();
        if let Some((key, _)) = line.split_once(':') {
            let key = key.trim();
            if !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !key.chars().next().unwrap().is_ascii_digit()
            {
                keys.insert(key.to_string());
            }
        }
    }

    // The fields assigned after the literal: `payload.lcp = …`, and the ones
    // behind a condition: `if (tag) payload.tag = tag;`.
    for name in ["basePayload", "track", "identify"] {
        let body = function_body(&source, name);
        for line in body.lines() {
            for part in line.split("payload.").skip(1) {
                let key: String = part
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !key.is_empty() {
                    keys.insert(key);
                }
            }
        }
    }

    keys
}

/// The payload fields the server reads out of a hit.
///
/// These are the fields `build_hit` looks up. A field that is not in the
/// tracker's payload is stored as `NULL` — silently.
fn server_fields() -> &'static [&'static str] {
    &[
        "website",
        "url",
        "referrer",
        "title",
        "hostname",
        "language",
        "screen",
        "name",
        "tag",
        "data",
        "lcp",
        "inp",
        "cls",
        "fcp",
        "ttfb",
    ]
}

/// The tracker sends every field the server reads.
#[test]
fn the_tracker_sends_every_field_the_server_reads() {
    let keys = tracker_keys();
    let mut missing: Vec<&str> = Vec::new();
    for field in server_fields() {
        if !keys.contains(*field) {
            missing.push(field);
        }
    }
    assert!(
        missing.is_empty(),
        "the tracker does not send {} — the server would store NULL for it",
        missing.join(", ")
    );
}

/// The server reads every field the tracker sends.
///
/// The other way round: a field the tracker sends and the server ignores is a
/// field somebody spent time on for nothing.
#[test]
fn the_server_reads_every_field_the_tracker_sends() {
    let keys = tracker_keys();
    // The fields that are not part of the payload: the code around it.
    let not_payload = [
        "www", "http", "https", "true", "false", "null", "undefined", "utf",
    ];
    let mut ignored: Vec<String> = Vec::new();
    for key in &keys {
        if server_fields().contains(&key.as_str()) {
            continue;
        }
        if not_payload.contains(&key.as_str()) {
            continue;
        }
        ignored.push(key.clone());
    }
    assert!(
        ignored.is_empty(),
        "the tracker sends {} but the server never reads them",
        ignored.join(", ")
    );
}

/// The web vitals are document-level: every page view of a document reports the
/// same numbers, so a route change cannot skew an average.
#[test]
fn the_vitals_are_frozen_per_document() {
    let source = tracker();

    // The metrics are read from the frozen snapshot, not from the live ones.
    assert!(
        source.contains("function documentLevelVitals()"),
        "the tracker must freeze the document's vitals"
    );
    assert!(
        source.contains("var metrics = documentLevelVitals();"),
        "the payload must carry the document's vitals"
    );
    assert!(
        !source.contains("payload.lcp = String(vitals.lcp);"),
        "the payload must not carry the live vitals — they belong to the document"
    );
}
