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

    // What DuckDB's buffer manager currently holds in memory.
    let memory_usage = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare("SELECT memory_usage_bytes FROM duckdb_memory()")?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => Ok(row.get::<_, i64>(0)?),
                None => Ok(0),
            }
        })
        .unwrap_or(0);

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
        database_size_bytes: database_size,
    })
}
