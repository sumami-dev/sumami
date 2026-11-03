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

    Json(Heartbeat {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        queued: state.writer.queued(),
        dropped: state.writer.dropped(),
        websites,
    })
}
