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
    /// Open the database (creating it if needed) and make sure the schema exists.
    pub fn open(path: &Path, memory_limit: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!("creating database directory {}: {e}", parent.display())
                })?;
            }
        }

        let conn = Connection::open(path)
            .map_err(|e| anyhow::anyhow!("opening duckdb database {}: {e}", path.display()))?;
        conn.execute_batch(&format!("SET memory_limit='{memory_limit}'"))?;
        // A small WAL keeps a crash from losing the last flush, without huge files.
        conn.execute_batch("SET checkpoint_threshold='16MB'")?;

        crate::db::init_schema(&conn)?;

        Ok(Self {
            path: path.to_path_buf(),
            memory_limit: memory_limit.to_string(),
            conn: Mutex::new(conn),
        })
    }

    /// The database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The configured DuckDB memory limit.
    pub fn memory_limit(&self) -> &str {
        &self.memory_limit
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
