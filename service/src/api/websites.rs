//! Website management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::api::AppState;
use crate::error::ApiError;
use crate::models::{CreateWebsite, UpdateWebsite, Website};

/// `GET /api/websites` — list websites.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Website>>, ApiError> {
    let websites = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, domain, created_at, updated_at, retention_days, country_header
                 FROM website ORDER BY created_at DESC",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(Website {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    domain: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    retention_days: row.get(5)?,
                    country_header: row.get(6)?,
                });
            }
            Ok(out)
        })
        .map_err(ApiError::internal)?;

    Ok(Json(websites))
}

/// `POST /api/websites` — create a website.
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateWebsite>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let domain = request
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let id = uuid::Uuid::new_v4().to_string();
    let now = crate::db::now();

    let created = state
        .db
        .with_write(|conn| {
            let mut stmt = conn.prepare("SELECT count(*) FROM website WHERE domain = ?")?;
            let mut rows = stmt.query([domain.as_deref()])?;
            let existing = match rows.next()? {
                Some(row) => row.get::<_, i64>(0)?,
                None => 0,
            };
            if existing > 0 {
                return Ok(false);
            }
            conn.execute(
                "INSERT INTO website (id, name, domain, created_at, updated_at, retention_days, country_header)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                duckdb::params![
                    id,
                    name,
                    domain,
                    now,
                    now,
                    state.config.retention_days,
                    state.config.country_header,
                ],
            )?;
            Ok(true)
        })
        .map_err(ApiError::internal)?;

    if !created {
        return Err(ApiError::Conflict(
            "a website with that domain already exists".into(),
        ));
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "websiteId": id,
            "name": name,
            "domain": domain,
            "createdAt": now,
            "updatedAt": now,
            "retentionDays": state.config.retention_days,
        })),
    ))
}

/// `GET /api/websites/{id}` — one website.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Website>, ApiError> {
    fetch(&state, &id).map(Json)
}

/// Load one website by id.
fn fetch(state: &AppState, id: &str) -> Result<Website, ApiError> {
    let website = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, domain, created_at, updated_at, retention_days, country_header
                 FROM website WHERE id = ?",
            )?;
            let mut rows = stmt.query([id])?;
            match rows.next()? {
                Some(row) => Ok(Some(Website {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    domain: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    retention_days: row.get(5)?,
                    country_header: row.get(6)?,
                })),
                None => Ok(None),
            }
        })
        .map_err(ApiError::internal)?;

    website.ok_or_else(|| ApiError::NotFound(format!("website not found: {id}")))
}

/// `PUT /api/websites/{id}` — update a website.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateWebsite>,
) -> Result<Json<Website>, ApiError> {
    let existing = fetch(&state, &id)?;

    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&existing.name)
        .to_string();
    let domain = match request.domain.as_deref() {
        Some(value) => {
            let value = value.trim().to_lowercase();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        None => existing.domain.clone(),
    };
    let retention_days = request.retention_days.unwrap_or(existing.retention_days);
    let country_header = request.country_header.clone().or(existing.country_header.clone());
    let now = crate::db::now();

    state
        .db
        .with_write(|conn| {
            conn.execute(
                "UPDATE website SET name = ?, domain = ?, retention_days = ?, country_header = ?, updated_at = ?
                 WHERE id = ?",
                duckdb::params![name, domain, retention_days, country_header, now, id],
            )?;
            Ok(())
        })
        .map_err(ApiError::internal)?;

    Ok(Json(Website {
        id,
        name,
        domain,
        created_at: existing.created_at,
        updated_at: now,
        retention_days,
        country_header,
    }))
}

/// `DELETE /api/websites/{id}` — delete a website and all of its data.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let removed = state
        .db
        .with_write(|conn| {
            conn.execute("DELETE FROM event WHERE website_id = ?", [&id])?;
            conn.execute("DELETE FROM session WHERE website_id = ?", [&id])?;
            let removed = conn.execute("DELETE FROM website WHERE id = ?", [&id])?;
            Ok(removed)
        })
        .map_err(ApiError::internal)?;

    if removed == 0 {
        return Err(ApiError::NotFound(format!("website not found: {id}")));
    }

    Ok(StatusCode::NO_CONTENT)
}
