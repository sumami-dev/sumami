//! The control plane: the log, the Search Console import, the saved reports and
//! the assistant's conversations.
//!
//! These are the things an instance keeps that are **not** analytics. They live
//! in the instance store — a small, transactional database of their own — so
//! they survive an analytics rebuild and can be backed up in a second.
//!
//! The endpoints here are the visible side of that store: what a site owner can
//! look at, import and save.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::api::stats::run_report;
use crate::api::AppState;
use crate::error::ApiError;
use crate::instance::{ChatMessage, GscRow, ImportReport, LogEntry, Snapshot};
use crate::models::ReportSpec;

// --------------------------------------------------------------------------------------
// The log
// --------------------------------------------------------------------------------------

/// The query of the log endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    /// Only the lines of this kind (`login`, `website`, `import`, …).
    #[serde(default)]
    pub kind: Option<String>,
    /// How many lines to return.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /api/logs` — what happened in this instance.
pub async fn logs(
    State(state): State<AppState>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    Ok(Json(state.instance.logs(query.kind.as_deref(), limit)?))
}

/// `POST /api/logs` — write a line to the log.
pub async fn write_log(
    State(state): State<AppState>,
    Json(entry): Json<LogEntry>,
) -> Result<StatusCode, ApiError> {
    state.instance.log(&entry)?;
    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------------------
// The Search Console import
// --------------------------------------------------------------------------------------

/// The body of a Search Console import.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GscImport {
    /// The website the rows belong to.
    pub website: String,
    /// The rows.
    pub rows: Vec<GscRow>,
    /// Where the rows came from (a file name).
    #[serde(default)]
    pub source: Option<String>,
}

/// `POST /api/gsc/import` — import a Search Console export.
///
/// **All or nothing**: the whole file lands in one transaction, so a dump that
/// fails halfway does not leave half a week of rows behind.
pub async fn import_gsc(
    State(state): State<AppState>,
    Json(import): Json<GscImport>,
) -> Result<Json<ImportReport>, ApiError> {
    let report = state
        .instance
        .import_gsc(&import.website, &import.rows, import.source.as_deref())?;
    state.instance.log(&LogEntry {
        at: crate::db::now(),
        level: "info".to_string(),
        kind: "import".to_string(),
        actor: None,
        website_id: Some(import.website.clone()),
        message: format!("imported {} Search Console rows", report.rows),
        data: None,
    })?;
    Ok(Json(report))
}

/// `GET /api/websites/{id}/gsc` — the imported rows, newest day first.
pub async fn gsc_rows(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<GscRow>>, ApiError> {
    let limit = query.limit.unwrap_or(500).clamp(1, 5000);
    Ok(Json(state.instance.gsc_rows(&id, limit)?))
}

/// `GET /api/websites/{id}/gsc/imports` — what was imported, and when.
pub async fn gsc_imports(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ImportReport>>, ApiError> {
    Ok(Json(state.instance.gsc_imports(&id)?))
}

// --------------------------------------------------------------------------------------
// The saved reports (snapshots)
// --------------------------------------------------------------------------------------

/// The body of a snapshot: a name and the report to save.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRequest {
    /// The name to save it under.
    pub name: String,
    /// The report — what to run.
    pub spec: ReportSpec,
}

/// `GET /api/snapshots` — the saved reports.
pub async fn snapshots(
    State(state): State<AppState>,
) -> Result<Json<Vec<Snapshot>>, ApiError> {
    Ok(Json(state.instance.snapshots()?))
}

/// `POST /api/snapshots` — run a report and save it under a name.
///
/// The report is run **now**, and both the question and the answer are stored —
/// so the saved report can be looked at later, and run again when the data has
/// moved on.
pub async fn save_snapshot(
    State(state): State<AppState>,
    Json(request): Json<SnapshotRequest>,
) -> Result<Json<Snapshot>, ApiError> {
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("the snapshot needs a name".into()));
    }
    let result = run_report(state.storage.as_ref(), &request.spec)?;
    let query = serde_json::to_string(&request.spec)?;
    let snapshot = state.instance.save_snapshot(&name, &query, &result.to_string())?;
    state.instance.log(&LogEntry {
        at: crate::db::now(),
        level: "info".to_string(),
        kind: "snapshot".to_string(),
        actor: None,
        website_id: Some(request.spec.website.clone()),
        message: format!("saved the report '{name}'"),
        data: None,
    })?;
    Ok(Json(snapshot))
}

/// `POST /api/snapshots/{name}/run` — run a saved report again.
///
/// The stored question is run against the data **as it is now**, and the answer
/// replaces the old one: a saved report is a saved question, not a saved number.
pub async fn run_snapshot(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Snapshot>, ApiError> {
    let snapshots = state.instance.snapshots()?;
    let snapshot = snapshots
        .into_iter()
        .find(|snapshot| snapshot.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("no snapshot named '{name}'")))?;

    let spec: ReportSpec = serde_json::from_str(&snapshot.query)
        .map_err(|err| ApiError::BadRequest(format!("the saved report is broken: {err}")))?;
    let result = run_report(state.storage.as_ref(), &spec)?;
    Ok(Json(state.instance.save_snapshot(
        &name,
        &snapshot.query,
        &result.to_string(),
    )?))
}

/// `DELETE /api/snapshots/{name}` — forget a saved report.
pub async fn delete_snapshot(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.instance.delete_snapshot(&name)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("no snapshot named '{name}'")))
    }
}

// --------------------------------------------------------------------------------------
// The assistant's conversations
// --------------------------------------------------------------------------------------

/// The body of a new conversation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    /// The website the conversation is about.
    #[serde(default)]
    pub website: Option<String>,
    /// The title of the conversation.
    #[serde(default)]
    pub title: Option<String>,
}

/// The body of a message.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessageRequest {
    /// Who is speaking: `user` or `assistant`.
    pub role: String,
    /// What was said.
    pub content: String,
    /// Anything else, as JSON.
    #[serde(default)]
    pub data: Option<String>,
}

/// `GET /api/chat/sessions` — the conversations.
pub async fn chat_sessions(
    State(state): State<AppState>,
) -> Result<Json<Vec<crate::instance::ChatSession>>, ApiError> {
    Ok(Json(state.instance.chat_sessions()?))
}

/// `POST /api/chat/sessions` — start a conversation.
pub async fn start_chat(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<crate::instance::ChatSession>, ApiError> {
    Ok(Json(state.instance.chat_session(
        request.website.as_deref(),
        request.title.as_deref(),
    )?))
}

/// `GET /api/chat/sessions/{id}` — the messages of a conversation, oldest first.
pub async fn chat_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ChatMessage>>, ApiError> {
    Ok(Json(state.instance.chat_messages(&id)?))
}

/// `POST /api/chat/sessions/{id}/messages` — say something.
pub async fn say(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ChatMessageRequest>,
) -> Result<Json<ChatMessage>, ApiError> {
    Ok(Json(state.instance.chat_message(
        &id,
        &request.role,
        &request.content,
        request.data.as_deref(),
    )?))
}

// --------------------------------------------------------------------------------------
// The cache
// --------------------------------------------------------------------------------------

/// `GET /api/cache` — how much the report cache holds.
pub async fn cache_stats(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entries = state.instance.cache_len()?;
    Ok(Json(serde_json::json!({ "entries": entries })))
}

/// `DELETE /api/cache` — empty the report cache.
pub async fn purge_cache(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let removed = state.instance.cache_purge()?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}
