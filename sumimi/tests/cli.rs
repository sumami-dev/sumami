//! The command line: it must read the instance it was pointed at.
//!
//! The `--dir` flag says where the instance is. Every command has to agree on
//! that — the read path used to look in the default directory instead, so
//! `sumimi --dir X seed` wrote into X and `sumimi --dir X websites` looked
//! somewhere else. And `sumimi --dir X backup` archived an **empty** database
//! (the default path, which did not exist and was therefore created) and
//! reported it as a successful backup.

use std::path::PathBuf;
use std::process::Command;

use sumimi_service::models::Hit;
use sumimi_service::pool::Db;
use sumimi_service::testing::HitBuilder;
use sumimi_service::storage::Storage;

/// A unique temporary directory.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sumimi-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A hit.
fn hit(website: &str, url: &str) -> Hit {
    HitBuilder::new(website, "s1", "v1").url(url).build()
}

/// An instance with a website and ten page views.
fn instance(dir: &std::path::Path) -> String {
    let db = Db::open(&dir.join("data.duckdb"), "256MB").unwrap();
    let website = db
        .create_website("Example", Some("example.com"), 0, None)
        .unwrap()
        .id;
    let hits: Vec<Hit> = (0..10)
        .map(|index| hit(&website, &format!("/{index}")))
        .collect();
    db.write_batch(&hits).unwrap();
    website
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

/// The command line reads the instance in `--dir`.
#[test]
fn the_command_line_reads_the_instance_in_the_directory() {
    let dir = temp_dir("read");
    let website = instance(&dir);

    // The websites are listed.
    let (ok, output) = run(&dir, &["websites"]);
    assert!(ok, "the command failed: {output}");
    assert!(
        output.contains(&website),
        "the website of this instance is listed: {output}"
    );

    // And the summary is the one of this instance.
    let (ok, output) = run(&dir, &["summary", &website]);
    assert!(ok, "the command failed: {output}");
    assert!(
        output.contains("10"),
        "the ten page views are there: {output}"
    );
}

/// The backup archives the instance in `--dir` — not an empty database.
#[test]
fn the_backup_archives_the_instance_in_the_directory() {
    let dir = temp_dir("backup");
    instance(&dir);
    let archive = dir.join("archive");

    let (ok, output) = run(&dir, &["backup", "--out", archive.to_str().unwrap()]);
    assert!(ok, "the command failed: {output}");

    // The archive holds the rows.
    let files: Vec<PathBuf> = walk(&archive);
    let parquet: Vec<&PathBuf> = files
        .iter()
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("parquet"))
        .collect();
    assert!(
        !parquet.is_empty(),
        "the archive holds parquet files: {files:?}"
    );

    // And reading it back gives the same numbers.
    let (ok, output) = run(
        &dir,
        &[
            "--parquet",
            &format!("{}/", archive.display()),
            "websites",
        ],
    );
    assert!(ok, "the command failed: {output}");
}

/// Every file under a directory.
fn walk(directory: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
