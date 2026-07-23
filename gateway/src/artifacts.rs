//! Artifacts API — list / download under `{workspace}/artifacts/`.

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::models::{ArtifactItem, DownloadArtifactResponse, ListArtifactsResponse};
use crate::state::AppState;
use crate::workspace::resolve_artifact_path;
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{Duration, Utc};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    pub path: String,
}

pub async fn list_artifacts(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ListArtifactsResponse>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;

    let root = PathBuf::from(&record.workspace.path).join("artifacts");
    if !root.exists() {
        return Ok(Json(ListArtifactsResponse { items: vec![] }));
    }

    let mut items = Vec::new();
    collect_artifacts(&root, &root, &mut items)?;
    items.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(ListArtifactsResponse { items }))
}

fn collect_artifacts(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<ArtifactItem>,
) -> Result<(), ApiError> {
    let entries = std::fs::read_dir(dir).map_err(|e| ApiError::Internal(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| ApiError::Internal(e.to_string()))?;
        let path = entry.path();
        let meta = entry
            .metadata()
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if meta.is_dir() {
            collect_artifacts(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| ApiError::Internal("path strip failed".into()))?
                .to_string_lossy()
                .replace('\\', "/");
            let modified_at = meta
                .modified()
                .map(|t| chrono::DateTime::<Utc>::from(t).to_rfc3339())
                .unwrap_or_else(|_| Utc::now().to_rfc3339());
            out.push(ArtifactItem {
                path: rel,
                size_bytes: meta.len(),
                modified_at,
            });
        }
    }
    Ok(())
}

pub async fn download_artifact(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DownloadQuery>,
) -> Result<Json<DownloadArtifactResponse>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;

    let workspace = PathBuf::from(&record.workspace.path);
    let _full = resolve_artifact_path(&workspace, &q.path)?;

    // Local gateway: authenticated content URL (no S3). Pass the same API key.
    let expires = Utc::now() + Duration::minutes(15);
    let url = format!(
        "{}/v1/agents/{}/artifacts/content?path={}",
        state.public_base_url.trim_end_matches('/'),
        id,
        urlencoding_encode(&q.path),
    );

    Ok(Json(DownloadArtifactResponse {
        path: q.path,
        url,
        expires_at: expires.to_rfc3339(),
    }))
}

pub async fn artifact_content(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DownloadQuery>,
) -> Result<axum::response::Response, ApiError> {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    let workspace = PathBuf::from(&record.workspace.path);
    let full = resolve_artifact_path(&workspace, &q.path)?;
    let bytes = std::fs::read(&full).map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut res = Body::from(bytes).into_response();
    *res.status_mut() = StatusCode::OK;
    let mime = mime_guess_from_path(&full);
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(&mime).unwrap_or_else(|_| {
            header::HeaderValue::from_static("application/octet-stream")
        }),
    );
    Ok(res)
}

fn urlencoding_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn mime_guess_from_path(path: &std::path::Path) -> String {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "json" => "application/json".into(),
        "md" | "markdown" => "text/markdown; charset=utf-8".into(),
        "txt" | "log" => "text/plain; charset=utf-8".into(),
        "html" | "htm" => "text/html; charset=utf-8".into(),
        "png" => "image/png".into(),
        "jpg" | "jpeg" => "image/jpeg".into(),
        "gif" => "image/gif".into(),
        "webp" => "image/webp".into(),
        "pdf" => "application/pdf".into(),
        "zip" => "application/zip".into(),
        _ => "application/octet-stream".into(),
    }
}
