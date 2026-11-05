//! The commands of the command line: each one has to do what it says.
//!
//! These are the commands that work **offline** — against the instance in
//! `--dir`, without a server. Every one of them is checked here, because the
//! command line is the one interface a person uses without reading anything.

use std::path::PathBuf;
use std::process::Command;

use sumimi_service::models::Hit;
use sumimi_service::pool::Db;
use sumimi_service::storage::Storage;
use sumimi_service::testing::HitBuilder;

/// A unique temporary directory.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sumimi-commands-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run the command line and return what it said.
fn run(dir: &std::path::Path, args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_sumimi"))
        .arg("--dir")
        .arg(dir)
        .args(args)
        .output()
        .expect("the command runs");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// An instance with a website and ten page views.
fn instance(dir: &std::path::Path) -> String {
    let db = Db::open(&dir.join("data.duckdb"), "256MB").unwrap();
    let website = db
        .create_website("Example", Some("example.com"), 0, None)
        .unwrap()
        .id;
    let hits: Vec<Hit> = (0..10)
        .map(|index| HitBuilder::new(&website, "s1", "v1").url(&format!("/{index}")).build())
        .collect();
    db.write_batch(&hits).unwrap();
    website
}

/// `seed` fills the instance with synthetic traffic.
#[test]
fn the_seed_fills_the_instance() {
    let dir = temp_dir("seed");

    let (ok, output) = run(
        &dir,
        &["seed", "--website", "Seeded", "--domain", "seeded.example", "--days", "3", "--visitors", "5"],
    );
    assert!(ok, "the command failed: {output}");
    assert!(
        output.contains("seeded") && output.contains("page views"),
        "the seed says what it did: {output}"
    );

    // And the websites are there afterwards.
    let (ok, output) = run(&dir, &["websites"]);
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("Seeded"), "{output}");
}

/// The reports: the pages, the referrers, the time series.
#[test]
fn the_reports_answer() {
    let dir = temp_dir("reports");
    let website = instance(&dir);

    for report in ["pages", "referrers", "browsers", "countries", "timeseries", "sessions"] {
        let (ok, output) = run(&dir, &[report, &website]);
        assert!(ok, "{report} failed: {output}");
    }

    // The pages are the ten that were written.
    let (ok, output) = run(&dir, &["pages", &website]);
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("/0"), "{output}");
}

/// `export` writes the report as JSON or as CSV.
#[test]
fn the_export_writes_the_report() {
    let dir = temp_dir("export");
    let website = instance(&dir);

    let (ok, output) = run(
        &dir,
        &["export", &website, "pages", "--format", "json"],
    );
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("/0"), "{output}");

    let (ok, output) = run(
        &dir,
        &["export", &website, "pages", "--format", "csv"],
    );
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("name"), "the CSV has a header: {output}");
    assert!(output.contains("/0"), "{output}");
}

/// `prune` throws away what the retention window says.
#[test]
fn the_prune_throws_the_old_data_away() {
    let dir = temp_dir("prune");
    let website = instance(&dir);

    let (ok, output) = run(&dir, &["prune", &website]);
    assert!(ok, "the command failed: {output}");
    assert!(
        output.contains("removed") && output.contains("events"),
        "the prune says what it removed: {output}"
    );
}

/// `erase` deletes a website and all of its data.
#[test]
fn the_erase_deletes_the_website() {
    let dir = temp_dir("erase");
    let website = instance(&dir);

    // Without `--yes` it asks — and the answer has to be the website id.
    let output = Command::new(env!("CARGO_BIN_EXE_sumimi"))
        .arg("--dir")
        .arg(&dir)
        .args(["erase", &website])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the command runs");
    assert!(
        !output.status.success(),
        "an erase without an answer does not go through"
    );

    // With `--yes` it does.
    let (ok, output) = run(&dir, &["erase", &website, "--yes"]);
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("removed"), "{output}");

    // And the website is gone.
    let (ok, output) = run(&dir, &["websites"]);
    assert!(ok, "the command failed: {output}");
    assert!(!output.contains(&website), "the website is gone: {output}");
}

/// The Search Console import, offline: the rows land in the instance store.
#[test]
fn the_search_console_import_lands_in_the_instance() {
    let dir = temp_dir("gsc");
    let website = instance(&dir);

    let export = dir.join("search-console.csv");
    std::fs::write(
        &export,
        "Date,Query,Page,Clicks,Impressions,CTR,Position\n\
         2026-01-15,sumimi analytics,/blog,3,120,2.5%,4.2\n\
         2026-01-16,sumimi cli,/docs,1,40,2.5%,9.0\n",
    )
    .unwrap();

    let (ok, output) = run(
        &dir,
        &["gsc", "import", export.to_str().unwrap(), "--website", &website],
    );
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("imported"), "{output}");

    // And they can be listed.
    let (ok, output) = run(&dir, &["gsc", "list", "--website", &website]);
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("sumimi analytics"), "{output}");
}

/// The uptime endpoints, offline: they are watched, listed and dropped.
#[test]
fn the_uptime_endpoints_are_managed() {
    let dir = temp_dir("uptime");
    let website = instance(&dir);

    let (ok, output) = run(
        &dir,
        &["uptime", "--website", &website, "--add", "https://example.com/health"],
    );
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("watching"), "{output}");

    let (ok, output) = run(&dir, &["uptime", "--website", &website, "--list"]);
    assert!(ok, "the command failed: {output}");
    assert!(output.contains("https://example.com/health"), "{output}");

    let (ok, output) = run(
        &dir,
        &[
            "uptime",
            "--website",
            &website,
            "--remove",
            "https://example.com/health",
        ],
    );
    assert!(ok, "the command failed: {output}");

    let (ok, output) = run(&dir, &["uptime", "--website", &website, "--list"]);
    assert!(ok, "the command failed: {output}");
    assert!(!output.contains("https://example.com/health"), "{output}");
}

/// A URL to check, with no website: the check runs and says what it found.
///
/// The check reaches out to the network, so it is the one command that is not
/// hermetic — it is here because the answer has to say *which* URL failed, not
/// because the network is reliable.
#[test]
fn a_url_to_check_is_answered() {
    let dir = temp_dir("check");

    let (ok, output) = run(&dir, &["uptime", "https://127.0.0.1:1/never"]);
    assert!(ok, "the command failed: {output}");
    assert!(
        output.contains("127.0.0.1:1"),
        "the answer names the URL that was checked: {output}"
    );
}
