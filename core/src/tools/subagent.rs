use crate::config::ModelConfig;
use crate::permission::PermissionConfig;
use crate::session::SessionManager;
use crate::subagent::{ResultStrategy, Subagent, SubagentConfig};
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
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
    permission_config: PermissionConfig,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
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
    registry.register(Box::new(single));
    registry.register(Box::new(spawn_all));
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
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
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
    );
}

/// Like [`re_wire_subagent_tools`], but also wires a shared [`SkillManager`]
/// so spawned subagents inherit parent session actives.
pub fn re_wire_subagent_tools_with_skills(
    registry: &mut ToolRegistry,
    model_config: ModelConfig,
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
    permission_config: PermissionConfig,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
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
    );
}

// ── SubagentSpawnTool ────────────────────────────────────────────────

pub(crate) struct SubagentSpawnTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
    permission_config: PermissionConfig,
    parent_max_iterations: usize,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Recursion depth of the agent that owns this tool. When this tool
    /// spawns a subagent, the child gets `parent_depth + 1` and the spawn
    /// is refused past `MAX_SUBAGENT_DEPTH`.
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
}

impl SubagentSpawnTool {
    fn new(
        model_config: ModelConfig,
        available_tools: Vec<String>,
        session_mgr: Option<Arc<Mutex<SessionManager>>>,
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
        }
    }

    pub fn with_supervisor(mut self, sv: Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>) -> Self {
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

        let (result, messages) = spawn_single(
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
        )
        .await?;

        // Persist full subagent conversation to disk so the parent context
        // stays small (cache-friendly) while preserving the complete history.
        let file_ref = persist_subagent_messages(&id, &messages)
            .await
            .map(|p| format!("\n\n---\n⚠️ Full subagent messages persisted to: {}", p.display()))
            .unwrap_or_default();

        // Save subagent session if session manager is available
        if let Some(ref mgr) = self.session_mgr {
            let mgr = mgr.lock();
            let _ = mgr.save_subagent_with_messages(&id, &messages);
        }

        Ok(format!("{}{}", result.format_output(strategy), file_ref))
    }
}

// ── SubagentSpawnAllTool (concurrent) ────────────────────────────────

pub(crate) struct SubagentSpawnAllTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
    permission_config: PermissionConfig,
    parent_max_iterations: usize,
    supervisor: Option<Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>>,
    cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Recursion depth of the agent that owns this tool.
    parent_depth: u8,
    skill_manager: Option<Arc<Mutex<crate::skills::SkillManager>>>,
}

impl SubagentSpawnAllTool {
    fn new(
        model_config: ModelConfig,
        available_tools: Vec<String>,
        session_mgr: Option<Arc<Mutex<SessionManager>>>,
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
        }
    }

    pub fn with_supervisor(mut self, sv: Arc<Mutex<crate::runtime::supervisor::ProcessSupervisor>>) -> Self {
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
        let mut task_infos: Vec<(String, String, Vec<String>, usize, ResultStrategy)> = Vec::new();
        for task_spec in tasks {
            let id = task_spec["id"].as_str().unwrap_or("unknown").to_string();
            let task = task_spec["task"].as_str().unwrap_or("").to_string();
            let tools: Vec<String> = task_spec["tools"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
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
        for (id, task, tools, max_iterations, strategy) in task_infos {
            let model_config = self.model_config.clone();
            let permission_config = self.permission_config.clone();
            let available_tools = if tools.is_empty() {
                self.available_tools.clone()
            } else {
                tools
            };

            let mgr_clone = self.session_mgr.clone();
            let sub_sender = event_sender.clone();
            let sv_clone = self.supervisor.clone();
            let ct_clone = self.cancel_token.clone();
            let parent_depth = self.parent_depth;
            let skill_manager = self.skill_manager.clone();

            join_set.spawn(async move {
                let args = serde_json::json!({
                    "id": id.clone(),
                    "task": task,
                    "tools": available_tools,
                    "max_iterations": max_iterations,
                });

                let result = spawn_single(
                    &args,
                    &model_config,
                    &available_tools,
                    sub_sender,
                    &permission_config,
                    parent_max_iterations,
                    strategy,
                    sv_clone,
                    ct_clone,
                    parent_depth,
                    skill_manager,
                )
                .await;

                // Persist messages to file (cache-friendly: parent context stays small).
                let file_ref = match &result {
                    Ok((_, messages)) => persist_subagent_messages(&id, messages)
                        .await
                        .map(|p| p.display().to_string()),
                    Err(_) => None,
                };

                if let Some(ref mgr) = mgr_clone {
                    let mgr = mgr.lock();
                    if let Ok((_, ref messages)) = result {
                        let _ = mgr.save_subagent_with_messages(&id, messages);
                    }
                }

                (id, result, strategy, file_ref)
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
                Ok((id, Ok((sub_result, _msgs)), strategy, file_ref)) => {
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
                    if let Some(path) = file_ref {
                        entry.push_str(&format!(
                            "⚠️ Full messages persisted to: {path}\n"
                        ));
                    }
                    entry.push('\n');
                    output.push_str(&entry);
                }
                Ok((id, Err(e), _, _)) => {
                    output.push_str(&format!("[{}] {} — ERROR: {}\n\n", idx + 1, id, e));
                }
                Err(e) => {
                    output.push_str(&format!("[{}] — JOIN ERROR: {}\n\n", idx + 1, e));
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
                    let (call_name, call_args) = pending_calls.pop_front().unwrap_or_else(|| {
                        (tool_name.clone(), String::new())
                    });
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
    let file_set: HashSet<&str> = lines
        .iter()
        .filter_map(|l| l.split(':').next())
        .collect();
    let total_lines = lines.len();
    let sample: Vec<&str> = lines.iter().take(8).copied().collect();
    let sample_str = truncate_str(&sample.join(" | "), 800);
    format!(
        "found {} matches in {} file(s). samples: {}",
        total_lines, file_set.len(), sample_str
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
    format!(
        "[{} lines, {} chars] {}",
        lines, chars, snippet
    )
}

fn summarise_shell(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let first: Vec<&str> = lines.iter().take(10).copied().collect();
    let s = format!(
        "[{} lines] {}",
        total,
        first.join("\n  ")
    );
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

// ── Subagent message persistence ─────────────────────────────────────

/// Write the full subagent conversation (`messages`) to
/// `~/.agverse/subagents/{agent_id}_{ts}.messages.json`.
/// Returns the absolute path on success so it can be included in the
/// parent-agent tool result as a pointer.  The parent context stays small
/// (cache-friendly), while the full history is preserved on disk.
///
/// Runs file I/O on a blocking thread so we don't stall the async runtime.
pub(crate) async fn persist_subagent_messages(
    agent_id: &str,
    messages: &[Message],
) -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let json = serde_json::to_string_pretty(messages).ok()?;
    let msg_count = messages.len();

    let agent_id = agent_id.to_string();
    tokio::task::spawn_blocking(move || -> Option<std::path::PathBuf> {
        let dir = std::path::PathBuf::from(&home)
            .join(".agverse")
            .join("subagents");
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }

        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = format!("{}_{}.messages.json", agent_id, ts);
        let path = dir.join(&filename);

        if std::fs::write(&path, &json).is_err() {
            return None;
        }

        tracing::info!(
            agent_id = %agent_id,
            path = %path.display(),
            msg_count = msg_count,
            "Persisted subagent messages"
        );

        Some(path)
    })
    .await
    .ok()
    .flatten()
}

// ── Shared spawn logic ───────────────────────────────────────────────

/// Result from spawning a single subagent.
struct SpawnResult {
    success: bool,
    iterations_used: usize,
    output: String,
    /// Text from only the final assistant turn.
    last_text: String,
    tool_count: usize,
    tool_summary: String,
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
    /// Legacy summary format (all text + tool summary). Used by Auto strategy.
    fn summary(&self) -> String {
        let mut s = format!(
            "[Sub-agent] ({} iterations, {} tools, {})\n\n{}",
            self.iterations_used,
            self.tool_count,
            if self.success {
                "success"
            } else {
                "incomplete"
            },
            self.output
        );
        if !self.tool_summary.is_empty() {
            s.push_str("\n\n--- Tool Execution Summary ---\n");
            s.push_str(&self.tool_summary);
        }
        s
    }

    /// Format the subagent output according to the chosen ResultStrategy.
    fn format_output(&self, strategy: ResultStrategy) -> String {
        match strategy {
            ResultStrategy::Full => {
                // Full: return last-turn text + tool_summary so the main agent
                // sees both the subagent's final analysis AND the raw tool data.
                let content = if self.last_text.is_empty() {
                    &self.output
                } else {
                    &self.last_text
                };
                let mut s = format!(
                    "[Sub-agent] ({} iterations, {} tools, {})\n\n{}",
                    self.iterations_used,
                    self.tool_count,
                    if self.success { "success" } else { "incomplete" },
                    content
                );
                if !self.tool_summary.is_empty() {
                    s.push_str("\n\n--- Tool Execution Summary ---\n");
                    s.push_str(&self.tool_summary);
                }
                s
            }
            ResultStrategy::Summary => {
                // Summary: return only the last-turn text (the system prompt
                // already instructed the subagent to summarise).
                let content = if self.last_text.is_empty() {
                    &self.output
                } else {
                    &self.last_text
                };
                format!(
                    "[Sub-agent] ({} iterations, {} tools, {})\n\n{}",
                    self.iterations_used,
                    self.tool_count,
                    if self.success { "success" } else { "incomplete" },
                    content
                )
            }
            ResultStrategy::Auto => self.summary(),
        }
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
    matches!(
        name,
        "subagent"
            | "subagents"
            | "skill_list"
            | "skill_load"
            | "skill_deactivate"
            | "skill_reload"
    )
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
) -> Result<(SpawnResult, Vec<crate::types::Message>)> {
    use crate::skills::SkillManager;
    use crate::subagent::MAX_SUBAGENT_DEPTH;

    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
    let task = args["task"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'task'"))?;

    // Hard cap on recursion depth: don't let a subagent spawn another if we
    // are already at MAX_SUBAGENT_DEPTH. Returning Err propagates up to the
    // parent LLM, which can decide to inline the work instead.
    if parent_depth >= MAX_SUBAGENT_DEPTH {
        return Err(anyhow::anyhow!(
            "Refusing to spawn subagent '{}' at recursion depth {}: \
             that would exceed the max depth of {}. \
             Do the work inline in the parent context instead.",
            id, parent_depth, MAX_SUBAGENT_DEPTH,
        ));
    }
    let child_depth = parent_depth + 1;

    let default_system_prompt = "You are a focused sub-agent. Complete the given task and return the result. Be concise. \
You have access to tools: read_file, glob, grep, shell, edit, webfetch, and git tools. \
CRITICAL: ALWAYS use the 'webfetch' tool to fetch web content. NEVER use shell with 'curl' or 'wget'. \
Do NOT attempt to read or process image files.";

    let mut persona_content = String::new();
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    // Find the workspace / project root by walking up from CWD until we
    // find a Cargo.toml. This is the directory subagent tools should use
    // as their effective working directory so relative paths resolve
    // correctly — the process CWD (e.g. app/) may be nested inside the
    // workspace and miss sibling crates.
    let workspace_root = find_workspace_root(&cwd);

    // 1. Global Agent Persona
    let global_agent = std::path::Path::new(&home).join(format!(".agverse/agents/{}.md", id));
    if let Ok(c) = tokio::fs::read_to_string(&global_agent).await {
        persona_content.push_str(&format!("Global Persona ({id}):\n{c}\n\n"));
    }

    // 2. Local/Project Agent Persona
    let local_agent = cwd.join(format!(".agverse/agents/{}.md", id));
    if let Ok(c) = tokio::fs::read_to_string(&local_agent).await {
        persona_content.push_str(&format!("Project Persona ({id}):\n{c}\n\n"));
    }

    let base_prompt = args["system_prompt"]
        .as_str()
        .unwrap_or(default_system_prompt)
        .to_string();

    let system_prompt = if persona_content.is_empty() {
        base_prompt
    } else {
        format!("{}\n\n=== Subagent Persona ===\n{}", base_prompt, persona_content)
    };

    // Inject the workspace root into the subagent's context so it knows
    // where the project lives and can resolve paths relative to the root.
    // We walk up from the process CWD to find the Cargo.toml workspace root
    // because the process CWD is often a nested directory (e.g. app/) while
    // sibling crates (core/, cli/) live at the workspace level.
    let ws_root = workspace_root.to_string_lossy().to_string();
    let mut system_prompt = format!("{system_prompt}\n\nWorking Directory: {ws_root}");

    let session_id = args.get("_session_id").and_then(|v| v.as_str());

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
        mgr.resolve_subagent_skills(&declared, session_id)
    } else {
        declared
    };
    if !effective_skills.is_empty() {
        system_prompt = SkillManager::inject_skill_content_into(
            skill_manager.as_ref(),
            &effective_skills,
            &system_prompt,
        );
    }

    let max_iterations = args["max_iterations"].as_u64().unwrap_or(parent_max_iterations as u64) as usize;

    // Determine tool names from args
    let tool_names: Vec<String> = if let Some(arr) = args["tools"].as_array() {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    } else {
        vec![
            "read_file".to_string(),
            "glob".to_string(),
            "grep".to_string(),
            "shell".to_string(),
            "edit".to_string(),
            "webfetch".to_string(),
        ]
    };

    // Check for "all" wildcard
    let is_all = args["tools"]
        .as_str()
        .map(|s| s == "all")
        .or_else(|| {
            args["tools"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .map(|s| s == "all")
        })
        .unwrap_or(false);

    let mut final_tool_names = if is_all {
        available_tools.to_vec()
    } else if tool_names.is_empty() {
        // Agent explicitly passed empty tools — respect that, but subagent can only think
        vec!["read_file".to_string()] // give at least read ability
    } else {
        tool_names
    };

    // Prevent subagents from getting tools the parent agent doesn't have.
    final_tool_names.retain(|t| available_tools.contains(t));

    // Strip meta-dispatch tools so a spawned subagent cannot recursively
    // spawn its own subagents or trigger skill loading — those operations
    // must remain under the explicit control of the parent LLM (or workflow
    // executor). This is the second layer of defence beyond the recursion
    // depth check above: even if a subagent somehow gets past the check
    // (e.g. via the `"all"` wildcard), the tool name won't be available.
    final_tool_names.retain(|t| !is_meta_dispatch_tool(t));

    // If all requested tools were filtered out, give at least read_file so it can do something.
    if final_tool_names.is_empty() {
        final_tool_names = vec!["read_file".to_string()];
    }

    let tool_count = final_tool_names.len();

    // Build real ToolRegistry with factory
    let mut tool_registry = ToolRegistry::from_names(&final_tool_names);
    if !effective_skills.is_empty() {
        SkillManager::sync_skill_scripts_for_skills(
            skill_manager.as_ref(),
            &effective_skills,
            &mut tool_registry,
            supervisor.clone(),
        );
    }

    let config = SubagentConfig {
        system_prompt,
        tools: final_tool_names,
        max_iterations,
        // Inherit parent model's context window so subagents can run long tasks.
        // Previously hard-coded at 32000 tokens — far below modern 1M+ models.
        max_context_tokens: model_config.max_context_tokens,
        result_strategy,
        working_dir: Some(workspace_root.clone()),
        recursion_depth: child_depth,
        skills: effective_skills,
        ..SubagentConfig::default()
    };

    let mut subagent = Subagent::new(
        id,
        config,
        model_config,
        tool_registry,
        permission_config.clone(),
    );
    subagent.session_id = session_id.map(|s| s.to_string());
    if let Some(sv) = supervisor {
        subagent = subagent.with_supervisor(sv);
    }
    if let Some(ct) = cancel_token {
        subagent = subagent.with_cancel_token(ct);
    }

    let result = subagent.run_with_sender(task, event_sender).await?;

    // Collect subagent messages for session saving
    let messages = subagent.into_messages();

    // Build tool execution summary from the subagent's message history
    let tool_summary = build_tool_summary(&messages);

    Ok((
        SpawnResult {
            success: result.success,
            iterations_used: result.iterations_used,
            output: result.output,
            last_text: result.last_text.clone(),
            tool_count,
            tool_summary,
        },
        messages,
    ))
}
