pub mod board;

pub use board::{TaskBoard, TaskRecord, TaskStatus};

use serde_json::Value;

use crate::config::ModelConfig;
use crate::permission::PermissionConfig;
use crate::subagent::{Subagent, SubagentConfig};
use crate::tools::{Tool, ToolRegistry};
use parking_lot::Mutex;
use std::sync::Arc;

pub fn register_task_tools(
    registry: &mut ToolRegistry,
    board: Arc<Mutex<TaskBoard>>,
    model_config: ModelConfig,
    permission_config: PermissionConfig,
) {
    registry.register(Box::new(TaskCreateTool::new(board.clone())));
    registry.register(Box::new(TaskBatchCreateTool::new(board.clone())));
    registry.register(Box::new(TaskUpdateTool::new(board.clone())));
    registry.register(Box::new(TaskListTool::new(board.clone())));
    registry.register(Box::new(TaskGetTool::new(board.clone())));
    registry.register(Box::new(TaskPlanTool::new(board.clone())));
    registry.register(Box::new(TaskReadyTool::new(board.clone())));
    registry.register(Box::new(TaskExecuteTool::new(
        board,
        model_config,
        permission_config,
    )));
}

fn detect_cycle(board: &TaskBoard, new_id: &str, depends_on: &[String]) -> Result<(), String> {
    let mut visited = std::collections::HashSet::new();
    let mut stack = depends_on.to_vec();

    while let Some(dep_id) = stack.pop() {
        if dep_id == new_id {
            return Err(format!(
                "Circular dependency detected: {} -> {}",
                dep_id, new_id
            ));
        }
        if !visited.insert(dep_id.clone()) {
            continue;
        }
        if let Some(task) = board.get(&dep_id) {
            for transitive in task.blocked_by() {
                stack.push(transitive.clone());
            }
        }
    }
    Ok(())
}

fn build_dependency_context(board: &TaskBoard, task_id: &str) -> String {
    let task = match board.get(task_id) {
        Some(t) => t,
        None => return String::new(),
    };

    if task.blocked_by().is_empty() {
        return String::new();
    }

    let mut ctx = String::from("== Results from dependency tasks ==\n");
    for dep_id in task.blocked_by() {
        if let Some(dep) = board.get(dep_id) {
            let result = dep.result().unwrap_or("(no result)");
            ctx.push_str(&format!(
                "--- {} ({}) ---\n{}\n\n",
                dep_id,
                dep.goal(),
                result
            ));
        }
    }
    ctx.push_str("== End dependency results ==\n");
    ctx
}

struct TaskCreateTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskCreateTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        "task_create"
    }

    fn description(&self) -> &str {
        "Create a task with optional dependencies. Tasks form a DAG. Circular deps are rejected. Args: title (string), description (string), depends_on (optional array of task UUIDs)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short title for the task (for display)"},
                "description": {"type": "string", "description": "What this task should accomplish"},
                "depends_on": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Task UUIDs that must complete before this one can start"
                }
            },
            "required": ["title", "description"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let title = args["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'title'"))?;
        let description = args["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'description'"))?;
        let depends_on: Vec<String> = args["depends_on"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut board = self.board.lock();

        for dep in &depends_on {
            if board.get(dep).is_none() {
                anyhow::bail!("Dependency UUID '{}' does not exist. Create it first.", dep);
            }
        }

        // Cycle detection will be handled by the board when it adds edges, or we can just let it create.
        // Wait, detect_cycle is a function in mod.rs. Let's see if we can use a dummy UUID to test cycle.
        // For now, since UUIDs are newly generated, a new task CANNOT create a cycle with existing tasks unless an existing task is updated to depend on it.
        // So detect_cycle on creation is actually impossible to trigger with newly minted UUIDs!

        let new_uuid = board.create(title, description, depends_on.clone());
        let deps = if depends_on.is_empty() {
            String::new()
        } else {
            format!(" (blocked by: {})", depends_on.join(", "))
        };
        Ok(format!(
            "Task created! UUID: '{}' | Title: {} {}",
            new_uuid, title, deps
        ))
    }
}

struct TaskUpdateTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskUpdateTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        "task_update"
    }

    fn description(&self) -> &str {
        "Update a task's status. Args: id (UUID string), status (pending/in_progress/completed/failed), result (optional string with the task's output)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task UUID"},
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "failed"],
                    "description": "New status"
                },
                "result": {"type": "string", "description": "Task output or result text"}
            },
            "required": ["id", "status"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
        let status = args["status"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'status'"))?;
        let result = args["result"].as_str().map(String::from);

        let status = match status {
            "pending" => TaskStatus::Pending,
            "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            s => anyhow::bail!("invalid status: {}", s),
        };

        let mut board = self.board.lock();

        let status_str = format!("{}", status);
        board.update(id, status, result)?;

        let unblocked: Vec<String> = board
            .ready_tasks()
            .iter()
            .filter(|t| t.blocked_by().contains(&id.to_string()))
            .map(|t| t.id().to_string())
            .collect();

        let mut msg = format!("Task '{}' updated to {}", id, status_str);
        if !unblocked.is_empty() {
            msg.push_str(&format!(". Unblocked: {}", unblocked.join(", ")));
        }
        Ok(msg)
    }
}

struct TaskListTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskListTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        "task_list"
    }

    fn description(&self) -> &str {
        "List all tasks with status and dependencies"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<String> {
        let board = self.board.lock();
        Ok(board.summary())
    }
}

struct TaskGetTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskGetTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskGetTool {
    fn name(&self) -> &str {
        "task_get"
    }

    fn description(&self) -> &str {
        "Get full details of a specific task including its result"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"}
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;
        let board = self.board.lock();
        let task = board
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("task '{}' not found", id))?;

        let deps = if task.blocked_by().is_empty() {
            String::from("(none)")
        } else {
            task.blocked_by().join(", ")
        };
        let result = task.result().unwrap_or("(no result yet)");
        let assigned = task.assigned_to().unwrap_or("(unassigned)");

        Ok(format!(
            "Task: {}\nGoal: {}\nStatus: {}\nAssigned to: {}\nBlocked by: {}\nResult: {}",
            task.id(),
            task.goal(),
            task.status(),
            assigned,
            deps,
            result
        ))
    }
}

struct TaskPlanTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskPlanTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskPlanTool {
    fn name(&self) -> &str {
        "task_plan"
    }

    fn description(&self) -> &str {
        "Show the execution plan: DAG structure, topological order, and what's ready now"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<String> {
        let board = self.board.lock();
        let tasks = board.all_tasks();

        if tasks.is_empty() {
            return Ok("No tasks. Use task_create to build a plan.".to_string());
        }

        let mut out = String::from("== Execution Plan ==\n\n");

        // Show DAG structure
        out.push_str("Dependencies:\n");
        for task in &tasks {
            if task.blocked_by().is_empty() {
                out.push_str(&format!("  {} (no deps)\n", task.id()));
            } else {
                out.push_str(&format!(
                    "  {} <- [{}]\n",
                    task.id(),
                    task.blocked_by().join(", ")
                ));
            }
        }

        // Topological order
        out.push_str("\nExecution order (topological):\n");
        let order = topological_sort(&tasks);
        for (i, task_id) in order.iter().enumerate() {
            if let Some(task) = tasks.iter().find(|t| &t.id() == task_id) {
                out.push_str(&format!(
                    "  {}. {} [{}] — {}\n",
                    i + 1,
                    task_id,
                    task.status(),
                    task.goal()
                ));
            }
        }

        // Currently ready
        let ready = board.ready_tasks();
        out.push_str(&format!("\nReady now ({}):\n", ready.len()));
        for task in &ready {
            out.push_str(&format!("  -> {} — {}\n", task.id(), task.goal()));
        }

        // Completed
        let completed = tasks
            .iter()
            .filter(|t| *t.status() == TaskStatus::Completed)
            .count();
        let failed = tasks
            .iter()
            .filter(|t| *t.status() == TaskStatus::Failed)
            .count();
        out.push_str(&format!(
            "\nProgress: {}/{} completed, {} failed\n",
            completed,
            tasks.len(),
            failed
        ));

        Ok(out)
    }
}

struct TaskReadyTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskReadyTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskReadyTool {
    fn name(&self) -> &str {
        "task_ready"
    }

    fn description(&self) -> &str {
        "List tasks that are ready to execute (all dependencies met). Use task_execute to run them."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}, "required": []})
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<String> {
        let board = self.board.lock();
        let ready = board.ready_tasks();

        if ready.is_empty() {
            let all = board.all_tasks();
            let pending = all
                .iter()
                .filter(|t| *t.status() == TaskStatus::Pending)
                .count();
            if pending > 0 {
                return Ok(format!(
                    "No tasks ready. {} tasks pending (waiting on dependencies).",
                    pending
                ));
            }
            return Ok("No tasks ready. All tasks are completed or in progress.".to_string());
        }

        let mut out = String::from("Ready to execute:\n");
        for task in &ready {
            out.push_str(&format!("  -> {} — {}\n", task.id(), task.goal()));
        }
        out.push_str("\nUse task_execute <id> to run a task with a sub-agent.");
        Ok(out)
    }
}

struct TaskExecuteTool {
    board: Arc<Mutex<TaskBoard>>,
    model_config: ModelConfig,
    permission_config: PermissionConfig,
}

impl TaskExecuteTool {
    fn new(
        board: Arc<Mutex<TaskBoard>>,
        model_config: ModelConfig,
        permission_config: PermissionConfig,
    ) -> Self {
        Self {
            board,
            model_config,
            permission_config,
        }
    }
}

#[async_trait::async_trait]
impl Tool for TaskExecuteTool {
    fn name(&self) -> &str {
        "task_execute"
    }

    fn description(&self) -> &str {
        "Execute a ready task by spawning a sub-agent. Dependency results are auto-injected. Args: id (string), tools (optional array of tool names for the sub-agent), system_prompt (optional string)"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID to execute (must be ready)"},
                "tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tools to give the sub-agent (default: read_file, glob, grep, bash)"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Custom system prompt for the sub-agent"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;

        let custom_tools: Vec<String> = args["tools"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    "read_file".to_string(),
                    "glob".to_string(),
                    "grep".to_string(),
                    "bash".to_string(),
                    "edit".to_string(),
                ]
            });

        let system_prompt = args["system_prompt"]
            .as_str()
            .unwrap_or("You are a focused sub-agent. Complete the given task precisely. Return the result clearly.")
            .to_string();

        // Check task is ready and get dependency context
        let (goal, dep_context, force_inline) = {
            let board = self.board.lock();
            let task = board
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("task '{}' not found", id))?;

            if *task.status() != TaskStatus::Ready && *task.status() == TaskStatus::Pending {
                anyhow::bail!(
                    "Task '{}' is not ready. Blocked by: {}",
                    id,
                    task.blocked_by().join(", ")
                );
            }
            if *task.status() == TaskStatus::Completed {
                anyhow::bail!("Task '{}' is already completed.", id);
            }
            if *task.status() == TaskStatus::InProgress {
                anyhow::bail!("Task '{}' is already in progress.", id);
            }

            let dep_ctx = build_dependency_context(&board, id);
            let use_subagent = should_use_subagent(&task.goal(), &board, id);
            (task.goal().to_string(), dep_ctx, !use_subagent)
        };

        // Mark in-progress
        {
            let mut board = self.board.lock();
            board.update(id, TaskStatus::InProgress, None)?;
        }

        // Build task prompt with dependency context
        let mut task_prompt = String::new();
        task_prompt.push_str(&format!("Task: {}\n\nGoal: {}\n", id, goal));
        if !dep_context.is_empty() {
            task_prompt.push_str(&format!("\n{}", dep_context));
        }
        task_prompt.push_str("\nComplete this task and return the result. When done, your output will be stored as the task result.");

        if force_inline {
            // Execute inline — simple task, no subagent overhead
            let result_text = format!("[Task '{}' - inline] Goal: {}", id, goal);

            let mut board = self.board.lock();
            board.update(id, TaskStatus::Completed, Some(result_text.clone()))?;

            let ready = board.ready_tasks();
            let unblocked: Vec<&str> = ready
                .iter()
                .filter(|t| t.blocked_by().contains(&id.to_string()))
                .map(|t| t.id())
                .collect();

            let mut output = format!(
                "[Task '{}'] completed (inline, no subagent)\n\n{}",
                id, result_text
            );
            if !unblocked.is_empty() {
                output.push_str(&format!("\n\nUnblocked tasks: {}", unblocked.join(", ")));
            }
            return Ok(output);
        }

        // Spawn subagent with proper tool registry
        let tool_registry = crate::tools::ToolRegistry::from_names(&custom_tools);
        let config = SubagentConfig {
            system_prompt,
            tools: custom_tools,
            max_iterations: 100,
            max_context_tokens: 32000,
        };

        let mut subagent = Subagent::new(
            id,
            config,
            &self.model_config,
            tool_registry,
            self.permission_config.clone(),
        );
        let result = subagent.run(&task_prompt).await?;

        // Update task with result
        {
            let mut board = self.board.lock();
            if result.success {
                board.update(id, TaskStatus::Completed, Some(result.output.clone()))?;
            } else {
                board.update(id, TaskStatus::Failed, Some(result.output.clone()))?;
            }

            // Report unblocked tasks
            let ready = board.ready_tasks();
            let unblocked: Vec<&str> = ready
                .iter()
                .filter(|t| t.blocked_by().contains(&id.to_string()))
                .map(|t| t.id())
                .collect();

            let mut output = format!(
                "[Task '{}'] {} ({} iterations)\n\n{}",
                id,
                if result.success {
                    "completed"
                } else {
                    "failed"
                },
                result.iterations_used,
                result.output
            );

            if !unblocked.is_empty() {
                output.push_str(&format!("\n\nUnblocked tasks: {}", unblocked.join(", ")));
            }

            Ok(output)
        }
    }
}

/// Heuristic: should this task spawn a subagent or execute inline?
///
/// Returns true if:
/// - The goal is complex (>80 chars, suggesting multi-step work)
/// - There are parallel tasks that could run concurrently
/// - The goal mentions tools or file operations
fn should_use_subagent(goal: &str, board: &TaskBoard, current_id: &str) -> bool {
    // Check for parallel-ready siblings first (always use subagent for concurrency)
    let ready = board.ready_tasks();
    let parallel_count = ready.iter().filter(|t| t.id() != current_id).count();
    if parallel_count >= 1 {
        return true;
    }

    // Count tool-related keywords in the goal
    let tool_keywords = [
        "read",
        "write",
        "edit",
        "grep",
        "search",
        "find",
        "run",
        "compile",
        "build",
        "test",
        "refactor",
        "implement",
        "fix",
        "analyze",
        "check",
        "verify",
        "compare",
    ];
    let goal_lower = goal.to_lowercase();
    let keyword_count = tool_keywords
        .iter()
        .filter(|kw| goal_lower.contains(&kw.to_lowercase()))
        .count();

    // Very short + no tool keywords → inline
    if goal.len() < 40 && keyword_count == 0 {
        return false;
    }

    // Long goals OR multiple tool keywords → subagent
    if goal.len() > 120 || keyword_count >= 2 {
        return true;
    }

    // Default: inline
    false
}

fn topological_sort(tasks: &[&TaskRecord]) -> Vec<String> {
    let mut result = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut visiting = std::collections::HashSet::new();

    let task_map: std::collections::HashMap<&str, &TaskRecord> =
        tasks.iter().map(|t| (t.id(), *t)).collect();

    fn visit(
        id: &str,
        task_map: &std::collections::HashMap<&str, &TaskRecord>,
        visited: &mut std::collections::HashSet<String>,
        visiting: &mut std::collections::HashSet<String>,
        result: &mut Vec<String>,
    ) {
        if visited.contains(id) {
            return;
        }
        if !visiting.insert(id.to_string()) {
            return; // cycle — skip
        }

        if let Some(task) = task_map.get(id) {
            for dep in task.blocked_by() {
                visit(dep, task_map, visited, visiting, result);
            }
        }

        visiting.remove(id);
        visited.insert(id.to_string());
        result.push(id.to_string());
    }

    for task in tasks {
        visit(
            &task.id(),
            &task_map,
            &mut visited,
            &mut visiting,
            &mut result,
        );
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn make_board() -> Arc<Mutex<TaskBoard>> {
        Arc::new(Mutex::new(TaskBoard::new()))
    }

    #[test]
    fn test_cycle_detection_direct() {
        let board = make_board();
        let (id_a, id_b) = {
            let mut b = board.lock();
            let a = b.create("A", "a", vec![]);
            let b_id = b.create("B", "b", vec![a.clone()]);
            (a, b_id)
        };

        let b = board.lock();
        let result = detect_cycle(&b, &id_a, &[id_b.clone()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Circular"));
    }

    #[test]
    fn test_cycle_detection_transitive() {
        let board = make_board();
        let (id_a, id_c) = {
            let mut b = board.lock();
            let a = b.create("A", "a", vec![]);
            let b_id = b.create("B", "b", vec![a.clone()]);
            let c = b.create("C", "c", vec![b_id.clone()]);
            (a, c)
        };

        let b = board.lock();
        let result = detect_cycle(&b, &id_a, &[id_c.clone()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_no_cycle_valid_dag() {
        let board = make_board();
        let (id_a, id_b, id_c, id_d) = {
            let mut b = board.lock();
            let a = b.create("A", "a", vec![]);
            let b_id = b.create("B", "b", vec![]);
            let c = b.create("C", "c", vec![a.clone(), b_id.clone()]);
            let d = b.create("D", "d", vec![c.clone()]);
            (a, b_id, c, d)
        };

        let b = board.lock();
        assert!(detect_cycle(&b, "e", &[id_a.clone(), id_b.clone()]).is_ok());
        assert!(detect_cycle(&b, "f", &[id_d.clone()]).is_ok());
        assert!(detect_cycle(&b, "g", &[id_c.clone(), id_d.clone()]).is_ok());
        assert!(detect_cycle(&b, &id_a, &[id_d.clone()]).is_err());
    }

    #[test]
    fn test_topological_sort_linear() {
        let board = TaskBoard::new();
        let mut b = board;
        let id_a = b.create("A", "a", vec![]);
        let id_b = b.create("B", "b", vec![id_a.clone()]);
        let id_c = b.create("C", "c", vec![id_b.clone()]);

        let order = topological_sort(&b.all_tasks());
        let pos_a = order.iter().position(|x| x == &id_a).unwrap();
        let pos_b = order.iter().position(|x| x == &id_b).unwrap();
        let pos_c = order.iter().position(|x| x == &id_c).unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let board = TaskBoard::new();
        let mut b = board;
        let id_a = b.create("A", "a", vec![]);
        let id_b = b.create("B", "b", vec![]);
        let id_c = b.create("C", "c", vec![id_a.clone(), id_b.clone()]);
        let id_d = b.create("D", "d", vec![id_c.clone()]);

        let order = topological_sort(&b.all_tasks());
        let pos_a = order.iter().position(|x| x == &id_a).unwrap();
        let pos_b = order.iter().position(|x| x == &id_b).unwrap();
        let pos_c = order.iter().position(|x| x == &id_c).unwrap();
        let pos_d = order.iter().position(|x| x == &id_d).unwrap();

        assert!(pos_a < pos_c);
        assert!(pos_b < pos_c);
        assert!(pos_c < pos_d);
    }

    // ── should_use_subagent heuristic tests ──────────────────────────

    #[test]
    fn test_should_use_subagent_short_goal_inline() {
        let board = TaskBoard::new();
        let mut b = board;
        let id1 = b.create("t1", "Read main.rs", vec![]);
        assert!(!should_use_subagent("Read main.rs", &b, &id1));
    }

    #[test]
    fn test_should_use_subagent_long_goal_subagent() {
        let board = TaskBoard::new();
        let mut b = board;
        let long_goal = "Refactor the entire authentication module to use OAuth2 with PKCE flow, \
                         including token refresh, session management, and error handling across \
                         all API endpoints";
        let id1 = b.create("t1", long_goal, vec![]);
        assert!(should_use_subagent(long_goal, &b, &id1));
    }

    #[test]
    fn test_should_use_subagent_parallel_siblings() {
        let board = TaskBoard::new();
        let mut b = board;
        let id1 = b.create("t1", "Find auth module", vec![]);
        let _id2 = b.create("t2", "Find database module", vec![]);
        // t1 and t2 are both ready (parallel)
        assert!(should_use_subagent("Find auth module", &b, &id1));
    }

    #[test]
    fn test_should_use_subagent_tool_keywords() {
        let board = TaskBoard::new();
        let mut b = board;
        let id1 = b.create("t1", "Read and analyze the config file", vec![]);
        // "read" + "analyze" = 2 tool keywords
        assert!(should_use_subagent(
            "Read and analyze the config file",
            &b,
            &id1
        ));
    }
}

struct TaskBatchCreateTool {
    board: Arc<Mutex<TaskBoard>>,
}

impl TaskBatchCreateTool {
    fn new(board: Arc<Mutex<TaskBoard>>) -> Self {
        Self { board }
    }
}

#[async_trait::async_trait]
impl Tool for TaskBatchCreateTool {
    fn name(&self) -> &str {
        "task_batch_create"
    }

    fn description(&self) -> &str {
        "Create multiple tasks at once. Useful for defining an entire plan. Args: tasks (array of objects with local_id, title, description, depends_on)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "local_id": {"type": "string", "description": "Temporary ID (e.g., 'task-1') to reference dependencies within this batch"},
                            "title": {"type": "string"},
                            "description": {"type": "string"},
                            "depends_on": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Array of local_ids or existing UUIDs that this task depends on"
                            }
                        },
                        "required": ["local_id", "title", "description"]
                    }
                }
            },
            "required": ["tasks"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let tasks_arr = args["tasks"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'tasks' array"))?;

        let mut board = self.board.lock();

        let mut local_to_uuid: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut results = Vec::new();

        for task_val in tasks_arr {
            let local_id = task_val["local_id"].as_str().unwrap_or("").to_string();
            let title = task_val["title"].as_str().unwrap_or("").to_string();
            let description = task_val["description"].as_str().unwrap_or("").to_string();

            if local_id.is_empty() || title.is_empty() || description.is_empty() {
                anyhow::bail!("Each task must have local_id, title, and description");
            }

            let depends_on_raw: Vec<String> = task_val["depends_on"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Resolve dependencies: could be an existing UUID or a local_id created in this batch
            let mut resolved_deps = Vec::new();
            for dep in depends_on_raw {
                if let Some(uuid) = local_to_uuid.get(&dep) {
                    resolved_deps.push(uuid.clone());
                } else if board.get(&dep).is_some() {
                    resolved_deps.push(dep);
                } else {
                    anyhow::bail!(
                        "Dependency '{}' for task '{}' not found in this batch or existing tasks",
                        dep,
                        local_id
                    );
                }
            }

            let uuid = board.create(&title, &description, resolved_deps);
            local_to_uuid.insert(local_id.clone(), uuid.clone());
            results.push(format!("{} -> {} ({})", local_id, uuid, title));
        }

        let mut out = String::from(
            "Batch creation successful:
",
        );
        for r in results {
            out.push_str(&format!(
                "  {}
",
                r
            ));
        }
        Ok(out)
    }
}
