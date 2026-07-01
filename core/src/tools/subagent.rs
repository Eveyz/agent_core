use crate::config::ModelConfig;
use crate::permission::PermissionConfig;
use crate::session::SessionManager;
use crate::subagent::{Subagent, SubagentConfig};
use crate::tools::{Tool, ToolRegistry, ToolUpdateFn};
use crate::types::EventSender;
use anyhow::Result;
use parking_lot::Mutex;
use serde_json::Value;
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
        let (result, _messages) = spawn_single(
            &args,
            &self.model_config,
            &self.available_tools,
            event_sender,
            &self.permission_config,
            self.parent_max_iterations,
        )
        .await?;

        // Save subagent session if session manager is available
        if let Some(ref mgr) = self.session_mgr {
            let mgr = mgr.lock();
            let _ = mgr.save_subagent("subagent", &result);
        }

        Ok(result.summary())
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
                            "max_iterations": {"type": "integer", "description": "Max iterations"}
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
        let mut task_infos: Vec<(String, String, Vec<String>, usize)> = Vec::new();
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

            task_infos.push((id, task, tools, max_iterations));
        }

        // Spawn all subagents concurrently on a JoinSet so they can be
        // aborted if the parent tool execution is cancelled. Without this,
        // canceling the parent leaves child subagents running as detached
        // tasks (process leak).
        let mut join_set = tokio::task::JoinSet::new();
        let parent_max_iterations = self.parent_max_iterations;
        for (id, task, tools, max_iterations) in task_infos {
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
                )
                .await;

                if let Some(ref mgr) = mgr_clone {
                    let mgr = mgr.lock();
                    if let Ok((ref sub_result, _)) = result {
                        let _ = mgr.save_subagent(&id, sub_result);
                    }
                }

                (id, result)
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
                Ok((id, Ok((sub_result, _msgs)))) => {
                    output.push_str(&format!(
                        "[{}] {} — {}\n{}\n\n",
                        idx + 1,
                        id,
                        if sub_result.success {
                            "success"
                        } else {
                            "incomplete"
                        },
                        sub_result.summary()
                    ));
                }
                Ok((id, Err(e))) => {
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

// ── Shared spawn logic ───────────────────────────────────────────────

/// Result from spawning a single subagent.
struct SpawnResult {
    success: bool,
    iterations_used: usize,
    output: String,
    tool_count: usize,
}

impl crate::session::SubagentResultLike for SpawnResult {
    fn summary_for_session(&self) -> String {
        format!(
            "[Subagent] iterations={} success={} tools={}\n{}",
            self.iterations_used, self.success, self.tool_count, self.output
        )
    }
}

impl SpawnResult {
    fn summary(&self) -> String {
        format!(
            "[Sub-agent] ({} iterations, {} tools, {})\n\n{}",
            self.iterations_used,
            self.tool_count,
            if self.success {
                "success"
            } else {
                "incomplete"
            },
            self.output
        )
    }
}

async fn spawn_single(
    args: &Value,
    model_config: &ModelConfig,
    available_tools: &[String],
    event_sender: Option<EventSender>,
    permission_config: &PermissionConfig,
    parent_max_iterations: usize,
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

    // 1. Global Agent Persona
    let global_agent = std::path::Path::new(&home).join(format!(".agverse/agents/{}.md", id));
    if let Ok(c) = std::fs::read_to_string(&global_agent) {
        persona_content.push_str(&format!("Global Persona ({id}):\n{c}\n\n"));
    }

    // 2. Local/Project Agent Persona
    let local_agent = cwd.join(format!(".agverse/agents/{}.md", id));
    if let Ok(c) = std::fs::read_to_string(&local_agent) {
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

    Ok((
        SpawnResult {
            success: result.success,
            iterations_used: result.iterations_used,
            output: result.output,
            tool_count,
        },
        messages,
    ))
}
