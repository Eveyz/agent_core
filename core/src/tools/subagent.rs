use crate::config::ModelConfig;
use crate::permission::PermissionConfig;
use crate::session::SessionManager;
use crate::subagent::spec::{AgentSpawnRequest, EffectiveAgentSpec, ParentAgentSpec, PromptLayers};
use crate::subagent::{PersonaKey, ResultStrategy, Subagent, SubagentConfig};
use crate::todo::SessionPlanStore;
use crate::tools::{Tool, ToolRegistry, ToolUpdateFn};
use crate::types::{EventSender, Message, Role};
use anyhow::Result;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

pub fn register_subagent_tools(
    registry: &mut ToolRegistry,
    model_config: ModelConfig,
    available_tool_names: Vec<String>,
    session_mgr: Option<Arc<SessionManager>>,
    permission_config: PermissionConfig,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
    approval_resolver: Option<crate::runtime::ApprovalResolver>,
    plan_store: Option<Arc<SessionPlanStore>>,
) {
    let parent_max_iterations = model_config.max_iterations;
    let mut single = SubagentSpawnTool::new(
        model_config.clone(),
        available_tool_names.clone(),
        session_mgr.clone(),
        permission_config.clone(),
        parent_max_iterations,
    )
    .with_parent_depth(parent_depth);
    let mut spawn_all = SubagentSpawnAllTool::new(
        model_config,
        available_tool_names,
        session_mgr,
        permission_config,
        parent_max_iterations,
    )
    .with_parent_depth(parent_depth);
    if let Some(ref sm) = skill_manager {
        single = single.with_skill_manager(sm.clone());
        spawn_all = spawn_all.with_skill_manager(sm.clone());
    }
    if let Some(sv) = supervisor {
        single = single.with_supervisor(sv.clone());
        spawn_all = spawn_all.with_supervisor(sv);
    }
    if let Some(ct) = cancel_token {
        single = single.with_cancel_token(ct.clone());
        spawn_all = spawn_all.with_cancel_token(ct);
    }
    if let Some(resolver) = approval_resolver {
        single = single.with_approval_resolver(resolver.clone());
        spawn_all = spawn_all.with_approval_resolver(resolver);
    }
    if let Some(store) = plan_store {
        single = single.with_plan_store(store.clone());
        spawn_all = spawn_all.with_plan_store(store);
    }
    registry.register(Box::new(single));
    registry.register(Box::new(spawn_all));
    registry.register(Box::new(SubagentTranscriptTool::default()));
}

/// Re-wire `subagent`/`subagents` tools in `registry` so that any spawn from
/// this registry propagates cancellation and process isolation.
///
/// Use at any agent-level execution boundary to fix up the Brain-built
/// registry (which is constructed with `None, None` for the meta tools,
/// because `Brain::build_tool_registry` doesn't have a `CancellationToken`
/// or `ProcessSupervisor` of its own — the boundary — Run, WorkflowNode,
/// Standalone — owns one and must inject it).
///
/// `parent_depth` is the recursion depth of the agent owning `registry`;
/// spawn calls inside it will become depth `parent_depth + 1`.
pub fn re_wire_subagent_tools(
    registry: &mut ToolRegistry,
    model_config: ModelConfig,
    session_mgr: Option<Arc<SessionManager>>,
    permission_config: PermissionConfig,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    parent_depth: u8,
) {
    re_wire_subagent_tools_with_skills(
        registry,
        model_config,
        session_mgr,
        permission_config,
        supervisor,
        cancel_token,
        parent_depth,
        None,
        ApprovalRouting::LegacyScoped,
        None,
    );
}

#[derive(Clone)]
pub enum ApprovalRouting {
    Run(crate::runtime::ApprovalResolver),
    /// Use the run-id-scoped compatibility channel when no owning Run exists
    /// (for example a standalone persisted workflow execution).
    LegacyScoped,
}

/// Like [`re_wire_subagent_tools`], but also wires a shared [`SkillManager`]
/// so spawned subagents inherit parent session actives.
pub fn re_wire_subagent_tools_with_skills(
    registry: &mut ToolRegistry,
    model_config: ModelConfig,
    session_mgr: Option<Arc<SessionManager>>,
    permission_config: PermissionConfig,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
    approval_routing: ApprovalRouting,
    plan_store: Option<Arc<SessionPlanStore>>,
) {
    if !registry.has("subagent") && !registry.has("subagents") {
        return;
    }
    let available_tools: Vec<String> = registry
        .list_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    registry.remove_all(&["subagent", "subagents"]);
    let approval_resolver = match approval_routing {
        ApprovalRouting::Run(resolver) => Some(resolver),
        ApprovalRouting::LegacyScoped => None,
    };
    register_subagent_tools(
        registry,
        model_config,
        available_tools,
        session_mgr,
        permission_config,
        supervisor,
        cancel_token,
        parent_depth,
        skill_manager,
        approval_resolver,
        plan_store,
    );
}

// ── SubagentSpawnTool ────────────────────────────────────────────────

#[derive(Default)]
struct SubagentTranscriptTool {
    root: Option<std::path::PathBuf>,
}

impl SubagentTranscriptTool {
    #[cfg(test)]
    fn with_root(root: std::path::PathBuf) -> Self {
        Self { root: Some(root) }
    }
}

#[async_trait::async_trait]
impl Tool for SubagentTranscriptTool {
    fn name(&self) -> &str {
        "subagent_transcript"
    }

    fn description(&self) -> &str {
        "Read a paginated canonical subagent transcript by runtime_id when a handoff reports missing context or more evidence is needed."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "runtime_id": { "type": "string" },
                "offset": { "type": "integer", "minimum": 0, "default": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
            },
            "required": ["runtime_id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let runtime_id = args["runtime_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'runtime_id'"))?;
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = (args["limit"].as_u64().unwrap_or(20) as usize).clamp(1, 100);
        let root = if let Some(root) = &self.root {
            root.clone()
        } else {
            crate::subagent::transcript::TranscriptRecorder::default_path(runtime_id)?
                .parent()
                .ok_or_else(|| anyhow::anyhow!("transcript root unavailable"))?
                .to_path_buf()
        };
        let expected_scope = crate::subagent::transcript::TranscriptScope {
            session_id: args
                .get("_session_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            parent_run_id: args
                .get("_parent_run_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        };
        if expected_scope.parent_run_id.is_none() {
            anyhow::bail!("subagent transcript lookup requires a parent run scope");
        }
        let runtime_id = runtime_id.to_string();
        let document = tokio::task::spawn_blocking(move || {
            crate::subagent::transcript::TranscriptRecorder::read_in(
                &root,
                &runtime_id,
                &expected_scope,
            )
        })
        .await
        .map_err(|error| anyhow::anyhow!("transcript reader task failed: {error}"))??;
        let total = document.messages.len();
        let messages: Vec<_> = document
            .messages
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect();
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "schema": "subagent-transcript-page/v1",
            "runtime_id": document.runtime_id,
            "outcome": document.outcome,
            "offset": offset,
            "returned": messages.len(),
            "total": total,
            "has_more": offset.saturating_add(messages.len()) < total,
            "messages": messages,
        }))?)
    }
}

pub(crate) struct SubagentSpawnTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
    session_mgr: Option<Arc<SessionManager>>,
    permission_config: PermissionConfig,
    parent_max_iterations: usize,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Recursion depth of the agent that owns this tool. When this tool
    /// spawns a subagent, the child gets `parent_depth + 1` and the spawn
    /// is refused past `MAX_SUBAGENT_DEPTH`.
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
    approval_resolver: Option<crate::runtime::ApprovalResolver>,
    plan_store: Option<Arc<SessionPlanStore>>,
}

impl SubagentSpawnTool {
    fn new(
        model_config: ModelConfig,
        available_tools: Vec<String>,
        session_mgr: Option<Arc<SessionManager>>,
        permission_config: PermissionConfig,
        parent_max_iterations: usize,
    ) -> Self {
        Self {
            model_config,
            available_tools,
            session_mgr,
            permission_config,
            parent_max_iterations,
            supervisor: None,
            cancel_token: None,
            parent_depth: 0,
            skill_manager: None,
            approval_resolver: None,
            plan_store: None,
        }
    }

    pub fn with_supervisor(
        mut self,
        sv: Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>,
    ) -> Self {
        self.supervisor = Some(sv);
        self
    }

    pub fn with_cancel_token(mut self, ct: tokio_util::sync::CancellationToken) -> Self {
        self.cancel_token = Some(ct);
        self
    }

    pub fn with_parent_depth(mut self, depth: u8) -> Self {
        self.parent_depth = depth;
        self
    }

    pub fn with_skill_manager(mut self, sm: Arc<Mutex<crate::skills::SkillManager>>) -> Self {
        self.skill_manager = Some(sm);
        self
    }

    pub fn with_approval_resolver(mut self, resolver: crate::runtime::ApprovalResolver) -> Self {
        self.approval_resolver = Some(resolver);
        self
    }

    pub fn with_plan_store(mut self, store: Arc<SessionPlanStore>) -> Self {
        self.plan_store = Some(store);
        self
    }
}

#[async_trait::async_trait]
impl Tool for SubagentSpawnTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Spawn a sub-agent with isolated context for a specific task. \
Use for: multi-step research, tasks needing clean context, parallel work. \
Do NOT use for: simple reads, single commands, quick searches — handle those yourself. \
Args: id (string), task (string), system_prompt (optional), tools (optional array of tool names, default: all parent tools), max_iterations (optional, default: parent agent's max_iterations)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Unique sub-agent ID"
                },
                "task": {
                    "type": "string",
                    "description": "Task description for the sub-agent to complete"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Custom system prompt (optional)"
                },
                "tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tool names (default: read_file, glob, grep, shell). Use 'all' for all parent tools."
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Max iterations (default: inherited from parent agent)"
                },
                "result_strategy": {
                    "type": "string",
                    "enum": ["auto", "full", "summary"],
                    "description": "How to format the result. 'full': return complete final output (best for code/data). 'summary': inject summarisation instructions and return only final text. 'auto' (default): return all text + tool summary."
                }
            },
            "required": ["id", "task"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_with_stream(args, None, None).await
    }

    async fn execute_with_stream(
        &self,
        args: Value,
        _on_update: Option<ToolUpdateFn>,
        event_sender: Option<EventSender>,
    ) -> Result<String> {
        let strategy = parse_result_strategy(&args);
        let id = args["id"].as_str().unwrap_or("unknown").to_string();

        let spawned = spawn_single(
            &args,
            &self.model_config,
            &self.available_tools,
            event_sender,
            &self.permission_config,
            self.parent_max_iterations,
            strategy,
            self.supervisor.clone(),
            self.cancel_token.clone(),
            self.parent_depth,
            self.skill_manager.clone(),
            self.approval_resolver.clone(),
            self.session_mgr.clone(),
            self.plan_store.clone(),
        )
        .await;
        let (result, _messages) = match spawned {
            Ok(value) => value,
            Err(error) => {
                return Ok(error.handoff(&id).render_for_parent());
            }
        };

        Ok(result.format_output(strategy))
    }
}

// ── SubagentSpawnAllTool (concurrent) ────────────────────────────────

pub(crate) struct SubagentSpawnAllTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
    session_mgr: Option<Arc<SessionManager>>,
    permission_config: PermissionConfig,
    parent_max_iterations: usize,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Recursion depth of the agent that owns this tool.
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
    approval_resolver: Option<crate::runtime::ApprovalResolver>,
    plan_store: Option<Arc<SessionPlanStore>>,
}

impl SubagentSpawnAllTool {
    fn new(
        model_config: ModelConfig,
        available_tools: Vec<String>,
        session_mgr: Option<Arc<SessionManager>>,
        permission_config: PermissionConfig,
        parent_max_iterations: usize,
    ) -> Self {
        Self {
            model_config,
            available_tools,
            session_mgr,
            permission_config,
            parent_max_iterations,
            supervisor: None,
            cancel_token: None,
            parent_depth: 0,
            skill_manager: None,
            approval_resolver: None,
            plan_store: None,
        }
    }

    pub fn with_supervisor(
        mut self,
        sv: Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>,
    ) -> Self {
        self.supervisor = Some(sv);
        self
    }

    pub fn with_cancel_token(mut self, ct: tokio_util::sync::CancellationToken) -> Self {
        self.cancel_token = Some(ct);
        self
    }

    pub fn with_parent_depth(mut self, depth: u8) -> Self {
        self.parent_depth = depth;
        self
    }

    pub fn with_skill_manager(mut self, sm: Arc<Mutex<crate::skills::SkillManager>>) -> Self {
        self.skill_manager = Some(sm);
        self
    }

    pub fn with_approval_resolver(mut self, resolver: crate::runtime::ApprovalResolver) -> Self {
        self.approval_resolver = Some(resolver);
        self
    }

    pub fn with_plan_store(mut self, store: Arc<SessionPlanStore>) -> Self {
        self.plan_store = Some(store);
        self
    }
}

#[async_trait::async_trait]
impl Tool for SubagentSpawnAllTool {
    fn name(&self) -> &str {
        "subagents"
    }

    fn description(&self) -> &str {
        "Spawn multiple sub-agents CONCURRENTLY (runs in parallel). \
Use when task_ready returns multiple unblocked tasks that are independent. \
Each sub-agent gets isolated context with access to: read_file, glob, grep, shell, edit, webfetch, git tools. \
Returns all results. \
Args: tasks (array of {id, task, tools?, max_iterations?})"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "description": "Array of sub-agent tasks to run concurrently",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "Sub-agent ID"},
                            "task": {"type": "string", "description": "Task description"},
                            "tools": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Tool names"
                            },
                            "max_iterations": {"type": "integer", "description": "Max iterations"},
                            "result_strategy": {
                                "type": "string",
                                "enum": ["auto", "full", "summary"],
                                "description": "How to format the result. 'full': return complete final output. 'summary': summarised. 'auto' (default): all text + tool summary."
                            }
                        },
                        "required": ["id", "task"]
                    }
                }
            },
            "required": ["tasks"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_with_stream(args, None, None).await
    }

    async fn execute_with_stream(
        &self,
        args: Value,
        _on_update: Option<ToolUpdateFn>,
        event_sender: Option<EventSender>,
    ) -> Result<String> {
        let tasks = args["tasks"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'tasks' array"))?;

        if tasks.is_empty() {
            return Ok("No tasks to execute.".to_string());
        }

        // Emit SubagentStart events immediately so TUI shows all boxes
        // before any subagent actually begins work.
        let mut task_infos: Vec<(String, String, Option<Vec<String>>, usize, ResultStrategy)> =
            Vec::new();
        for task_spec in tasks {
            let id = task_spec["id"].as_str().unwrap_or("unknown").to_string();
            let task = task_spec["task"].as_str().unwrap_or("").to_string();
            let tools: Option<Vec<String>> =
                task_spec.get("tools").and_then(Value::as_array).map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });
            let max_iterations = task_spec["max_iterations"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(self.parent_max_iterations);
            let strategy = parse_result_strategy(task_spec);

            task_infos.push((id, task, tools, max_iterations, strategy));
        }

        // Spawn all subagents concurrently on a JoinSet so they can be
        // aborted if the parent tool execution is cancelled. Without this,
        // canceling the parent leaves child subagents running as detached
        // tasks (process leak).
        let mut join_set = tokio::task::JoinSet::new();
        let parent_max_iterations = self.parent_max_iterations;
        let parent_session_id = args
            .get("_session_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let parent_prompt_id = args
            .get("_prompt_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let parent_working_dir = args
            .get("_working_dir")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let parent_run_id = args
            .get("_parent_run_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let parent_call_id = args
            .get("_parent_call_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        for (id, task, tools, max_iterations, strategy) in task_infos {
            let model_config = self.model_config.clone();
            let permission_config = self.permission_config.clone();
            let parent_available_tools = self.available_tools.clone();

            let mgr_clone = self.session_mgr.clone();
            let plan_store_clone = self.plan_store.clone();
            let sub_sender = event_sender.clone();
            let sv_clone = self.supervisor.clone();
            let ct_clone = self.cancel_token.clone();
            let parent_depth = self.parent_depth;
            let skill_manager = self.skill_manager.clone();
            let approval_resolver = self.approval_resolver.clone();
            let parent_session_id = parent_session_id.clone();
            let parent_prompt_id = parent_prompt_id.clone();
            let parent_working_dir = parent_working_dir.clone();
            let parent_run_id = parent_run_id.clone();
            let parent_call_id = parent_call_id.clone();

            join_set.spawn(async move {
                let mut args = serde_json::json!({
                    "id": id.clone(),
                    "task": task,
                    "max_iterations": max_iterations,
                });
                if let Some(tools) = tools {
                    args.as_object_mut()
                        .expect("subagent args are an object")
                        .insert("tools".to_string(), serde_json::json!(tools));
                }
                if let Some(ref session_id) = parent_session_id {
                    args.as_object_mut()
                        .expect("subagent args are an object")
                        .insert("_session_id".to_string(), Value::String(session_id.clone()));
                }
                if let Some(ref prompt_id) = parent_prompt_id {
                    args.as_object_mut()
                        .expect("subagent args are an object")
                        .insert("_prompt_id".to_string(), Value::String(prompt_id.clone()));
                }
                if let Some(ref working_dir) = parent_working_dir {
                    args.as_object_mut()
                        .expect("subagent args are an object")
                        .insert(
                            "_working_dir".to_string(),
                            Value::String(working_dir.clone()),
                        );
                }
                if let Some(ref run_id) = parent_run_id {
                    args.as_object_mut()
                        .expect("subagent args are an object")
                        .insert("_parent_run_id".to_string(), Value::String(run_id.clone()));
                }
                if let Some(ref call_id) = parent_call_id {
                    args.as_object_mut()
                        .expect("subagent args are an object")
                        .insert(
                            "_parent_call_id".to_string(),
                            Value::String(call_id.clone()),
                        );
                }

                let result = spawn_single(
                    &args,
                    &model_config,
                    &parent_available_tools,
                    sub_sender,
                    &permission_config,
                    parent_max_iterations,
                    strategy,
                    sv_clone,
                    ct_clone,
                    parent_depth,
                    skill_manager,
                    approval_resolver,
                    mgr_clone,
                    plan_store_clone,
                )
                .await;

                (id, result, strategy)
            });
        }

        // Collect results. If the parent is cancelled, the JoinSet is
        // dropped, which aborts all child tasks.
        let mut results = Vec::new();
        while let Some(res) = join_set.join_next().await {
            results.push(res);
        }

        let mut output = String::new();
        output.push_str(&format!(
            "=== Sub-agent Batch Results ({} tasks) ===\n\n",
            results.len()
        ));

        for (idx, result) in results.iter().enumerate() {
            match result {
                Ok((id, Ok((sub_result, _msgs)), strategy)) => {
                    let mut entry = format!(
                        "[{}] {} — {}\n{}\n",
                        idx + 1,
                        id,
                        if sub_result.success {
                            "success"
                        } else {
                            "incomplete"
                        },
                        sub_result.format_output(*strategy)
                    );
                    entry.push('\n');
                    output.push_str(&entry);
                }
                Ok((id, Err(e), _)) => {
                    let handoff = e.handoff(id);
                    output.push_str(&format!("[{}] {}\n{}\n\n", idx + 1, id, handoff.render_for_parent()));
                }
                Err(e) => {
                    let handoff = crate::subagent::handoff::SubagentHandoff::from_error(
                        format!("batch-join-{}", idx + 1),
                        e.to_string(),
                    );
                    output.push_str(&format!("[{}]\n{}\n\n", idx + 1, handoff.render_for_parent()));
                }
            }
        }

        output.push_str("=== End batch results ===");
        Ok(output)
    }
}

// ── Tool summary builders ────────────────────────────────────────────

/// Max chars per single tool result in the summary (before truncation).
/// Kept per-result only; total summary is capped downstream by the hygiene
/// layer's Incidental 16K-char budget, so we don't need a separate global cap.
const TOOL_SUMMARY_PER_RESULT_MAX: usize = 2000;

/// Build a compact summary of what tools the subagent called and what they found.
/// Walks the subagent's message history, pairing each assistant tool_call with
/// its corresponding tool result, and summarises by tool type.
fn build_tool_summary(messages: &[Message]) -> String {
    // Single pass: collect (tool_name, args_snippet, result_content) tuples.
    // We match Tool messages back to the most recent pending assistant call.
    let mut pending_calls: VecDeque<(String, String)> = VecDeque::new(); // (name, args_snippet)
    let mut entries: Vec<(String, String, String)> = Vec::new(); // (name, args, result)

    for msg in messages {
        match msg.role {
            Role::Assistant => {
                if let Some(ref calls) = msg.tool_calls {
                    for tc in calls {
                        let args_snippet = truncate_str(&tc.function.arguments, 200);
                        pending_calls.push_back((tc.function.name.clone(), args_snippet));
                    }
                }
            }
            Role::Tool => {
                let tool_name = msg.name.as_deref().unwrap_or("?").to_string();
                let content = msg.content.as_deref().unwrap_or("");
                if !content.is_empty() {
                    let (call_name, call_args) = pending_calls
                        .pop_front()
                        .unwrap_or_else(|| (tool_name.clone(), String::new()));
                    let result_summary = summarise_tool_content(&call_name, content);
                    entries.push((call_name, call_args, result_summary));
                }
            }
            _ => {}
        }
    }

    if entries.is_empty() {
        return String::new();
    }

    let mut summary = String::new();
    for (idx, (name, args, result)) in entries.iter().enumerate() {
        summary.push_str(&format!(
            "[{}] {} {}\n  → {}\n",
            idx + 1,
            name,
            args,
            result
        ));
    }
    summary
}

/// Summarise a single tool result by type.
fn summarise_tool_content(tool_name: &str, content: &str) -> String {
    match tool_name {
        "grep" => summarise_grep(content),
        "glob" => summarise_glob(content),
        "read_file" | "read" => summarise_read_file(content),
        "shell" | "bash" => summarise_shell(content),
        "webfetch" | "web_fetch" => summarise_webfetch(content),
        "edit" | "write_file" => summarise_edit(content),
        "ls" | "list_files" => summarise_ls(content),
        "search_content" => summarise_grep(content), // same format
        "search_file" => summarise_glob(content),
        _ => summarise_generic(content),
    }
}

fn summarise_grep(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let file_set: HashSet<&str> = lines.iter().filter_map(|l| l.split(':').next()).collect();
    let total_lines = lines.len();
    let sample: Vec<&str> = lines.iter().take(8).copied().collect();
    let sample_str = truncate_str(&sample.join(" | "), 800);
    format!(
        "found {} matches in {} file(s). samples: {}",
        total_lines,
        file_set.len(),
        sample_str
    )
}

fn summarise_glob(content: &str) -> String {
    let files: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    let count = files.len();
    let first: Vec<&str> = files.iter().take(8).copied().collect();
    let mut s = format!("found {} file(s).", count);
    if !first.is_empty() {
        s.push_str(" files: ");
        s.push_str(&first.join(", "));
        if count > first.len() {
            s.push_str(&format!(", ... and {} more", count - first.len()));
        }
    }
    truncate_str(&s, TOOL_SUMMARY_PER_RESULT_MAX).to_string()
}

fn summarise_read_file(content: &str) -> String {
    let lines = content.lines().count();
    let chars = content.len();
    // Keep a meaningful prefix so the main agent can see actual code content.
    let snippet = truncate_str(content, 1500);
    format!("[{} lines, {} chars] {}", lines, chars, snippet)
}

fn summarise_shell(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let first: Vec<&str> = lines.iter().take(10).copied().collect();
    let s = format!("[{} lines] {}", total, first.join("\n  "));
    truncate_str(&s, TOOL_SUMMARY_PER_RESULT_MAX).to_string()
}

fn summarise_webfetch(content: &str) -> String {
    let chars = content.len();
    let snippet = truncate_str(content, 500).to_string();
    format!("[{} chars] {}", chars, snippet)
}

fn summarise_edit(content: &str) -> String {
    if content.is_empty() {
        return "(no output)".to_string();
    }
    truncate_str(content, 500).to_string()
}

fn summarise_ls(content: &str) -> String {
    let entries: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    let count = entries.len();
    let first: Vec<&str> = entries.iter().take(10).copied().collect();
    let mut s = format!("{} entries. ", count);
    if !first.is_empty() {
        s.push_str(&first.join(", "));
        if count > first.len() {
            s.push_str(&format!(", ... and {} more", count - first.len()));
        }
    }
    truncate_str(&s, TOOL_SUMMARY_PER_RESULT_MAX).to_string()
}

fn summarise_generic(content: &str) -> String {
    let chars = content.len();
    let snippet = truncate_str(content, 500).to_string();
    format!("[{} chars] {}", chars, snippet)
}

/// Truncate a string to at most `max_len` chars, appending "..." if cut.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let end = floor_char_boundary(s, max_len - 3);
    format!("{}...", &s[..end])
}

use crate::util::floor_char_boundary;

// ── Shared spawn logic ───────────────────────────────────────────────

/// Result from spawning a single subagent.
struct SpawnResult {
    runtime_id: String,
    success: bool,
    iterations_used: usize,
    output: String,
    /// Text from only the final assistant turn.
    last_text: String,
    tool_count: usize,
    tool_summary: String,
    transcript_ref: Option<String>,
}

#[derive(Debug)]
struct SpawnFailure {
    runtime_id: Option<String>,
    transcript_ref: Option<String>,
    error: anyhow::Error,
}

impl std::fmt::Display for SpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.error)
    }
}

impl From<anyhow::Error> for SpawnFailure {
    fn from(error: anyhow::Error) -> Self {
        Self { runtime_id: None, transcript_ref: None, error }
    }
}

impl SpawnFailure {
    fn handoff(&self, fallback_id: &str) -> crate::subagent::handoff::SubagentHandoff {
        crate::subagent::handoff::SubagentHandoff::from_error_with_transcript(
            self.runtime_id.clone().unwrap_or_else(|| fallback_id.to_string()),
            self.error.to_string(),
            self.transcript_ref.clone(),
        )
    }
}

impl crate::session::SubagentResultLike for SpawnResult {
    fn summary_for_session(&self) -> String {
        format!(
            "[Subagent] iterations={} success={} tools={}\n{}\n\n{}",
            self.iterations_used, self.success, self.tool_count, self.output, self.tool_summary
        )
    }
}

impl SpawnResult {
    /// Format the subagent output according to the chosen ResultStrategy.
    fn format_output(&self, strategy: ResultStrategy) -> String {
        let summary = match strategy {
            ResultStrategy::Full | ResultStrategy::Summary if !self.last_text.is_empty() => {
                self.last_text.clone()
            }
            _ => self.output.clone(),
        };
        crate::subagent::handoff::SubagentHandoff::from_runtime_result(
            self.runtime_id.clone(),
            self.success,
            summary,
            self.tool_summary.clone(),
            self.transcript_ref.clone(),
            self.iterations_used,
            self.tool_count,
        )
        .render_for_parent()
    }
}

/// Walk up from `start` until we find a `Cargo.toml`, then return that
/// directory as the workspace root. Falls back to `start` if no Cargo.toml is
/// found in the ancestor chain.
fn find_workspace_root(start: &std::path::Path) -> std::path::PathBuf {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(start)
    };
    loop {
        if current.join("Cargo.toml").exists() {
            return current;
        }
        if !current.pop() {
            // Reached filesystem root without finding Cargo.toml — fall back.
            return start.to_path_buf();
        }
    }
}

/// Parse the result_strategy field from tool args, defaulting to Auto.
fn parse_result_strategy(args: &Value) -> ResultStrategy {
    match args["result_strategy"].as_str() {
        Some("full") => ResultStrategy::Full,
        Some("summary") => ResultStrategy::Summary,
        _ => ResultStrategy::Auto,
    }
}

/// Check if a tool name is a meta-dispatch tool that should NOT be inherited
/// by a spawned subagent. Tools excluded here can only reach a subagent
/// explicitly through path D (`brain.build_tool_registry` for a workflow
/// agent node with empty `def.tools`).
///
/// Filtering policy: spawn-driven subagents (`subagent`/`subagents`) and
/// runtime skill loaders (`skill_list`/`skill_load`/`skill_deactivate`/
/// `skill_reload`) are meta tools that orchestrate OTHER pieces of work
/// from the parent — they should never be inherited implicitly.
pub(crate) fn is_meta_dispatch_tool(name: &str) -> bool {
    crate::subagent::spec::is_meta_dispatch_tool(name)
}

/// Resolve a child's tool set as a strict subset of the concrete tools held by
/// the parent. Missing `tools` means inherit all; an explicit empty array means
/// no tools. The order follows the parent's registry so the result is stable.
fn select_subagent_tools(args: &Value, available_tools: &[String]) -> Vec<String> {
    let requested: Option<HashSet<&str>> = match args.get("tools") {
        None => None,
        Some(Value::String(value)) if value == "all" => None,
        Some(Value::Array(values)) => Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| *name != "all")
                .collect(),
        ),
        Some(_) => Some(HashSet::new()),
    };

    available_tools
        .iter()
        .filter(|name| !is_meta_dispatch_tool(name))
        .filter(|name| {
            requested
                .as_ref()
                .is_none_or(|wanted| wanted.contains(name.as_str()))
        })
        .cloned()
        .collect()
}

fn is_authorized_child_tool(
    name: &str,
    available_tools: &[String],
    effective_skills: &[String],
) -> bool {
    available_tools.iter().any(|available| available == name)
        || effective_skills
            .iter()
            .any(|skill| name.starts_with(&format!("skill.{skill}.")))
}

fn child_allows_skill_scripts(available_tools: &[String]) -> bool {
    available_tools.iter().any(|name| name == "shell")
}

/// The parent Run has already selected the effective cwd/worktree. Preserve
/// that exact scope for the child instead of rediscovering from process CWD.
fn effective_subagent_working_dir(args: &Value) -> std::path::PathBuf {
    if let Some(path) = args
        .get("_working_dir")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
    {
        return std::path::PathBuf::from(path);
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    find_workspace_root(&cwd)
}

async fn read_persona_file(persona_root: &std::path::Path, key: &PersonaKey) -> Option<String> {
    let canonical_root = tokio::fs::canonicalize(persona_root).await.ok()?;
    let target = persona_root.join(format!("{}.md", key.as_str()));
    let canonical_target = tokio::fs::canonicalize(target).await.ok()?;
    if !canonical_target.starts_with(&canonical_root) {
        tracing::warn!(
            path = %canonical_target.display(),
            root = %canonical_root.display(),
            "Ignoring persona file outside the configured persona root"
        );
        return None;
    }
    tokio::fs::read_to_string(canonical_target).await.ok()
}

async fn spawn_single(
    args: &Value,
    model_config: &ModelConfig,
    available_tools: &[String],
    event_sender: Option<EventSender>,
    permission_config: &PermissionConfig,
    parent_max_iterations: usize,
    result_strategy: ResultStrategy,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
    approval_resolver: Option<crate::runtime::ApprovalResolver>,
    session_mgr: Option<Arc<SessionManager>>,
    plan_store: Option<Arc<SessionPlanStore>>,
) -> std::result::Result<(SpawnResult, Vec<crate::types::Message>), SpawnFailure> {
    use crate::skills::SkillManager;

    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
    let task = args["task"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'task'"))?;

    let default_system_prompt = "You are a focused sub-agent. Complete the given task and return the result. Be concise. \
Only use tools actually present in your tool schema; capabilities are delegated by the parent and may be empty. \
Do NOT attempt to bypass a missing capability through another tool.";

    let mut persona_content = String::new();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let workspace_root = effective_subagent_working_dir(args);

    if let Ok(persona_key) = PersonaKey::parse(id) {
        let global_root = std::path::Path::new(&home).join(".agverse").join("agents");
        if let Some(c) = read_persona_file(&global_root, &persona_key).await {
            persona_content.push_str(&format!("Global Persona ({id}):\n{c}\n\n"));
        }

        let local_root = workspace_root.join(".agverse").join("agents");
        if let Some(c) = read_persona_file(&local_root, &persona_key).await {
            persona_content.push_str(&format!("Project Persona ({id}):\n{c}\n\n"));
        }
    }

    let mut base_prompt = args["system_prompt"]
        .as_str()
        .unwrap_or(default_system_prompt)
        .to_string();

    // Inject the exact parent execution root so the child cannot silently
    // widen a worktree-scoped Run back to the process checkout.
    let ws_root = workspace_root.to_string_lossy().to_string();

    let parent_session_id = args.get("_session_id").and_then(|v| v.as_str());
    let parent_prompt_id = args.get("_prompt_id").and_then(|v| v.as_str());
    let parent_run_id = args.get("_parent_run_id").and_then(Value::as_str);
    let parent_call_id = args.get("_parent_call_id").and_then(Value::as_str);

    // Pre-allocate child session + prompt so todo tools can bind before run.
    let allocated = session_mgr.as_ref().and_then(|mgr| {
        match mgr.pre_allocate_subagent_session(
            id,
            parent_session_id,
            parent_run_id,
            parent_call_id,
        ) {
            Ok(ids) => Some(ids),
            Err(error) => {
                tracing::warn!(
                    subagent_id = %id,
                    error = %error,
                    "Failed to pre-allocate subagent session"
                );
                None
            }
        }
    });

    // Inherit parent session actives ∪ any declared skills on the spawn args.
    let declared: Vec<String> = args
        .get("skills")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let effective_skills = if let Some(ref sm) = skill_manager {
        let mgr = sm.lock();
        mgr.resolve_subagent_skills(&declared, parent_session_id)
    } else {
        declared
    };
    if !effective_skills.is_empty() {
        base_prompt = SkillManager::inject_skill_content_into(
            skill_manager.as_ref(),
            &effective_skills,
            &base_prompt,
        );
    }

    let requested_iterations = args["max_iterations"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok());

    let selected_tools = select_subagent_tools(args, available_tools);
    let output_contract = crate::subagent::spec::output_contract(result_strategy);
    let effective = EffectiveAgentSpec::resolve(
        ParentAgentSpec {
            available_tools: available_tools.to_vec(),
            max_iterations: parent_max_iterations,
            max_context_tokens: model_config.max_context_tokens,
            permission: permission_config.clone(),
            working_dir: workspace_root.clone(),
            recursion_depth: parent_depth,
        },
        AgentSpawnRequest {
            role_name: id.to_string(),
            requested_tools: Some(selected_tools),
            requested_max_iterations: requested_iterations,
            skills: effective_skills,
            prompt: PromptLayers {
                base: base_prompt,
                persona: persona_content,
                runtime: format!("Working Directory: {ws_root}"),
                output_contract: output_contract.to_string(),
            },
            result_strategy,
            memory_identity: None,
        },
    )?;
    let final_tool_names = effective.tools.clone();

    let tool_count = final_tool_names.len();

    // Build real ToolRegistry with factory
    let mut tool_registry = ToolRegistry::from_names(&final_tool_names);
    // Skill scripts are executable capabilities. The parent Run only exposes
    // `shell` in Build mode, so use that inherited capability as the hard
    // boundary for dynamic script registration in children as well.
    if !effective.skills.is_empty() && child_allows_skill_scripts(available_tools) {
        SkillManager::sync_skill_scripts_for_skills(
            skill_manager.as_ref(),
            &effective.skills,
            &mut tool_registry,
            supervisor.clone(),
        );
        let unauthorized: Vec<String> = tool_registry
            .clone_names()
            .into_iter()
            .filter(|name| !is_authorized_child_tool(name, available_tools, &effective.skills))
            .collect();
        let unauthorized_refs: Vec<&str> = unauthorized.iter().map(String::as_str).collect();
        tool_registry.remove_all(&unauthorized_refs);
    }

    if let (Some(store), Some((child_sid, child_pid))) = (&plan_store, &allocated) {
        crate::tools::todo::register_todo_tools(
            &mut tool_registry,
            store.clone(),
            Some(child_sid.clone()),
            Some(child_pid.clone()),
        );
    }

    let config = SubagentConfig {
        system_prompt: effective.system_prompt,
        tools: final_tool_names,
        max_iterations: effective.max_iterations,
        // Inherit parent model's context window so subagents can run long tasks.
        // Previously hard-coded at 32000 tokens — far below modern 1M+ models.
        max_context_tokens: effective.max_context_tokens,
        result_strategy: effective.result_strategy,
        working_dir: Some(effective.working_dir),
        recursion_depth: effective.recursion_depth,
        skills: effective.skills,
        ..SubagentConfig::default()
    };

    let mut subagent = Subagent::new(
        id,
        config,
        model_config,
        tool_registry,
        effective.permission,
    );
    subagent = subagent.with_runtime_scope(
        parent_session_id.map(ToOwned::to_owned),
        parent_prompt_id.map(ToOwned::to_owned),
        parent_run_id.map(ToOwned::to_owned),
    );
    if let Some(sv) = supervisor {
        subagent = subagent.with_supervisor(sv);
    }
    if let Some(ct) = cancel_token {
        subagent = subagent.with_cancel_token(ct);
    }
    if let Some(resolver) = approval_resolver {
        subagent = subagent.with_approval_resolver(resolver);
    }

    let runtime_subagent_id = subagent.id().to_string();
    let run_result = subagent.run_with_sender(task, event_sender).await;
    let transcript_ref = subagent
        .transcript_path()
        .map(|path| path.display().to_string());

    // Collect subagent messages for session saving
    let messages = subagent.into_messages();

    if let (Some(mgr), Some((child_sid, child_pid))) = (session_mgr.as_ref(), &allocated) {
        let _ = mgr.finalize_subagent_session(child_sid, child_pid, id, &messages);
    }

    let result = match run_result {
        Ok(result) => result,
        Err(error) => {
            if let Some(path) = &transcript_ref {
                return Err(SpawnFailure {
                    runtime_id: Some(runtime_subagent_id),
                    transcript_ref: Some(path.clone()),
                    error,
                });
            }
            tracing::warn!(
                subagent_id = %runtime_subagent_id,
                "Failed to persist partial subagent transcript"
            );
            return Err(SpawnFailure {
                runtime_id: Some(runtime_subagent_id),
                transcript_ref: None,
                error,
            });
        }
    };

    // Build tool execution summary from the subagent's message history
    let tool_summary = build_tool_summary(&messages);

    Ok((
        SpawnResult {
            runtime_id: runtime_subagent_id,
            success: result.success,
            iterations_used: result.iterations_used,
            output: result.output,
            last_text: result.last_text.clone(),
            tool_count,
            tool_summary,
            transcript_ref,
        },
        messages,
    ))
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn persona_lookup_rejects_a_symlink_outside_its_root() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let persona_root = temp.path().join("agents");
        std::fs::create_dir(&persona_root).expect("persona root");
        let outside = temp.path().join("outside.md");
        std::fs::write(&outside, "secret").expect("outside persona");
        symlink(&outside, persona_root.join("reviewer.md")).expect("persona symlink");
        let key = PersonaKey::parse("reviewer").expect("valid persona key");
        assert!(read_persona_file(&persona_root, &key).await.is_none());
    }

    #[tokio::test]
    async fn parent_can_page_a_subagent_transcript_by_runtime_id() {
        use crate::subagent::transcript::{TranscriptOutcome, TranscriptRecorder};

        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
        let mut recorder = TranscriptRecorder::new_in(temp.path(), runtime_id).unwrap();
        recorder.set_scope(Some("session-1".into()), Some("run-1".into()));
        recorder
            .finalize(
                &[Message::user("task"), Message::assistant("answer")],
                TranscriptOutcome::Succeeded,
            )
            .unwrap();

        let tool = SubagentTranscriptTool::with_root(temp.path().to_path_buf());
        let page = tool
            .execute(serde_json::json!({
                "runtime_id": runtime_id,
                "offset": 1,
                "limit": 1,
                "_session_id": "session-1",
                "_parent_run_id": "run-1"
            }))
            .await
            .unwrap();
        let page: Value = serde_json::from_str(&page).unwrap();
        assert_eq!(page["total"], 2);
        assert_eq!(page["returned"], 1);
        assert_eq!(page["messages"][0]["content"], "answer");
    }

    #[test]
    fn requested_tools_are_strictly_bounded_by_parent_capabilities() {
        let args = serde_json::json!({ "tools": ["shell", "write_file"] });
        let selected = select_subagent_tools(&args, &names(&["read_file", "grep"]));
        assert!(selected.is_empty());
    }

    #[test]
    fn omitted_tools_inherit_parent_capabilities_but_not_meta_tools() {
        let args = serde_json::json!({});
        let selected = select_subagent_tools(
            &args,
            &names(&["read_file", "shell", "subagent", "skill_load"]),
        );
        assert_eq!(selected, names(&["read_file", "shell"]));
    }

    #[test]
    fn explicit_empty_tools_remains_empty() {
        let args = serde_json::json!({ "tools": [] });
        let selected = select_subagent_tools(&args, &names(&["read_file", "shell"]));
        assert!(selected.is_empty());
    }

    #[test]
    fn injected_parent_working_directory_is_not_widened() {
        let args = serde_json::json!({ "_working_dir": "/tmp/project-worktree/nested" });
        assert_eq!(
            effective_subagent_working_dir(&args),
            std::path::PathBuf::from("/tmp/project-worktree/nested")
        );
    }

    #[test]
    fn effective_skill_scripts_are_authorized_without_stale_parent_snapshot_entry() {
        let available = names(&["read_file"]);
        let skills = names(&["reporting"]);
        assert!(is_authorized_child_tool(
            "skill.reporting.export",
            &available,
            &skills
        ));
        assert!(!is_authorized_child_tool(
            "skill.unrelated.export",
            &available,
            &skills
        ));
    }

    #[test]
    fn skill_scripts_require_build_capabilities() {
        assert!(!child_allows_skill_scripts(&names(&["read_file", "grep"])));
        assert!(child_allows_skill_scripts(&names(&["read_file", "shell"])));
    }
}
