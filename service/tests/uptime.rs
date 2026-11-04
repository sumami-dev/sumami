//! The uptime checker: is the site down, or are we?
//!
//! The whole point of the feature is the second question, so the tests are
//! about **not confusing the two**:
//!
//! * the site answers → **up**;
//! * the site does not answer, and the references do → **the site is down**;
//! * neither answers → **we are offline**, and the check says so instead of
//!   blaming the site.
//!
//! The references are pointed at a local server, so the tests do not need the
//! internet and do not depend on whether the machine running them has any.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use sumimi_service::uptime::{Availability, Checker, UptimeCheck, Verdict};

/// A tiny HTTP server that answers everything with a status.
fn server(status: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Read the request (until the headers end), then answer.
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let answer = format!(
                "HTTP/1.1 {status} OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            );
            let _ = stream.write_all(answer.as_bytes());
            let _ = stream.flush();
        }
    });

    port
}

/// A port nothing is listening on.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// A checker whose references point at a local server.
///
/// The references have to answer for the check to say "the site is down" —
/// that is the whole mechanism, so the tests control it exactly.
fn checker(reference_port: u16) -> Checker {
    let mut checker = Checker::with_timeout(Duration::from_secs(2));
    checker.references = vec![format!("http://127.0.0.1:{reference_port}/")];
    checker.reference_address = format!("127.0.0.1:{reference_port}");
    checker
}

/// A site that answers is up.
#[test]
fn a_site_that_answers_is_up() {
    let port = server(200);
    let checker = checker(port);
    let verdict = checker.check(&format!("http://127.0.0.1:{port}/"));
    match verdict {
        Verdict::Up {
            status,
            latency_ms,
        } => {
            assert_eq!(status, 200);
            assert!(latency_ms < 5_000, "the answer was quick");
        }
        other => panic!("the site answered, so it is up: {other:?}"),
    }
}

/// A site that does not answer, while the references do, is **down**.
#[test]
fn a_site_that_does_not_answer_is_down() {
    let reference = server(200);
    let dead = closed_port();
    let checker = checker(reference);

    let verdict = checker.check(&format!("http://127.0.0.1:{dead}/"));
    match verdict {
        Verdict::Down { error } => {
            assert!(!error.is_empty(), "the failure is described");
        }
        other => panic!("the network was fine, so the site is down: {other:?}"),
    }
}

/// When the references do not answer either, **we** are the problem.
#[test]
fn a_network_that_does_not_answer_is_not_the_sites_fault() {
    let dead = closed_port();
    // The references point at a closed port too.
    let checker = checker(dead);

    let verdict = checker.check(&format!("http://127.0.0.1:{dead}/"));
    match verdict {
        Verdict::NetworkDown {
            error,
            references,
        } => {
            assert!(!error.is_empty());
            assert!(
                !references.is_empty(),
                "the references are named, so the owner can see why"
            );
        }
        other => panic!("our own network failed, so it is not the site's fault: {other:?}"),
    }
}

/// A server that answers with an error is up; one that answers with a server
/// error is down.
///
/// The line is at 500: a 404 means the server is there and the page is not
/// (that is up — the site is answering); a 503 means the server is broken, and
/// a site that answers every request with 503 is not up.
#[test]
fn a_server_error_is_down_and_a_not_found_is_up() {
    let port = server(404);
    let answering = checker(port);
    let verdict = answering.check(&format!("http://127.0.0.1:{port}/"));
    match verdict {
        Verdict::Up { status, .. } => assert_eq!(status, 404, "a 404 is still an answer"),
        other => panic!("the server answered, so it is up: {other:?}"),
    }

    let port = server(503);
    let broken = checker(port);
    let verdict = broken.check(&format!("http://127.0.0.1:{port}/"));
    match verdict {
        Verdict::Down { error } => assert!(error.contains("503"), "{error}"),
        other => panic!("the server is broken, so it is down: {other:?}"),
    }
}

/// The record says what happened.
#[test]
fn the_record_says_what_happened() {
    let port = server(200);
    let checker = checker(port);

    let check = checker.record("w1", &format!("http://127.0.0.1:{port}/"));
    assert_eq!(check.website_id, "w1");
    assert!(check.ok, "the site answered");
    assert_eq!(check.status, Some(200));
    assert!(check.latency_ms.is_some());
    assert!(check.error.is_none());
    assert_eq!(
        check.network_ok, None,
        "a site that answers needs no reference check"
    );

    // A site that does not answer, with a fine network.
    let dead = closed_port();
    let check = checker.record("w1", &format!("http://127.0.0.1:{dead}/"));
    assert!(!check.ok);
    assert_eq!(check.network_ok, Some(true), "the network was fine");
}

/// The availability counts the checks that could say something.
#[test]
fn the_availability_leaves_out_what_it_cannot_know() {
    let now = chrono::Utc::now().naive_utc();
    let check = |ok: bool, network_ok: Option<bool>| UptimeCheck {
        website_id: "w1".to_string(),
        at: now,
        ok,
        status: None,
        latency_ms: Some(100),
        error: None,
        network_ok,
    };

    // Ten checks, all of them up.
    let checks: Vec<UptimeCheck> = (0..10).map(|_| check(true, None)).collect();
    let availability = Availability::of(&checks);
    assert_eq!(availability.checks, 10);
    assert_eq!(availability.up, 10, "every one of them answered");
    assert_eq!(availability.percent, 100.0);
    assert_eq!(availability.unknown, 0);

    // Eight up, two down — the network was fine, so they count.
    let mut checks: Vec<UptimeCheck> = (0..8).map(|_| check(true, None)).collect();
    checks.push(check(false, Some(true)));
    checks.push(check(false, Some(true)));
    let availability = Availability::of(&checks);
    assert_eq!(availability.up, 8);
    assert_eq!(availability.percent, 80.0);
    assert_eq!(availability.unknown, 0);

    // Two of them could not say anything: they are left out of the count.
    let mut checks: Vec<UptimeCheck> = (0..8).map(|_| check(true, None)).collect();
    checks.push(check(false, Some(false)));
    checks.push(check(false, Some(false)));
    let availability = Availability::of(&checks);
    assert_eq!(availability.checks, 10);
    assert_eq!(availability.up, 8);
    assert_eq!(availability.unknown, 2);
    assert_eq!(
        availability.percent, 100.0,
        "the eight that answered were all up"
    );
}

/// A check that cannot say anything does not count against the site.
#[test]
fn a_check_we_could_not_make_does_not_count_against_the_site() {
    let now = chrono::Utc::now().naive_utc();
    let checks = vec![
        UptimeCheck {
            website_id: "w1".to_string(),
            at: now,
            ok: true,
            status: Some(200),
            latency_ms: Some(50),
            error: None,
            network_ok: None,
        },
        UptimeCheck {
            website_id: "w1".to_string(),
            at: now,
            ok: false,
            status: None,
            latency_ms: None,
            error: Some("we were offline".to_string()),
            network_ok: Some(false),
        },
    ];
    let availability = Availability::of(&checks);
    assert_eq!(availability.up, 1);
    assert_eq!(availability.unknown, 1);
    assert_eq!(
        availability.percent, 100.0,
        "the one check we could make said the site was up"
    );
}

/// The checker does not hang on a port that accepts but never answers.
#[test]
fn the_checker_does_not_hang_on_a_silent_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().unwrap().port();
    // A server that accepts and says nothing.
    thread::spawn(move || {
        for stream in listener.incoming() {
            let _stream = stream;
            thread::sleep(Duration::from_secs(30));
        }
    });

    let checker = Checker::with_timeout(Duration::from_millis(500));
    let started = std::time::Instant::now();
    let _ = checker.check(&format!("http://127.0.0.1:{port}/"));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the check gave up in time"
    );
}

/// A TCP connection to a closed port fails, so the reference check works.
#[test]
fn the_reference_check_notices_a_dead_network() {
    let dead = closed_port();
    let started = std::time::Instant::now();
    let failed = TcpStream::connect_timeout(
        &format!("127.0.0.1:{dead}").parse().unwrap(),
        Duration::from_secs(1),
    )
    .is_err();
    assert!(failed, "the port is closed");
    assert!(started.elapsed() < Duration::from_secs(2));
}
