//! Health endpoint.

use axum::extract::State;
use axum::Json;

use crate::api::AppState;
use crate::models::Heartbeat;

/// `GET /api/heartbeat` — liveness plus a few counters.
pub async fn heartbeat(State(state): State<AppState>) -> Json<Heartbeat> {
    let websites = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare("SELECT count(*) FROM website")?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(row.get::<_, i64>(0)?),
                None => Ok(0),
            }
        })
        .unwrap_or(0);

    // What DuckDB's buffer manager currently holds in memory, and the limits DuckDB
    // itself reports (the effective limit is read back from DuckDB, not from our
    // configuration — that way a failed `SET` cannot go unnoticed).
    let (memory_usage, effective_limit, threads) = state
        .db
        .with_read(|conn| {
            let mut usage = 0i64;
            let mut stmt = conn.prepare("SELECT memory_usage_bytes FROM duckdb_memory()")?;
            let mut rows = stmt.query([])?;
            if let Some(row) = rows.next()? {
                usage = row.get(0).unwrap_or(0);
            }
            drop(rows);
            drop(stmt);

            let limit: String = conn
                .query_row(
                    "SELECT CAST(current_setting('memory_limit') AS VARCHAR)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or_else(|_| "unknown".to_string());

            let threads: i64 = conn
                .query_row("SELECT current_setting('threads')", [], |r| r.get(0))
                .unwrap_or(0);

            Ok((usage, limit, threads))
        })
        .unwrap_or((0, "unknown".to_string(), 0));

    // The size on disk, including the write-ahead log.
    let database_size = std::fs::metadata(state.config.db_path.as_path())
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    Json(Heartbeat {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        queued: state.writer.queued(),
        dropped: state.writer.dropped(),
        websites,
        duckdb_memory_bytes: memory_usage,
        duckdb_memory_limit: state.config.duckdb_memory_limit.clone(),
        duckdb_memory_limit_effective: effective_limit,
        duckdb_threads: threads,
        database_size_bytes: database_size,
    })
}
