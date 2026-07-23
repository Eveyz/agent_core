//! Run endpoints — create / list / get / cancel / stream / approve / answer.

use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::models::{
    AnswerRequest, ApproveRequest, CreateRunRequest, ListRunsResponse, RunView,
};
use crate::state::AppState;
use crate::store::{AgentRecord, RunRecord};
use crate::stream::{envelope_to_sse_events, map_status};
use agent_core::permission::ApprovalChoice;
use agent_core::runtime::command::RunCommand;
use agent_core::runtime::event::RunEvent;
use agent_core::RunState;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use chrono::Utc;
use futures::stream::{self, Stream};
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRunsQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

/// Shared helper used by create-agent and create-run.
pub async fn start_run_on_agent(
    state: &AppState,
    record: &mut AgentRecord,
    body: &CreateRunRequest,
) -> Result<RunView, ApiError> {
    if record.status == "ARCHIVED" {
        return Err(ApiError::Conflict("agent is archived".into()));
    }
    if body.prompt.text.trim().is_empty() {
        return Err(ApiError::BadRequest("prompt.text is required".into()));
    }

    // Cursor semantics: only one active run per agent.
    ensure_agent_idle(state, record).await?;

    if let Some(ref mode) = body.mode {
        let rm = state.run_manager.lock().await;
        if let Some(parsed) = parse_agent_mode(mode) {
            rm.set_mode(parsed);
        }
    }

    // Resume session history.
    let sm = state.session_manager.clone();
    let sid = record.id.clone();
    let history = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>(
            sm.resume(&sid)?
                .map(|s| s.messages)
                .unwrap_or_default(),
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let cwd = record.workspace.path.clone();
    let created = {
        let rm = state.run_manager.lock().await;
        rm.create_run_with_workdir(
            &body.prompt.text,
            Some(record.id.clone()),
            Some(cwd),
            history,
            None,
            false,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    };

    {
        let rm = state.run_manager.lock().await;
        rm.command(&created.run_id, RunCommand::Start)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    // Persist run metadata + spawn terminal-status watcher.
    let now = Utc::now().to_rfc3339();
    let run_rec = RunRecord {
        id: created.run_id.clone(),
        agent_id: record.id.clone(),
        status: "RUNNING".into(),
        created_at: now.clone(),
        updated_at: now,
        duration_ms: None,
        result: None,
        prompt_id: created.prompt_id.clone(),
    };
    record.latest_run_id = Some(created.run_id.clone());
    record.run_ids.insert(0, created.run_id.clone());
    record.runs.insert(created.run_id.clone(), run_rec.clone());
    record.updated_at = Utc::now().to_rfc3339();
    state.agent_store.save(record)?;

    spawn_run_watcher(state.clone(), record.id.clone(), created.run_id.clone());

    Ok(run_rec.to_view())
}

async fn ensure_agent_idle(state: &AppState, record: &AgentRecord) -> Result<(), ApiError> {
    let rm = state.run_manager.lock().await;
    for run_id in &record.run_ids {
        if let Ok(st) = rm.run_state(run_id).await {
            if st.is_alive() {
                return Err(ApiError::Conflict(format!(
                    "agent_busy: run {run_id} is still {}",
                    map_status(&st)
                )));
            }
        }
    }
    Ok(())
}

fn spawn_run_watcher(state: AppState, agent_id: String, run_id: String) {
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let st = {
                let rm = state.run_manager.lock().await;
                rm.run_state(&run_id).await.ok()
            };
            let Some(st) = st else { break };
            if !st.is_terminal() {
                continue;
            }

            let (status, result) = match st {
                RunState::Completed => {
                    // Try to pull final text from the event log.
                    let text = {
                        let rm = state.run_manager.lock().await;
                        rm.load_run_log(&run_id)
                            .ok()
                            .and_then(|envs| {
                                envs.into_iter().rev().find_map(|e| match e.event {
                                    RunEvent::RunCompleted { final_text } => Some(final_text),
                                    _ => None,
                                })
                            })
                    };
                    ("FINISHED", text)
                }
                RunState::Cancelled => ("CANCELLED", None),
                RunState::Failed => {
                    let text = {
                        let rm = state.run_manager.lock().await;
                        rm.load_run_log(&run_id)
                            .ok()
                            .and_then(|envs| {
                                envs.into_iter().rev().find_map(|e| match e.event {
                                    RunEvent::RunFailed { error } => Some(error),
                                    _ => None,
                                })
                            })
                    };
                    ("FAILED", text)
                }
                _ => break,
            };

            let duration_ms = started.elapsed().as_millis() as u64;
            let _ = state.agent_store.update(&agent_id, |rec| {
                if let Some(run) = rec.runs.get_mut(&run_id) {
                    run.status = status.into();
                    run.updated_at = Utc::now().to_rfc3339();
                    run.duration_ms = Some(duration_ms);
                    run.result = result.clone();
                }
            });
            break;
        }
    });
}

pub async fn create_run(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateRunRequest>,
) -> Result<Json<RunView>, ApiError> {
    let mut record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    let run = start_run_on_agent(&state, &mut record, &body).await?;
    Ok(Json(run))
}

pub async fn list_runs(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListRunsQuery>,
) -> Result<Json<ListRunsResponse>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;

    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let mut items = Vec::new();
    for rid in &record.run_ids {
        if let Some(r) = record.runs.get(rid) {
            items.push(live_run_view(&state, r).await);
        }
    }

    if let Some(cursor) = q.cursor.as_deref() {
        if let Some(pos) = items.iter().position(|r| r.id == cursor) {
            items = items.split_off(pos + 1);
        }
    }

    let next_cursor = if items.len() > limit {
        Some(items[limit - 1].id.clone())
    } else {
        None
    };
    items.truncate(limit);

    Ok(Json(ListRunsResponse {
        items,
        next_cursor,
    }))
}

async fn live_run_view(state: &AppState, r: &RunRecord) -> RunView {
    let mut view = r.to_view();
    let rm = state.run_manager.lock().await;
    if let Ok(st) = rm.run_state(&r.id).await {
        view.status = map_status(&st).to_string();
    }
    view
}

pub async fn get_run(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Json<RunView>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    let run = record
        .runs
        .get(&run_id)
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id} not found")))?;
    Ok(Json(live_run_view(&state, run).await))
}

pub async fn cancel_run(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Json<RunView>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    if !record.runs.contains_key(&run_id) {
        return Err(ApiError::NotFound(format!("run {run_id} not found")));
    }

    {
        let rm = state.run_manager.lock().await;
        let st = rm
            .run_state(&run_id)
            .await
            .map_err(|_| ApiError::Conflict("run_not_cancellable".into()))?;
        if st.is_terminal() {
            return Err(ApiError::Conflict("run_not_cancellable".into()));
        }
        rm.cancel_run(&run_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    let updated = state.agent_store.update(&id, |rec| {
        if let Some(run) = rec.runs.get_mut(&run_id) {
            run.status = "CANCELLED".into();
            run.updated_at = Utc::now().to_rfc3339();
        }
    })?;
    let run = updated.runs.get(&run_id).unwrap();
    Ok(Json(run.to_view()))
}

pub async fn stream_run(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    if !record.runs.contains_key(&run_id) {
        return Err(ApiError::NotFound(format!("run {run_id} not found")));
    }

    // Replay from event log, then live-tail.
    let replay = {
        let rm = state.run_manager.lock().await;
        rm.load_run_log(&run_id).unwrap_or_default()
    };

    let live_rx = {
        let rm = state.run_manager.lock().await;
        rm.subscribe(&run_id).await.ok()
    };

    let replay_events: Vec<Event> = replay
        .iter()
        .flat_map(envelope_to_sse_events)
        .collect();

    let replay_stream = stream::iter(replay_events.into_iter().map(Ok::<_, Infallible>));

    let live_stream = async_stream::stream! {
        let Some(rx) = live_rx else { return };
        let mut rx = BroadcastStream::new(rx);
        while let Some(item) = rx.next().await {
            let Ok(env) = item else { continue };
            for ev in envelope_to_sse_events(&env) {
                yield Ok::<Event, Infallible>(ev);
            }
        }
    };

    let combined = replay_stream.chain(live_stream);

    Ok(Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("{\"type\":\"heartbeat\"}"),
    ))
}

pub async fn approve(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
    Json(body): Json<ApproveRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    if !record.runs.contains_key(&run_id) {
        return Err(ApiError::NotFound(format!("run {run_id} not found")));
    }

    let choice = parse_choice(&body.choice)?;
    let rm = state.run_manager.lock().await;
    let ok = rm
        .resolve_approval(Some(&run_id), &body.prompt_id, choice)
        .await;
    if !ok {
        // Fallback via command channel.
        rm.command(
            &run_id,
            RunCommand::Approve {
                prompt_id: body.prompt_id.clone(),
                choice: parse_choice(&body.choice)?,
            },
        )
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn answer(
    _auth: AuthUser,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
    Json(body): Json<AnswerRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let record = state
        .agent_store
        .get(&id)?
        .ok_or_else(|| ApiError::NotFound(format!("agent {id} not found")))?;
    if !record.runs.contains_key(&run_id) {
        return Err(ApiError::NotFound(format!("run {run_id} not found")));
    }

    let rm = state.run_manager.lock().await;
    rm.command(
        &run_id,
        RunCommand::Answer {
            prompt_id: body.prompt_id,
            answer: body.answer,
        },
    )
    .await
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

fn parse_choice(s: &str) -> Result<ApprovalChoice, ApiError> {
    Ok(match s {
        "allow_once" | "AllowOnce" => ApprovalChoice::AllowOnce,
        "allow_session" | "AllowSession" => ApprovalChoice::AllowSession,
        "allow_persistent" | "AllowPersistent" => ApprovalChoice::AllowPersistent,
        "deny" | "Deny" => ApprovalChoice::Deny,
        "deny_persistent" | "DenyPersistent" => ApprovalChoice::DenyPersistent,
        other => {
            return Err(ApiError::BadRequest(format!(
                "unknown approval choice '{other}'"
            )));
        }
    })
}

fn parse_agent_mode(s: &str) -> Option<agent_core::AgentMode> {
    match s.to_ascii_lowercase().as_str() {
        "ask" => Some(agent_core::AgentMode::Ask),
        "plan" => Some(agent_core::AgentMode::Plan),
        "build" | "agent" => Some(agent_core::AgentMode::Build),
        _ => None,
    }
}
