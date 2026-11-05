//! The memory budget — the two tiers the product is specified against.
//!
//! | | the machine | the engine's ceiling | the **process** must stay under |
//! | --- | --- | --- | --- |
//! | minimum | 512 MB (a 0.5G VPS) | 128 MB | 256 MB |
//! | recommended | 1 GB (a normal VPS) | 256 MB | 512 MB |
//!
//! The assertion is against the **process's** peak resident set, not the
//! engine's own number: what a small box runs out of is the whole process —
//! the code pages, the writer's queue, the pages the allocator keeps — and the
//! engine's share is only a part of it.
//!
//! The budget is chosen by the environment, so the same test runs against both
//! tiers:
//!
//! ```bash
//! SUMIMI_DUCKDB_MEMORY_LIMIT=128MB cargo test -p sumimi-service --test memory_budget
//! SUMIMI_DUCKDB_MEMORY_LIMIT=256MB cargo test -p sumimi-service --test memory_budget
//! ```
//!
//! **The RSS assertion needs Linux.** There the peak is read from
//! `/proc/self/status` (`VmHWM`). On macOS the number is not the number a VPS
//! sees (the shared pages of every mapped binary are counted differently), so
//! the assertion is skipped there and the Linux CI does it.

use std::path::PathBuf;

use sumimi_service::config::budget;
use sumimi_service::memcheck;

/// How many events a run pushes.
///
/// "Real scale" for a small box: a few hundred thousand page views, with
/// realistic repetition (the same pages, the same visitors) so the storage
/// engine compresses what it would really compress.
const EVENTS: usize = 200_000;

/// The tier the environment asks for.
fn tier() -> (&'static str, &'static str, u64) {
    let configured = std::env::var("SUMIMI_DUCKDB_MEMORY_LIMIT")
        .unwrap_or_else(|_| budget::RECOMMENDED_ENGINE.to_string());
    if configured == budget::MIN_ENGINE {
        (budget::MIN_ENGINE, "the minimum requirement", budget::MIN_RSS)
    } else {
        (
            budget::RECOMMENDED_ENGINE,
            "the recommended setup",
            budget::RECOMMENDED_RSS,
        )
    }
}

/// The whole process fits in the budget.
///
/// This is the test that would fail if the memory story changed: a bigger
/// buffer, a bigger queue, a query that holds its hash table in memory, an
/// allocator that never gives anything back.
#[test]
fn the_process_fits_in_the_memory_budget() {
    let (engine, name, ceiling) = tier();
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "sumimi-budget-{}-{}",
        std::process::id(),
        engine.replace("MB", "mb")
    ));

    let report = memcheck::run(engine, EVENTS, &dir).unwrap();

    println!(
        "memory budget ({name}, {engine} engine): {}",
        report.summary()
    );

    // Nothing may be dropped: a dropped event would make the measurement a lie.
    assert_eq!(
        report.dropped, 0,
        "the queue overflowed — the measurement would not cover the events it claims to"
    );
    assert!(
        report.flushes > 0,
        "nothing was written — the run did not measure a write path"
    );

    // The functional side: the events are all there.
    assert_eq!(report.events, EVENTS);

    // The budget itself.
    if let Some(peak) = report.peak_rss {
        assert!(
            peak < ceiling,
            "the process peaked at {:.0} MB, over the {} MB budget for {name}\n  {}\n  \
             (the engine itself was holding {:.1} MB)",
            peak as f64 / (1024.0 * 1024.0),
            ceiling / (1024 * 1024),
            report.summary(),
            report.engine_bytes as f64 / (1024.0 * 1024.0),
        );
    } else {
        println!("no peak RSS on this platform — the budget is asserted on Linux CI");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// The two tiers are the two the documentation promises.
#[test]
fn the_two_budgets_are_the_documented_ones() {
    assert_eq!(budget::MIN_ENGINE, "128MB");
    assert_eq!(budget::MIN_RSS, 256 * 1024 * 1024);
    assert_eq!(budget::RECOMMENDED_ENGINE, "256MB");
    assert_eq!(budget::RECOMMENDED_RSS, 512 * 1024 * 1024);

    // The engine's ceiling is a quarter of the machine, capped at 256 MB.
    assert_eq!(
        sumimi_service::config::MEMORY_LIMIT_MAX,
        budget::RECOMMENDED_RSS / 2
    );
}

/// The thread count follows the budget: a parallel `COUNT(DISTINCT …)` builds
/// one hash table per thread, so the memory a query needs scales with it.
#[test]
fn the_threads_follow_the_budget() {
    assert_eq!(sumimi_service::config::threads_for(budget::MIN_ENGINE, 64), 2);
    assert_eq!(
        sumimi_service::config::threads_for(budget::RECOMMENDED_ENGINE, 64),
        4
    );
    // Never more than the machine has cores.
    assert_eq!(sumimi_service::config::threads_for(budget::RECOMMENDED_ENGINE, 2), 2);
}
