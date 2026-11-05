//! The memory budget, measured in its own process.
//!
//! ```bash
//! cargo run --release --example memory_budget -- --engine 128MB
//! cargo run --release --example memory_budget -- --engine 256MB
//! ```
//!
//! One budget per process: the peak resident set is a high-water mark of the
//! whole process, so two budgets in one process would measure each other.
//!
//! The exit code is the assertion: **0** when the process stayed inside its
//! budget, **1** when it did not. The Linux CI job runs this twice — once per
//! tier — and goes red when the process outgrows its box.

use std::path::PathBuf;

use sumimi_service::config::budget;
use sumimi_service::memcheck;

/// How many events a run pushes.
const EVENTS: usize = 200_000;

fn main() {
    let mut engine = budget::RECOMMENDED_ENGINE.to_string();
    let mut events = EVENTS;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--engine" => engine = args.next().unwrap_or_default(),
            "--events" => {
                events = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(EVENTS)
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let ceiling = if engine == budget::MIN_ENGINE {
        budget::MIN_RSS
    } else {
        budget::RECOMMENDED_RSS
    };

    let dir: PathBuf = std::env::temp_dir().join(format!(
        "sumimi-budget-example-{}-{}",
        std::process::id(),
        engine.replace("MB", "mb")
    ));

    let report = match memcheck::run(&engine, events, &dir) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("the run failed: {err:#}");
            std::process::exit(2);
        }
    };

    println!("{}", report.summary());

    let _ = std::fs::remove_dir_all(&dir);

    let Some(peak) = report.peak_rss else {
        // Off Linux the exact high-water mark is not available — the sampled
        // maximum is printed above for information, but the budget is only
        // asserted where the number means what the VPS sees.
        println!("no exact peak RSS on this platform — run it on Linux to assert the budget");
        return;
    };

    let mb = peak as f64 / (1024.0 * 1024.0);
    let limit = ceiling as f64 / (1024.0 * 1024.0);
    if peak >= ceiling {
        println!("OVER BUDGET: the process peaked at {mb:.0} MB, the budget is {limit:.0} MB");
        std::process::exit(1);
    }
    println!("within budget: {mb:.0} MB of {limit:.0} MB");
}
