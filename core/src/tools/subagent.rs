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
) {
    let parent_max_iterations = model_config.max_iterations;
    registry.register(Box::new(SubagentSpawnTool::new(
        model_config.clone(),
        available_tool_names.clone(),
        session_mgr.clone(),
        permission_config.clone(),
        parent_max_iterations,
    )));
    registry.register(Box::new(SubagentSpawnAllTool::new(
        model_config,
        available_tool_names,
        session_mgr,
        permission_config,
        parent_max_iterations,
    )));
}

// ── SubagentSpawnTool ────────────────────────────────────────────────

struct SubagentSpawnTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
    permission_config: PermissionConfig,
    parent_max_iterations: usize,
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
        }
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
                    "description": "Tool names (default: read_file, glob, grep, bash). Use 'all' for all parent tools."
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
        let (result, _messages) = spawn_single(
            &args,
            &self.model_config,
            &self.available_tools,
            event_sender,
            &self.permission_config,
            self.parent_max_iterations,
            strategy,
        )
        .await?;

        // Save subagent session if session manager is available
        if let Some(ref mgr) = self.session_mgr {
            let mgr = mgr.lock();
            let _ = mgr.save_subagent("subagent", &result);
        }

        Ok(result.format_output(strategy))
    }
}

// ── SubagentSpawnAllTool (concurrent) ────────────────────────────────

struct SubagentSpawnAllTool {
    model_config: ModelConfig,
    available_tools: Vec<String>,
    session_mgr: Option<Arc<Mutex<SessionManager>>>,
    permission_config: PermissionConfig,
    parent_max_iterations: usize,
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
        }
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
Each sub-agent gets isolated context with access to: read_file, glob, grep, bash, edit, webfetch, git tools. \
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
                )
                .await;

                if let Some(ref mgr) = mgr_clone {
                    let mgr = mgr.lock();
                    if let Ok((ref sub_result, _)) = result {
                        let _ = mgr.save_subagent(&id, sub_result);
                    }
                }

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
                    output.push_str(&format!(
                        "[{}] {} — {}\n{}\n\n",
                        idx + 1,
                        id,
                        if sub_result.success {
                            "success"
                        } else {
                            "incomplete"
                        },
                        sub_result.format_output(*strategy)
                    ));
                }
                Ok((id, Err(e), _)) => {
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
        "bash" => summarise_bash(content),
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

fn summarise_bash(content: &str) -> String {
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

async fn spawn_single(
    args: &Value,
    model_config: &ModelConfig,
    available_tools: &[String],
    event_sender: Option<EventSender>,
    permission_config: &PermissionConfig,
    parent_max_iterations: usize,
    result_strategy: ResultStrategy,
) -> Result<(SpawnResult, Vec<crate::types::Message>)> {
    let id = args["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
    let task = args["task"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'task'"))?;

    let default_system_prompt = "You are a focused sub-agent. Complete the given task and return the result. Be concise. \
You have access to tools: read_file, glob, grep, bash, edit, webfetch, and git tools. \
CRITICAL: ALWAYS use the 'webfetch' tool to fetch web content. NEVER use bash with 'curl' or 'wget'. \
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
    let system_prompt = format!("{system_prompt}\n\nWorking Directory: {ws_root}");

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
            "bash".to_string(),
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

    // If all requested tools were filtered out, give at least read_file so it can do something.
    if final_tool_names.is_empty() {
        final_tool_names = vec!["read_file".to_string()];
    }

    let tool_count = final_tool_names.len();

    // Build real ToolRegistry with factory
    let tool_registry = ToolRegistry::from_names(&final_tool_names);

    let config = SubagentConfig {
        system_prompt,
        tools: final_tool_names,
        max_iterations,
        max_context_tokens: 32000,
        result_strategy,
        working_dir: Some(workspace_root.clone()),
        ..SubagentConfig::default()
    };

    let session_id = args.get("_session_id").and_then(|v| v.as_str()).map(|s| s.to_string());

    let mut subagent = Subagent::new(
        id,
        config,
        model_config,
        tool_registry,
        permission_config.clone(),
    );
    subagent.session_id = session_id;

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
