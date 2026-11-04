//! Parquet export and archive reading.
//!
//! The hot database is a DuckDB file that the server holds an exclusive lock on — you
//! cannot copy it while the service runs, and copying a 400 GB file is not a backup
//! strategy anyway. So the cold tier is **Parquet**: one file per day per table,
//! written once, never rewritten, readable by anything (DuckDB, Spark, Polars, pandas).
//!
//! ```text
//! sumimi backup --out ./archive                 # write the archive
//! sumimi summary --parquet './archive/*'        # analyse the archive
//! ```

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use duckdb::Connection;

/// What an export produced.
#[derive(Debug, Default)]
pub struct BackupStats {
    /// How many day-files were written.
    pub files: usize,
    /// How many rows were written.
    pub rows: i64,
    /// The directory the archive was written to.
    pub directory: PathBuf,
}

/// Write the hot database to a Parquet archive, partitioned by day.
///
/// One pass per table: DuckDB's `PARTITION_BY (day)` writes one directory per day
/// (`sessions/day=2026-11-03/…`) in a single scan. Scanning per day would be O(days).
///
/// The archive is additive and self-describing: the partition column is part of the
/// file, so any reader (DuckDB, Spark, Polars, pandas) can read it without knowing
/// our layout.
pub fn export(
    conn: &Connection,
    out: &Path,
    website: Option<&str>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    skip_existing: bool,
) -> Result<BackupStats> {
    let mut stats = BackupStats {
        directory: out.to_path_buf(),
        ..Default::default()
    };

    for (table, sub) in [("event", "events"), ("revenue", "revenue")] {
        let directory = out.join(sub);
        std::fs::create_dir_all(&directory)?;

        if skip_existing && has_files(&directory) {
            continue;
        }

        // A table with no rows is not an error: a site without custom events simply
        // has no event archive.
        let (min, max) = match range_of(conn, table, website) {
            Ok(range) => range,
            Err(_) => continue,
        };
        let first = from.unwrap_or(min);
        let last = to.unwrap_or(max);

        let filter = match website {
            Some(_) => "website_id = ? AND created_at >= ? AND created_at < ?",
            None => "created_at >= ? AND created_at < ?",
        };

        let count_sql = format!("SELECT count(*) FROM {table} WHERE {filter}");
        let count: i64 = match website {
            Some(id) => conn.query_row(
                &count_sql,
                duckdb::params![id, start_of(first), start_of(last + chrono::Duration::days(1))],
                |row| row.get(0),
            )?,
            None => conn.query_row(
                &count_sql,
                duckdb::params![start_of(first), start_of(last + chrono::Duration::days(1))],
                |row| row.get(0),
            )?,
        };

        if count == 0 {
            continue;
        }

        // One scan, partitioned by day.
        let copy_sql = format!(
            "COPY (
                 SELECT *, strftime(created_at, '%Y-%m-%d') AS day
                 FROM {table}
                 WHERE {filter}
             ) TO ? (FORMAT PARQUET, COMPRESSION ZSTD, PARTITION_BY (day))"
        );

        match website {
            Some(id) => conn.execute(
                &copy_sql,
                duckdb::params![
                    id,
                    start_of(first),
                    start_of(last + chrono::Duration::days(1)),
                    directory.to_string_lossy()
                ],
            )?,
            None => conn.execute(
                &copy_sql,
                duckdb::params![
                    start_of(first),
                    start_of(last + chrono::Duration::days(1)),
                    directory.to_string_lossy()
                ],
            )?,
        };

        stats.rows += count;
        stats.files += count_partitions(&directory);
    }

    // The website table is tiny; keep it so an archive can be listed.
    let websites_file = out.join("websites.parquet");
    if !websites_file.exists() {
        conn.execute(
            "COPY (SELECT * FROM website) TO ? (FORMAT PARQUET, COMPRESSION ZSTD)",
            [&websites_file.to_string_lossy()],
        )
        .context("writing the website table")?;
    }

    Ok(stats)
}

/// Midnight of a day.
fn start_of(day: NaiveDate) -> chrono::NaiveDateTime {
    day.and_hms_opt(0, 0, 0).unwrap()
}

/// Does a directory hold any parquet file?
fn has_files(directory: &Path) -> bool {
    fn walk(directory: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return false;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_dir() {
                if walk(&path) {
                    return true;
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("parquet") {
                return true;
            }
        }
        false
    }
    walk(directory)
}

/// How many day-partitions were written?
fn count_partitions(directory: &Path) -> usize {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

/// The first and last day that have data in a table.
fn range_of(
    conn: &Connection,
    table: &str,
    website: Option<&str>,
) -> Result<(NaiveDate, NaiveDate)> {
    let (min, max): (Option<chrono::NaiveDateTime>, Option<chrono::NaiveDateTime>) = match website {
        Some(id) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT min(created_at), max(created_at) FROM {table} WHERE website_id = ?"
            ))?;
            let mut rows = stmt.query([id])?;
            match rows.next()? {
                Some(row) => (row.get(0)?, row.get(1)?),
                None => (None, None),
            }
        }
        None => {
            let mut stmt =
                conn.prepare(&format!("SELECT min(created_at), max(created_at) FROM {table}"))?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => (row.get(0)?, row.get(1)?),
                None => (None, None),
            }
        }
    };

    let min = min
        .map(|t| t.date())
        .ok_or_else(|| anyhow!("there is no data to archive"))?;
    let max = max
        .map(|t| t.date())
        .ok_or_else(|| anyhow!("there is no data to archive"))?;
    Ok((min, max))
}

/// Open a Parquet archive as a database the reports can query.
///
/// The archive is read through `read_parquet()` — it is never imported into the hot
/// database, so analysing ten years of history costs nothing but the query itself.
pub fn open_archive(pattern: &str) -> Result<sumimi_service::pool::Db> {
    let conn = Connection::open_in_memory()?;

    let escape = |value: &str| value.replace('\'', "''");

    // The archive layout: <root>/events/*.parquet, <root>/revenue/*.parquet.
    let root = pattern.trim_end_matches("/*").trim_end_matches('/');
    for (table, sub) in [("event", "events"), ("revenue", "revenue")] {
        let directory = format!("{root}/{sub}");
        // A table with no rows has no file at all — read_parquet would fail on an
        // empty glob, so the view is only created when there is something to read.
        if !has_files(Path::new(&directory)) {
            continue;
        }
        let glob = format!("{directory}/**/*.parquet");
        let sql = format!(
            "CREATE VIEW {table} AS SELECT * FROM read_parquet('{}')",
            escape(&glob)
        );
        conn.execute_batch(&sql)
            .with_context(|| format!("reading the archive at {glob}"))?;
    }

    let websites = format!("{root}/websites.parquet");
    let sql = format!(
        "CREATE VIEW website AS SELECT * FROM read_parquet('{}')",
        escape(&websites)
    );
    // A website table is optional in an archive.
    let _ = conn.execute_batch(&sql);

    Ok(sumimi_service::pool::Db::from_connection(
        std::path::Path::new(pattern),
        conn,
    ))
}
