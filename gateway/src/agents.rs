//! Agent endpoints — create / list / get / archive / unarchive / delete.

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::models::{
    CreateAgentRequest, CreateAgentResponse, CreateRunRequest, ListAgentsResponse,
};
use crate::runs::start_run_on_agent;
use crate::state::AppState;
use crate::store::AgentRecord;
use crate::workspace;
use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    #[serde(default = "default_include_archived")]
    pub include_archived: bool,
}

fn default_include_archived() -> bool {
    true
}

pub async fn create_agent(
    _auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<CreateAgentRequest>,
) -> Result<Json<CreateAgentResponse>, ApiError> {
    if body.prompt.text.trim().is_empty() {
        return Err(ApiError::BadRequest("prompt.text is required".into()));
    }

    let agent_id = body
        .agent_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if state.agent_store.get(&agent_id)?.is_some() {
        return Err(ApiError::Conflict(format!("agent {agent_id} already exists")));
    }

    // Optional model switch for this process (affects subsequent runs).
    if let Some(ref model) = body.model {
        let mut rm = state.run_manager.lock().await;
        rm.switch_model(&model.id)
            .map_err(|e| ApiError::BadRequest(format!("invalid model: {e}")))?;
    }

    if let Some(ref mode) = body.mode {
        let rm = state.run_manager.lock().await;
        if let Some(parsed) = parse_mode(mode) {
            rm.set_mode(parsed);
        }
    }

    let provisioned = workspace::provision(&agent_id, body.env.as_ref(), &body.repos).await?;
    let cwd = provisioned.record.path.clone();
    let model_name = {
        let rm = state.run_manager.lock().await;
        rm.brain().current_model_name().to_string()
    };

    let name = body
        .name
        .clone()
        .unwrap_or_else(|| truncate_name(&body.prompt.text));

    // Create durable session (agent id == session id).
    let sm = state.session_manager.clone();
    let sid = agent_id.clone();
    let cwd_clone = cwd.clone();
    let model_clone = model_name.clone();
    let title = name.clone();
    tokio::task::spawn_blocking(move || {
        sm.save(Some(&sid), &[], &cwd_clone, &model_clone)?;
        let _ = sm.rename(&sid, &title);
        anyhow::Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let now = Utc::now().to_rfc3339();
    let mut record = AgentRecord {
        id: agent_id.clone(),
        name: name.clone(),
        status: "ACTIVE".into(),
        created_at: now.clone(),
        updated_at: now,
        latest_run_id: None,
        workspace: provisioned.record,
        repos: provisioned.repos,
        mode: body.mode.clone(),
        run_ids: Vec::new(),
        runs: Default::default(),
    };
    state.agent_store.save(&record)?;

    let run = start_run_on_agent(
        &state,
        &mut record,
        &CreateRunRequest {
            prompt: body.prompt,
            mode: body.mode,
        },
    )
    .await?;

    Ok(Json(CreateAgentResponse {
        agent: record.to_detail(),
        run,
    }))
}

pub async fn list_agents(
    _auth: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<ListAgentsQuery>,
) -> Result<Json<ListAgentsResponse>, ApiError> {
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let mut items = state.agent_store.list(q.include_archived)?;

    if let Some(cursor) = q.cursor.as_deref() {
        if let Some(pos) = items.iter().position(|a| a.id == cursor) {
            items = items.split_off(pos + 1);
        }
    }

    let next_cursor = if items.len() > limit {
        Some(items[limit - 1].id.clone())
    } else {
        None
    };
    items.truncate(limit);

    Ok(Json(ListAgentsResponse {
        items: items.iter().map(|a| a.to_summary()).collect(),
        next_cursor,
    }))
}

pub async fn get_agent(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::models::AgentDetail>, ApiError> {
    let rec = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    Ok(Json(rec.to_detail()))
}

pub async fn archive_agent(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::models::AgentDetail>, ApiError> {
    let rec = state.agent_store.update(&id, |r| {
        r.status = "ARCHIVED".into();
    })?;
    let sm = state.session_manager.clone();
    let sid = id.clone();
    let _ = tokio::task::spawn_blocking(move || sm.archive(&sid)).await;
    Ok(Json(rec.to_detail()))
}

pub async fn unarchive_agent(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::models::AgentDetail>, ApiError> {
    let rec = state.agent_store.update(&id, |r| {
        r.status = "ACTIVE".into();
    })?;
    let sm = state.session_manager.clone();
    let sid = id.clone();
    let _ = tokio::task::spawn_blocking(move || sm.unarchive(&sid)).await;
    Ok(Json(rec.to_detail()))
}

pub async fn delete_agent(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Cancel any live runs for this agent first.
    {
        let rm = state.run_manager.lock().await;
        let runs = rm.list_runs().await;
        for run_id in runs {
            if let Ok(handle_state) = rm.run_state(&run_id).await {
                if handle_state.is_alive() {
                    // Best-effort: cancel if this run belongs to the agent.
                    if let Some(rec) = state.agent_store.get(&id)? {
                        if rec.run_ids.contains(&run_id) {
                            let _ = rm.cancel_run(&run_id).await;
                        }
                    }
                }
            }
        }
    }

    let deleted = state.agent_store.delete(&id)?;
    if !deleted {
        return Err(ApiError::NotFound(format!("agent {id} not found")));
    }
    let sm = state.session_manager.clone();
    let sid = id.clone();
    let _ = tokio::task::spawn_blocking(move || sm.delete(&sid)).await;
    Ok(Json(serde_json::json!({ "id": id, "deleted": true })))
}

fn truncate_name(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= 60 {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(59).collect::<String>())
    }
}

fn parse_mode(s: &str) -> Option<agent_core::AgentMode> {
    match s.to_ascii_lowercase().as_str() {
        "ask" => Some(agent_core::AgentMode::Ask),
        "plan" => Some(agent_core::AgentMode::Plan),
        "build" | "agent" => Some(agent_core::AgentMode::Build),
        _ => None,
    }
}
