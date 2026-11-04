//! Database access.
//!
//! DuckDB is a single-writer, embedded database, and it has one behaviour that shapes
//! this design: **a connection keeps the snapshot it started with**. A second connection
//! opened before a write keeps returning the old state, even in read-write mode (see
//! `docs/duckdb-notes.md`).
//!
//! So the server uses exactly **one** connection, guarded by a mutex. Every read and
//! every write goes through it, which means:
//!
//!   * the dashboard always sees freshly committed data,
//!   * there is a single writer (DuckDB's model anyway),
//!   * a slow query delays the next flush — acceptable, and better than stale numbers.
//!
//! The CLI opens the file read-only instead (see `cli/src/main.rs`), because a snapshot
//! is exactly what an offline analysis wants.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use duckdb::Connection;

/// The Sumami database handle.
pub struct Db {
    path: PathBuf,
    memory_limit: String,
    /// The one and only connection.
    conn: Mutex<Connection>,
}

impl Db {
    /// Wrap an existing connection (used by the CLI, which opens the file itself).
    pub fn from_connection(path: &Path, conn: Connection) -> Self {
        Self {
            path: path.to_path_buf(),
            memory_limit: "0B".to_string(),
            conn: Mutex::new(conn),
        }
    }

    /// Open the database (creating it if needed) and make sure the schema exists.
    pub fn open(path: &Path, memory_limit: &str) -> Result<Self> {
        Self::open_with(path, memory_limit, "32MB", "2GB", 0)
    }

    /// Open the database and apply the full set of DuckDB settings.
    pub fn open_with(
        path: &Path,
        memory_limit: &str,
        flush_threshold: &str,
        temp_limit: &str,
        threads: usize,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!("creating database directory {}: {e}", parent.display())
                })?;
            }
        }

        let mut conn = Connection::open(path)
            .map_err(|e| anyhow::anyhow!("opening duckdb database {}: {e}", path.display()))?;
        Self::configure(
            &conn,
            memory_limit,
            flush_threshold,
            temp_limit,
            threads,
        )?;

        // An older database is rewritten into a new file: DuckDB cannot drop a
        // constraint, and it never gives the blocks of a dropped table back to the
        // filesystem, so an in-place rebuild would leave the file twice as large.
        let version = crate::db::schema_version(&conn);
        if version > 0 && version < crate::db::SCHEMA_VERSION {
            tracing::info!(
                "the database was written by schema v{version}; rebuilding it as v{}",
                crate::db::SCHEMA_VERSION
            );
            let rebuilt = crate::db::rebuild_to_new_file(path, &conn)?;
            // The old file is locked by this connection — close it before the swap.
            drop(conn);
            std::fs::rename(&rebuilt, path).map_err(|e| {
                anyhow::anyhow!(
                    "swapping the rebuilt database {} in: {e}",
                    rebuilt.display()
                )
            })?;
            conn = Connection::open(path).map_err(|e| {
                anyhow::anyhow!("opening the rebuilt duckdb database {}: {e}", path.display())
            })?;
            Self::configure(
                &conn,
                memory_limit,
                flush_threshold,
                temp_limit,
                threads,
            )?;
        }

        crate::db::init_schema(&conn)?;

        Ok(Self {
            path: path.to_path_buf(),
            memory_limit: memory_limit.to_string(),
            conn: Mutex::new(conn),
        })
    }

    /// Apply the full set of DuckDB settings.
    fn configure(
        conn: &Connection,
        memory_limit: &str,
        flush_threshold: &str,
        temp_limit: &str,
        threads: usize,
    ) -> Result<()> {
        conn.execute_batch(&format!("SET memory_limit='{memory_limit}'"))?;
        // Hand freed memory back to the OS sooner than DuckDB's 128 MB default, so a
        // small box does not stay big after a traffic burst.
        conn.execute_batch(&format!(
            "SET allocator_flush_threshold='{flush_threshold}'"
        ))?;
        // A query that spills must not fill the disk.
        conn.execute_batch(&format!("SET max_temp_directory_size='{temp_limit}'"))?;
        // A small WAL keeps a crash from losing the last flush, without huge files.
        conn.execute_batch("SET checkpoint_threshold='16MB'")?;

        // Threads are derived from the memory budget: a parallel DISTINCT aggregate
        // needs one hash table per thread.
        let threads = if threads > 0 {
            threads
        } else {
            crate::config::threads_for(
                memory_limit,
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            )
        };
        conn.execute_batch(&format!("SET threads={threads}"))?;
        Ok(())
    }

    /// The database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The configured DuckDB memory limit.
    pub fn memory_limit(&self) -> &str {
        &self.memory_limit
    }

    /// Open a Parquet archive as an in-memory database.
    ///
    /// The archive is queried in place through `read_parquet()`; it is never imported.
    /// The views carry the same names as the tables, so the DuckDB backend's SQL runs
    /// over an archive unchanged.
    pub fn open_archive(pattern: &str) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let escape = |value: &str| value.replace('\'', "''");
        let root = pattern.trim_end_matches("/*").trim_end_matches('/');

        for (table, sub) in [("event", "events"), ("revenue", "revenue")] {
            let directory = format!("{root}/{sub}");
            if !has_parquet_files(std::path::Path::new(&directory)) {
                continue;
            }
            let glob = format!("{directory}/**/*.parquet");
            conn.execute_batch(&format!(
                "CREATE VIEW {table} AS SELECT * FROM read_parquet('{}')",
                escape(&glob)
            ))?;
        }

        let websites = format!("{root}/websites.parquet");
        let _ = conn.execute_batch(&format!(
            "CREATE VIEW website AS SELECT * FROM read_parquet('{}')",
            escape(&websites)
        ));

        Ok(Self {
            path: PathBuf::from(pattern),
            memory_limit: "0B".to_string(),
            conn: Mutex::new(conn),
        })
    }

    /// Run `f` with the database connection.
    ///
    /// The archival job uses this: DuckDB connections cannot be shared between
    /// threads, and the database file is locked by this process anyway.
    pub fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    /// Run `f` with the database connection.
    pub fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    /// Run `f` with the database connection, for writing.
    pub fn with_write<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }
}

/// Does a directory (recursively) hold any parquet file?
fn has_parquet_files(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        if path.is_dir() {
            if has_parquet_files(&path) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("parquet") {
            return true;
        }
    }
    false
}
