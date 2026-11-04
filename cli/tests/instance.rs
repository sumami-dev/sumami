//! The instance directory: what lives in it, and what the CLI finds there.
//!
//! The instance is a **directory**: the data file, the control plane and the
//! secret all live inside it. Every command has to agree on where that is —
//! and the CLI's `--dir` flag has to be the same answer the seed used, or the
//! CLI seeds one directory and reads another.

use std::path::PathBuf;

use sumami_cli::{backup, seed};
use sumami_service::config::Paths;
use sumami_service::pool::Db;
use sumami_service::storage::{ReportQuery, Storage, TimeRange};

/// A unique temporary directory.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sumami-instance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The instance directory holds everything, and the CLI finds it by `--dir`.
#[test]
fn the_instance_directory_holds_the_data_and_the_cli_finds_it() {
    let dir = temp_dir("layout");
    let paths = Paths::new(Some(dir.clone()), None, "duckdb");

    assert_eq!(paths.data, dir.join("data.duckdb"), "the data file");
    assert_eq!(
        paths.instance,
        dir.join("instance.sqlite"),
        "the control plane"
    );
    assert_eq!(paths.secret, dir.join("jwt-secret"), "the secret");
    assert_eq!(paths.logs, dir.join("logs"), "the logs");
    assert_eq!(paths.acme, dir.join("acme"), "the certificates");

    // The SQLite backend puts its data file there too.
    let sqlite = Paths::new(Some(dir.clone()), None, "sqlite");
    assert_eq!(sqlite.data, dir.join("data.sqlite"));
}

/// A backup must contain the rows — not an empty database.
///
/// The backup reads the file **inside the instance directory**. When it looked
/// at the default path instead, it archived nothing and said so with a cheerful
/// "archived 0 rows" — which reads like a successful backup of an empty site.
#[test]
fn a_backup_of_an_instance_directory_contains_its_rows() {
    let dir = temp_dir("backup");
    let paths = Paths::new(Some(dir.clone()), None, "duckdb");

    let db = Db::open(&paths.data, "256MB").unwrap();
    let (website, views, _events) = seed::seed(
        &db,
        &seed::SeedOptions {
            website: "Example".to_string(),
            domain: "example.com".to_string(),
            days: 3,
            visitors_per_day: 50,
        },
    )
    .unwrap();
    assert!(views > 0, "the seed wrote something");

    // The backup.
    let archive = dir.join("archive");
    let stats = db
        .with_write(|conn| backup::export(conn, &archive, None, None, None, false))
        .unwrap();
    assert!(
        stats.rows >= views as i64,
        "the archive holds the rows: {} (the seed wrote {views})",
        stats.rows
    );
    assert!(stats.files > 0, "the archive holds files");

    // And the archive answers the same as the database.
    let range = TimeRange {
        start: chrono::DateTime::from_timestamp(0, 0)
            .map(|d| d.naive_utc())
            .unwrap(),
        end: chrono::DateTime::from_timestamp(4_000_000_000, 0)
            .map(|d| d.naive_utc())
            .unwrap(),
    };
    let hot = db.summary(&ReportQuery::new(&website, range)).unwrap();
    let cold = backup::open_archive(&format!("{}/", archive.display())).unwrap();
    let cold = cold.summary(&ReportQuery::new(&website, range)).unwrap();
    assert_eq!(hot.pageviews, cold.pageviews, "the same page views");
    assert_eq!(hot.visitors, cold.visitors, "the same visitors");
    assert_eq!(hot.views, cold.views, "the same sessions");
}
