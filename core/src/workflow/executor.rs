//! Workflow executor — runs a planned DAG stage-by-stage.
//!
//! Each stage's nodes execute in parallel (bounded by a semaphore). Node-level
//! routers decide which downstream nodes actually run; nodes not selected by an
//! upstream router are marked `skipped`. Cancellation propagates via a
//! [`CancellationToken`]. Every node's input/output/tokens/latency is recorded
//! to the `workflow_run_node_results` table.

use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent_registry::{self, AgentDef, AgentHistoryEntry, AgentMemoryStore};
use crate::config::Config;
use crate::mode::AgentMode;
use crate::memory::embedding::EmbeddingModel;
use crate::memory::storage::Storage;
use crate::permission::PermissionConfig;
use crate::runtime::Brain;
use crate::subagent::Subagent;
use crate::tools::ToolRegistry;
use crate::types::{AgentEvent, EventSender};

use super::context::{RouterConfig, WorkflowContext};
use super::definition::{
    NodeType, NodeDef, OnNodeFailure, WorkflowDef, WorkflowRunNodeResult,
};
use super::planner;
use super::definition::TrustMode;

/// Result of a workflow execution.
#[derive(Debug, Clone)]
pub struct WorkflowRunResult {
    pub run_id: String,
    pub status: String,
    pub output: serde_json::Value,
    pub error: String,
    pub total_token_input: i64,
    pub total_token_output: i64,
}

pub struct WorkflowExecutor {
    storage: Storage,
    brain: Arc<Brain>,
}

impl WorkflowExecutor {
    pub fn new(storage: Storage, brain: Arc<Brain>) -> Self {
        Self { storage, brain }
    }

    /// Execute a workflow to completion (or cancellation).
    pub async fn execute(
        &self,
        workflow: &WorkflowDef,
        input: serde_json::Value,
        session_id: &str,
        cancel_token: CancellationToken,
        event_tx: Option<EventSender>,
    ) -> Result<WorkflowRunResult> {
        // 1. Plan (topological stages + cycle detection).
        let plan = planner::plan(&workflow.nodes, &workflow.edges)?;

        // 2. Create the run record.
        let storage_for_run = self.storage.clone();
        let wf_id = workflow.id.clone();
        let sess = session_id.to_string();
        let input_clone = input.clone();
        let run_id = tokio::task::spawn_blocking(move || {
            super::definition::create_run(&storage_for_run, &wf_id, &sess, &input_clone)
        })
        .await
        .context("create_run task failed")??;

        emit(&event_tx, || AgentEvent::WorkflowStarted {
            workflow_id: workflow.id.clone(),
            run_id: run_id.clone(),
        });

        let ctx = Arc::new(WorkflowContext::new(input.clone()));
        let skipped: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let max_concurrent = workflow.max_concurrent.max(1);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));

        let mut total_tokens = (0i64, 0i64);
        let mut run_error = String::new();
        let mut aborted = false;

        for stage in &plan.stages {
            if cancel_token.is_cancelled() {
                aborted = true;
                break;
            }

            // Collect nodes that should execute (not skipped).
            let executable: Vec<&NodeDef> = stage
                .nodes
                .iter()
                .filter_map(|id| workflow.nodes.iter().find(|n| &n.id == id))
                .filter(|n| !skipped.lock().contains(&n.id))
                .collect();

            if executable.is_empty() {
                continue;
            }

            // Spawn each node as a parallel task.
            let mut handles = Vec::new();
            for node in executable {
                let incoming = planner::incoming_edges(&node.id, &workflow.edges);
                let node_input = ctx.resolve_input(&node.id, &incoming);

                let handle = self.spawn_node(
                    node.clone(),
                    node_input,
                    ctx.clone(),
                    skipped.clone(),
                    workflow.clone(),
                    semaphore.clone(),
                    cancel_token.clone(),
                    event_tx.clone(),
                    run_id.clone(),
                    session_id.to_string(),
                );
                handles.push(handle);
            }

            // Await all nodes in the stage.
            for handle in handles {
                match handle.await {
                    Ok(Ok((_node_output, tokens))) => {
                        total_tokens.0 += tokens.0;
                        total_tokens.1 += tokens.1;
                    }
                    Ok(Err(e)) => {
                        tracing::error!("workflow node failed: {e}");
                        run_error = e.to_string();
                        match workflow.on_node_failure {
                            OnNodeFailure::Abort => {
                                aborted = true;
                                break;
                            }
                            OnNodeFailure::Continue | OnNodeFailure::Skip => {}
                        }
                    }
                    Err(e) => {
                        run_error = format!("task join error: {e}");
                        match workflow.on_node_failure {
                            OnNodeFailure::Abort => {
                                aborted = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }

            if aborted {
                break;
            }
        }

        // Collect the output node's value.
        let output = workflow
            .nodes
            .iter()
            .find(|n| n.node_type == NodeType::Output)
            .and_then(|n| ctx.get_output(&n.id))
            .unwrap_or_else(|| {
                // Fallback: last agent node output.
                workflow
                    .nodes
                    .iter()
                    .filter(|n| n.node_type == NodeType::Agent)
                    .last()
                    .and_then(|n| ctx.get_output(&n.id))
                    .unwrap_or(serde_json::Value::Null)
            });

        let status = if aborted {
            "cancelled".to_string()
        } else if !run_error.is_empty() && workflow.on_node_failure == OnNodeFailure::Abort {
            "failed".to_string()
        } else {
            "completed".to_string()
        };

        // Finalize the run record.
        let storage = self.storage.clone();
        let run_id_clone = run_id.clone();
        let status_clone = status.clone();
        let output_clone = output.clone();
        let err_clone = run_error.clone();
        let ti = total_tokens.0;
        let to = total_tokens.1;
        let _ = tokio::task::spawn_blocking(move || {
            let _ = super::definition::finish_run(
                &storage,
                &run_id_clone,
                &status_clone,
                &output_clone,
                &err_clone,
                ti,
                to,
            );
        })
        .await;

        emit(&event_tx, || AgentEvent::WorkflowCompleted {
            run_id: run_id.clone(),
            status: status.clone(),
            output: output.clone(),
        });

        Ok(WorkflowRunResult {
            run_id,
            status,
            output,
            error: run_error,
            total_token_input: total_tokens.0,
            total_token_output: total_tokens.1,
        })
    }

    /// Spawn a single node's execution as a tokio task.
    #[allow(clippy::too_many_arguments)]
    fn spawn_node(
        &self,
        node: NodeDef,
        input: serde_json::Value,
        ctx: Arc<WorkflowContext>,
        skipped: Arc<Mutex<HashSet<String>>>,
        workflow: WorkflowDef,
        semaphore: Arc<tokio::sync::Semaphore>,
        cancel_token: CancellationToken,
        event_tx: Option<EventSender>,
        run_id: String,
        session_id: String,
    ) -> tokio::task::JoinHandle<Result<(serde_json::Value, (i64, i64))>> {
        let storage = self.storage.clone();
        let brain = self.brain.clone();

        tokio::spawn(async move {
            // Acquire a concurrency permit.
            let _permit = semaphore.acquire().await.map_err(|e| anyhow::anyhow!(e))?;

            if cancel_token.is_cancelled() {
                return Err(anyhow::anyhow!("cancelled"));
            }

            emit(&event_tx, || AgentEvent::WorkflowNodeStarted {
                run_id: run_id.clone(),
                node_id: node.id.clone(),
                node_type: node.node_type.as_str().to_string(),
                label: node.label.clone(),
            });

            let started = std::time::Instant::now();
            let started_at = Utc::now().to_rfc3339();

            let exec_result = execute_node(
                &node,
                &input,
                &brain,
                &storage,
                &workflow,
                &session_id,
                &run_id,
                cancel_token.clone(),
                event_tx.clone(),
            )
            .await;

            let latency_ms = started.elapsed().as_millis() as i64;
            let finished_at = Utc::now().to_rfc3339();

            let (output, tokens, status, error) = match exec_result {
                Ok((out, tok)) => (out, tok, "completed", String::new()),
                Err(e) => {
                    let msg = e.to_string();
                    (serde_json::Value::Null, (0, 0), "failed", msg)
                }
            };

            // Store the output in the context (for downstream resolution).
            ctx.set_output(&node.id, output.clone());

            // Apply the node's router: skip downstream nodes not in targets.
            apply_router(&node, &output, &workflow, &skipped);

            // Record the node result.
            let result = WorkflowRunNodeResult {
                id: uuid::Uuid::new_v4().to_string(),
                workflow_run_id: run_id.clone(),
                node_id: node.id.clone(),
                agent_history_id: String::new(),
                status: status.to_string(),
                input: input.clone(),
                output: output.clone(),
                error: error.clone(),
                token_input: tokens.0,
                token_output: tokens.1,
                cost_usd: 0.0,
                latency_ms,
                started_at: Some(started_at),
                finished_at: Some(finished_at),
                created_at: Utc::now().to_rfc3339(),
            };
            let rec_storage = storage.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let _ = super::definition::record_node_result(&rec_storage, &result);
            })
            .await;

            emit(&event_tx, || AgentEvent::WorkflowNodeEnded {
                run_id: run_id.clone(),
                node_id: node.id.clone(),
                status: status.to_string(),
                output: output.clone(),
            });

            if status == "failed" {
                Err(anyhow::anyhow!(error))
            } else {
                Ok((output, tokens))
            }
        })
    }
}

/// Execute a single node, dispatching by type.
#[allow(clippy::too_many_arguments)]
async fn execute_node(
    node: &NodeDef,
    input: &serde_json::Value,
    brain: &Brain,
    storage: &Storage,
    workflow: &WorkflowDef,
    session_id: &str,
    run_id: &str,
    cancel_token: CancellationToken,
    event_tx: Option<EventSender>,
) -> Result<(serde_json::Value, (i64, i64))> {
    match node.node_type {
        NodeType::Input => {
            // The input node outputs the workflow input.
            let wf_input = input
                .get("_workflow_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Ok((wf_input, (0, 0)))
        }
        NodeType::Output => {
            // The output node forwards its resolved input as the workflow output.
            Ok((input.clone(), (0, 0)))
        }
        NodeType::Transform => {
            // V1 transform: pass-through (config may specify field extraction later).
            let config = &node.config;
            if let Some(extract) = config.get("extract").and_then(|v| v.as_str()) {
                if let Some(val) = input.get(extract) {
                    return Ok((val.clone(), (0, 0)));
                }
            }
            Ok((input.clone(), (0, 0)))
        }
        NodeType::HumanApproval => {
            // V1: auto-approve (full approval UI in a later phase).
            Ok((
                serde_json::json!({"approved": true, "input": input}),
                (0, 0),
            ))
        }
        NodeType::Agent => {
            if node.agent_id.is_empty() {
                return Err(anyhow::anyhow!(
                    "agent node '{}' has no agent_id",
                    node.id
                ));
            }
            execute_agent_node(
                node,
                input,
                brain,
                storage,
                workflow,
                session_id,
                run_id,
                cancel_token,
                event_tx,
            )
            .await
        }
    }
}

/// Execute an agent node: build a Subagent from the AgentDef and run it.
#[allow(clippy::too_many_arguments)]
async fn execute_agent_node(
    node: &NodeDef,
    input: &serde_json::Value,
    brain: &Brain,
    storage: &Storage,
    workflow: &WorkflowDef,
    session_id: &str,
    _run_id: &str,
    _cancel_token: CancellationToken,
    event_tx: Option<EventSender>,
) -> Result<(serde_json::Value, (i64, i64))> {
    // Fetch the agent definition.
    let agent_id = node.agent_id.clone();
    let s = storage.clone();
    let def = tokio::task::spawn_blocking(move || {
        agent_registry::get(&s, &agent_id)
    })
    .await??;

    // Build runtime components.
    let mut subagent_config = agent_registry::build_subagent_config(&def);
    subagent_config.system_prompt =
        inject_skill_content(brain, &def.skills, &subagent_config.system_prompt);

    let model_config = agent_registry::build_model_config(&def, &brain.config);
    let permission_config = workflow
        .trust_mode
        .build_permission_config(&brain.config.permissions, &def);

    let registry = if def.tools.is_empty() {
        brain.build_tool_registry(AgentMode::Build)
    } else {
        ToolRegistry::from_names(&def.tools)
    };

    let memory = if def.memory_enabled > 0 {
        Some(Arc::new(build_agent_memory_store(brain, storage.clone())))
    } else {
        None
    };

    let mut subagent = Subagent::new_with_memory(
        &def.name,
        subagent_config,
        &model_config,
        registry,
        permission_config,
        memory,
        Some(def.id.clone()),
    );

    let task = format_agent_input(node, input);
    let started = std::time::Instant::now();
    let result = subagent.run_with_sender(&task, event_tx).await?;
    let elapsed_ms = started.elapsed().as_millis() as i64;

    // Record agent history.
    let entry = AgentHistoryEntry {
        agent_id: def.id.clone(),
        session_id: session_id.to_string(),
        workflow_run_id: _run_id.to_string(),
        trigger: "workflow".to_string(),
        input: task,
        output: result.output.clone(),
        iterations_used: result.iterations_used as u32,
        success: result.success,
        model_used: model_config.model_id.clone(),
        process_time_ms: elapsed_ms,
        ..Default::default()
    };
    let hist_storage = storage.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let _ = agent_registry::history::record(&hist_storage, &entry);
    })
    .await;

    // V1: token tracking not yet wired from Subagent streams.
    let tokens = (0i64, 0i64);
    let output = serde_json::json!({
        "result": result.output,
        "success": result.success,
        "iterations": result.iterations_used,
    });

    Ok((output, tokens))
}

/// Apply a node's router: mark downstream nodes not in the route targets as skipped.
fn apply_router(
    node: &NodeDef,
    output: &serde_json::Value,
    workflow: &WorkflowDef,
    skipped: &Arc<Mutex<HashSet<String>>>,
) {
    let Some(router) = RouterConfig::from_node_config(&node.config) else {
        return;
    };
    let targets: HashSet<String> = router.route(output).into_iter().collect();
    let downstream: Vec<String> = planner::outgoing_edges(&node.id, &workflow.edges)
        .iter()
        .map(|e| e.target_node_id.clone())
        .collect();
    let mut skip = skipped.lock();
    for target in downstream {
        if !targets.contains(&target) {
            skip.insert(target);
        }
    }
}

/// Build an [`AgentMemoryStore`] from the Brain's embedding configuration.
pub fn build_agent_memory_store(
    brain: &Brain,
    storage: Storage,
) -> AgentMemoryStore {
    if let Some(ref mem) = brain.config.memory {
        if mem.embedding_enabled {
            if let Ok(model) = EmbeddingModel::new(&mem.embedding_model) {
                return AgentMemoryStore::new(storage, Arc::new(model));
            }
        }
    }
    AgentMemoryStore::without_embedding(storage)
}

/// Inject skill content into a system prompt (content path).
fn inject_skill_content(brain: &Brain, skills: &[String], system_prompt: &str) -> String {
    let mut prompt = system_prompt.to_string();
    if let Some(ref sm) = brain.skill_manager {
        let mgr = sm.lock();
        for name in skills {
            if let Ok(Some(content)) = mgr.load_skill_context(name) {
                if !prompt.is_empty() {
                    prompt.push_str("\n\n");
                }
                prompt.push_str(&content);
            }
        }
    }
    prompt
}

/// Format a node's resolved JSON input into a readable task string.
fn format_agent_input(node: &NodeDef, input: &serde_json::Value) -> String {
    // If the input is a simple string, pass it through directly.
    if let Some(s) = input.as_str() {
        return s.to_string();
    }
    // Otherwise, render a readable JSON representation.
    let pretty = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
    format!(
        "Task for agent '{}':\n\n{}\n\nComplete the task and return the result.",
        node.label, pretty
    )
}

/// Emit an event if a sender is present (swallows send errors).
fn emit<F: FnOnce() -> AgentEvent>(event_tx: &Option<EventSender>, f: F) {
    if let Some(tx) = event_tx {
        let _ = tx.send(f());
    }
}


