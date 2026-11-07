//! Health endpoint.

use axum::extract::State;
use axum::Json;

use crate::api::AppState;
use crate::models::Heartbeat;

/// `GET /api/heartbeat` — liveness plus a few counters.
pub async fn heartbeat(State(state): State<AppState>) -> Json<Heartbeat> {
    let stats = state.storage.stats().unwrap_or_default();
    let gate = state.gate.stats();

    Json(Heartbeat {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        queued: state.writer.queued(),
        dropped: state.writer.dropped(),
        websites: stats.websites,
        duckdb_memory_bytes: stats.memory_bytes,
        duckdb_memory_limit: stats.memory_limit.clone(),
        duckdb_memory_limit_effective: stats.memory_limit,
        duckdb_threads: stats.threads,
        flushes: state.writer.flushes(),
        flush_p50_ms: state.writer.flush_percentile(50),
        flush_p99_ms: state.writer.flush_percentile(99),
        flush_max_ms: state.writer.flush_history().iter().copied().max().unwrap_or(0),
        database_size_bytes: stats.size_bytes,
        inflight_requests: gate.inflight_requests,
        inflight_bytes: gate.inflight_bytes,
        refused_rate: gate.refused_rate,
        refused_inflight: gate.refused_inflight,
        refused_body: gate.refused_body,
        admit_rps: state.config.max_admit_rps,
        rss_bytes: crate::rss::current_rss_portable(),
        peak_rss_bytes: crate::rss::peak_rss(),
        machine: crate::machine::machine(),
    })
}
