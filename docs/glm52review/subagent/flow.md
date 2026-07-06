I have a complete picture now. Let me synthesize and present my comprehensive findings.

---

# Comprehensive Exploration: Subagent / Sub-Agent Execution System in agent_core

## 1. Project Structure Overview

The codebase is a Rust workspace with three crates:

```
agent_core/
├── core/           ← The main library (all subagent logic lives here)
│   └── src/
│       ├── subagent/mod.rs          ← Subagent struct + execution loop
│       ├── tools/subagent.rs        ← `subagent` + `subagents` tools (the spawn API)
│       ├── tools/skill.rs           ← `skill_list/load/deactivate/reload` tools
│       ├── tools/script.rs         ← `skill.<name>.<script>` tool (executable scripts)
│       ├── skills/                  ← SkillManager + manifest parser
│       ├── tasks/                   ← Task DAG with `task_execute` (spawns subagents)
│       ├── runtime/                 ← Brain, RunManager, Run lifecycle
│       ├── runtime/run/             ← Run lifecycle/turn/context/compact/recovery
│       ├── workflow/                ← Multi-agent DAG workflow executor (PLAN-0009)
│       ├── agent_registry/           ← User-defined agents (AgentDef persistence)
│       └── context.rs               ← 7-segment ContextEngine
├── app/             ← Tauri + React frontend
│   └── src-tauri/src/lib.rs        ← Tauri commands (run_agent_standalone, run_workflow, ...)
├── cli/             ← CLI binary
│   └── src/main.rs                  ← Wires skill/task/subagent tools via Brain callbacks
```

## 2. Complete File Listing With Descriptions

### Subagent execution core

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/subagent/mod.rs` | **The `Subagent` struct + execution loop.** Defines `SubagentConfig`, `SubagentResult`, `ResultStrategy` (Auto/Full/Summary), and `SubagentManager` (a registry of named configs). Contains `Subagent::new`, `new_with_memory`, `run`, and `run_with_sender`. Streams completion tokens to an `EventSender`. Implements per-agent memory injection/persistence (`AgentMemoryStore`). Manages NO process state — uses the `BashTool`'s `default_working_dir` trick to avoid global CWD races between concurrent subagents. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/subagent.rs` | **The `subagent` and `subagents` tools** that the parent agent calls to spawn children. `SubagentSpawnTool` (single) and `SubagentSpawnAllTool` (concurrent batch via `tokio::task::JoinSet`). Includes persona loading (global `~/.agverse/agents/<id>.md` and project-local personas), workspace-root detection, tool-list inheritance/intersection, message persistence to `~/.agverse/subagents/<id>_<ts>.messages.json`, and `build_tool_summary` to produce compact result summaries for the parent. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tasks/mod.rs` | **Task DAG tools** — `task_create`, `task_batch_create`, `task_update`, `task_list`, `task_get`, `task_plan`, `task_ready`, `task_execute`. The `task_execute` tool is a *second* code path that spawns subagents (`TaskExecuteTool`) using `Subagent::new` directly with a heuristic decision (`should_use_subagent`) of inline-vs-subagent based on goal length and tool keywords. Dependency results are auto-injected into the subagent prompt via `build_dependency_context`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tasks/board.rs` | `TaskRecord`, `TaskBoard` (in-memory DAG with statuses pending/in_progress/ready/completed/failed). |

### Skill / script / reference support

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/skills/mod.rs` | **`SkillManager`** — scans multiple search dirs (`~/.agent/skills/`, `~/.agents/skills/`, `~/.claude/skills/`, `~/.agverse/skills/`, `~/.agverse/plugins/**/skills/`, `AGVERSE_BUILTIN_SKILLS`, etc.), parses `SKILL.md` files, tracks active skills, builds the catalog used in the system prompt (Segment 6: "Loaded Skills"), and handles script discovery (`discover_scripts` — manifest-declared + auto-discovered from a `scripts/` directory). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/skills/manifest.rs` | `SkillManifest` parsed from `SKILL.md` YAML frontmatter. Fields: `name`, `description`, `version`, `triggers`, `tags`, `read_when`, `requires` (declared but **not enforced at runtime**), `provides_tools` (declared but **not enforced**), `priority`, `content_path`, `scripts: Vec<ScriptEntry>`. Each `ScriptEntry` has `name`/`description`/`file`/`timeout_secs`/`schema`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/skill.rs` | Four agents-facing tools: `skill_list`, `skill_load`, `skill_deactivate`, `skill_reload`. `skill_load` activates a skill (loads its body + script list into context) by calling `mgr.load_skill_context(name)` and `mgr.activate(name)`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/script.rs` | **`SkillScriptTool`** — dynamically-registered tool named `skill.<skill_name>.<script_name>`. Converts JSON args to CLI flags (`--key value`, `--flag`, nested → JSON string). Runs via `sh -c` either synchronously (legacy) or via the `ProcessSupervisor` (process-group isolation, streaming stdout, kill-on-cancel). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/paths.rs` | `get_skills_dir()` returns `~/.agverse/skills`. `get_agverse_dir()` returns `~/.agverse` root. |

### Lifecycle / runtime infrastructure

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/mod.rs` | Module root + re-exports (`Brain`, `RunManager`, `Run`, `RunCommand`, etc.). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/brain.rs` | **`Brain`** — shared, reusable core. Builds a per-Run `ToolRegistry` via `build_tool_registry(mode)` which: (1) registers default tools, (2) registers memory tools (Standard/Deep mode only), (3) registers todo + skill tools (`register_skill_tools`), (4) applies extra tool callbacks (e.g. MCP), (5) filters by `AgentMode`, (6) **registers subagent tools last** with the *filtered* tool list as the subagent's `available_tools`. The skill_manager is **only constructed at Brain boot**, not per-Run. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/manager.rs` | `RunManager` — creates Runs, manages channels, supports concurrent Runs, worktree isolation (`create_run_in_worktree`), cancellation propagation. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/run/mod.rs` | `Run` struct definition + construction. RAII cleanup in `Drop`: cancels token, aborts `JoinSet`, kills all supervised processes. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/run/lifecycle.rs` | The turn loop (`run_loop`), command polling, pause/resume, approval resolution. Triggers **skill auto-activation** on the user message (`check_triggers` + `@skill:name` tags). Handles `/goal` decomposition. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/run/turn.rs` | Per-turn execution: refresh context segments, model call, streaming, tool dispatch via `ToolOrchestrator`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/run/context.rs` | `refresh_context_segments` — refreshes ENVIRONMENT, TOOL_CATALOG (cached by fingerprint), ACTIVE_MEMORY (project instructions + core memory), **LOADED_SKILLS** (catalog + active content), EXECUTION_PLAN. Calls `sync_skill_scripts()` to register/unregister `skill.<name>.<script>` tools based on currently-active skills. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/run/helpers.rs` | `cancel_and_cleanup` and `cleanup_on_exit` — kill processes, abort tasks, drop pending approvals, cancel steering messages. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/guard.rs` | **`EventGuard`** — RAII guard that emits a terminal `SubagentEnd{success:false}` event on drop if not `.complete()`d. Used by `Subagent::run_with_sender` to prevent orphaned spinners when an early `?` or panic occurs. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/supervisor.rs` | `ProcessSupervisor` — places children in their own process group (`setpgid(0,0)`), kills the group with `killpg(SIGKILL)`, reaps zombies. Owned by `Run`; subagents do NOT share a `Run`'s supervisor — see "Architecture" below. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/runtime/tool_orchestrator.rs` | `ToolOrchestrator` — runs the tool calls of a turn (Sequential/Parallel), with permission gating and approval flow. Subagents instantiate this directly inline (no RunWrapper). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/event.rs` / `event_log.rs` | `RunEvent` / `AgentEvent` variants. Notable subagent events: `SubagentStart`, `SubagentTurnStart`, `SubagentMessageUpdate`, `SubagentToolStart`, `SubagentToolUpdate`, `SubagentToolEnd`, `SubagentApprovalRequired`, `SubagentEnd`. Workflow events: `WorkflowStarted`, `WorkflowNodeStarted`, `WorkflowNodeEnded`, `WorkflowCompleted`. |

### Agent & workflow orchestration (PLAN-0009)

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/agent_registry/mod.rs` | `AgentRegistry` coordinator; CRUD + builder helpers. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/agent_registry/definition.rs` | `AgentDef` (SQLite-persisted: name, system_prompt, model, skills, tools, permission_mode, permission_rules, max_iterations, max_context_tokens, memory_enabled, memory_group). Includes `build_subagent_config(def)` — converts AgentDef → SubagentConfig (all 9 PLAN-0009 fields). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/agent_registry/memory.rs` | `AgentMemoryStore` — per-agent isolated memory with optional embedding-based recall. Used by workflow nodes and `run_agent_standalone`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/agent_registry/history.rs` | Per-agent execution history (input, output, iterations, success, model, latency). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/agent_registry/skill_drafts.rs` | **Experimental** heuristic skill-draft generator from agent history. Drafts are written to a `drafts/` directory and require human approval via `approve_draft` before being promoted to live skills. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/mod.rs` | Module root. Re-exports `WorkflowDef`, `WorkflowExecutor`, `plan`, `validate`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/definition.rs` | `WorkflowDef`, `NodeDef { node_type, agent_id, config }`, `EdgeDef`, `NodeType` (Input/Output/Agent/Transform/HumanApproval), `TrustMode` (Inherit/Trusted/Readonly), `OnNodeFailure` (Abort/Continue/Skip). SQLite persistence. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/executor.rs` | **`WorkflowExecutor`** — DAG toposort → stage-by-stage parallel execution (semaphore-bounded). `execute_agent_node` fetches an `AgentDef`, calls `build_subagent_config`, **injects skill content into the system prompt by name via `inject_skill_content(brain, &def.skills, ...)`**, optionally inherits all Brain tools (Build mode) or uses named subset, and constructs `Subagent::new_with_memory`. Records agent history after execution. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/planner.rs` | Topological DAG planner (returns parallel `Stage`s). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/validate.rs` | Cycle detection, orphan-node detection, missing-config checks, agent_id presence. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/context.rs` | `WorkflowContext` — structured JSON state between nodes (not free-form messages). `RouterConfig` for conditional routing. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/workflow/trust.rs` | `TrustMode::build_permission_config` — modifies per-agent permission config based on workflow trust. |

### Session / persistence / context

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/session.rs` | `SessionManager` — saves full subagent conversations via `save_subagent_with_messages` and the legacy 2-message summary form `save_subagent`. The `SubagentResultLike` trait abstracts over `SpawnResult` (in `tools/subagent.rs`) and `SubagentResult` (in `subagent/mod.rs`). `resume(session_id)` reloads messages for session resumption. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/context.rs` | `ContextEngine` — the 7-segment system-prompt engine. Segments: IDENTITY, PRINCIPLES, ENVIRONMENT, TOOL_CATALOG, ACTIVE_MEMORY, LOADED_SKILLS, EXECUTION_PLAN. Includes `trim_to_fit` (5-stage compression) and `stable_prefix_fingerprint` for cache-stability tracking. |

### Tools (factory + per-tool implementations)

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/mod.rs` | `Tool` trait, `ToolRegistry`, factory `build_tool_by_name`, and `register_memory_tools`. Default registry: read_file, write_file, edit, grep, glob, bash, webfetch, (optional: tavily_search). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/bash.rs` | `BashTool` with `with_supervisor(sup, working_dir)` (Run path) and `with_default_working_dir(wd)` (subagent path — sets `default_working_dir` on the tool so process CWD stays untouched and concurrent subagents don't race). |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/read_file.rs`, `edit.rs`, `write_file.rs`, `glob.rs`, `grep.rs`, `webfetch.rs`, `tavily_search.rs`, `todo.rs`, `core_memory.rs`, `archival_memory.rs`, `recall_memory.rs` | Standard implementations. All inherit from `build_tool_by_name` factory. |

### Prompt / mode

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/prompt.rs` | `DEFAULT_IDENTITY`, `DEFAULT_PRINCIPLES` (includes the "Subagent decision rules" section!), legacy `PromptBuilder`/`PromptAssembler`, and `memory_mode_prompt`. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/mode.rs` | `AgentMode` (Ask/Plan/Build) — controls tool filtering. Ask = read-only, Plan = read+plan, Build = all. `tools_to_remove()` returns the list to filter out per mode. |

### Frontend wiring (Tauri commands)

| File | Purpose |
|---|---|
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/app/src-tauri/src/lib.rs` | Tauri commands: `run_agent_standalone` (runs an AgentDef as a one-shot subagent), `run_workflow` (calls `WorkflowExecutor::execute`), agent CRUD, workflow CRUD+save/validate, agent history/memory commands. |
| `/Users/zniverse/Documents/projects/rust-projects/agent_core/cli/src/main.rs` | CLI wires skills+tasks via `brain.register_tool_fn` callbacks (so they flow into every Run's `ToolRegistry`), `/skill` slash-command handlers. |

## 3. Key Code Excerpts — How the Subagent System Works

### 3a. The `subagent` tool's spawn path

The user-facing tool the parent LLM calls:

```rust
// /Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/tools/subagent.rs:67-111
impl Tool for SubagentSpawnTool {
    fn name(&self) -> &str { "subagent" }
    fn description(&self) -> &str {
        "Spawn a sub-agent with isolated context for a specific task. \
         Use for: multi-step research, tasks needing clean context, parallel work. \
         Do NOT use for: simple reads, single commands, quick searches — handle those yourself. \
         Args: id (string), task (string), system_prompt (optional), \
         tools (optional array of tool names, default: all parent tools), \
         max_iterations (optional, default: parent agent's max_iterations)"
    }
    // ... required: ["id", "task"], optional: system_prompt, tools, max_iterations, result_strategy
}
```

The default subagent prompt is hardcoded in `spawn_single` (`tools/subagent.rs:731-734`):
```rust
let default_system_prompt = "You are a focused sub-agent. Complete the given task and return the result. Be concise. \
You have access to tools: read_file, glob, grep, bash, edit, webfetch, and git tools. \
CRITICAL: ALWAYS use the 'webfetch' tool to fetch web content. NEVER use bash with 'curl' or 'wget'. \
Do NOT attempt to read or process image files.";
```

The subagent's persona is augmented by reading an optional `~/.agverse/agents/<id>.md` (global) and `{cwd}/.agverse/agents/<id>.md` (project-local) persona file:

```rust
// tools/subagent.rs:749-770
let global_agent = std::path::Path::new(&home).join(format!(".agverse/agents/{}.md", id));
if let Ok(c) = tokio::fs::read_to_string(&global_agent).await {
    persona_content.push_str(&format!("Global Persona ({id}):\n{c}\n\n"));
}
let local_agent = cwd.join(format!(".agverse/agents/{}.md", id));
if let Ok(c) = tokio::fs::read_to_string(&local_agent).await {
    persona_content.push_str(&format!("Project Persona ({id}):\n{c}\n\n"));
}
let system_prompt = if persona_content.is_empty() {
    base_prompt
} else {
    format!("{}\n\n=== Subagent Persona ===\n{}", base_prompt, persona_content)
};
let ws_root = workspace_root.to_string_lossy().to_string();
let system_prompt = format!("{system_prompt}\n\nWorking Directory: {ws_root}");
```

### 3b. Subagent tool-list inheritance — subagents are restricted to a subset of parent tools

```rust
// tools/subagent.rs:799-828
let is_all = args["tools"].as_str().map(|s| s == "all")
    .or_else(|| args["tools"].as_array().and_then(|a| a.first())
        .and_then(|v| v.as_str()).map(|s| s == "all"))
    .unwrap_or(false);

let mut final_tool_names = if is_all {
    available_tools.to_vec()              // "all" wildcard
} else if tool_names.is_empty() {
    vec!["read_file".to_string()]         // empty → at least read_file
} else {
    tool_names
};
// CRITICAL: subagents NEVER get tools the parent doesn't have
final_tool_names.retain(|t| available_tools.contains(t));
if final_tool_names.is_empty() {
    final_tool_names = vec!["read_file".to_string()];
}
let tool_registry = ToolRegistry::from_names(&final_tool_names);
```

The default tool set for a subagent (when `tools` arg is absent) is **read-heavy**: `["read_file", "glob", "grep", "bash", "edit", "webfetch"]` — note that **no skill tools, no script tools, no memory tools, no subagent tools** are given to subagents by default. They are NOT recursive.

### 3c. Concurrent subagent execution — JoinSet for cancel-propagation

```rust
// tools/subagent.rs:269-330
let mut join_set = tokio::task::JoinSet::new();
for (id, task, tools, max_iterations, strategy) in task_infos {
    let model_config = self.model_config.clone();
    // ... clone all per-task inputs
    join_set.spawn(async move {
        let args = serde_json::json!({ "id": id.clone(), "task": task, /* ... */ });
        let result = spawn_single(&args, &model_config, &available_tools, sub_sender,
                                   &permission_config, parent_max_iterations, strategy).await;
        // ... persist messages to disk
        (id, result, strategy, file_ref)
    });
}
// Collect results. If the parent is cancelled, the JoinSet is dropped,
// which aborts all child tasks.
let mut results = Vec::new();
while let Some(res) = join_set.join_next().await { results.push(res); }
```

### 3d. Subagent execution loop

```rust
// subagent/mod.rs:193-260 (abridged)
pub async fn run_with_sender(&mut self, task: &str, event_sender: Option<EventSender>) -> Result<SubagentResult> {
    // PLAN-0009: inject relevant memories before the task is added.
    self.inject_memory(task);

    // Inject strategy-specific instructions (Summary, Full, Auto) as system messages
    match self.config.result_strategy {
        ResultStrategy::Summary => { self.context.add(Message::system("CRITICAL: ... ONLY a concise summary ...")); }
        ResultStrategy::Full    => { self.context.add(Message::system("CRITICAL: Output ALL findings and data verbatim ...")); }
        ResultStrategy::Auto    => { /* default */ }
    }

    self.context.add(Message::user(task));
    let _ = event_sender.send(AgentEvent::SubagentStart { subagent_id, role_name, task });
    let mut guard = EventGuard::new(move || { /* emit SubagentEnd{success:false} on drop */ });

    for iteration in 0..self.config.max_iterations {
        let _ = event_sender.send(AgentEvent::SubagentTurnStart { subagent_id, turn_index });
        self.context.trim_to_fit();
        let messages = self.context.messages();
        let tools = self.registry.tool_definitions();
        let stream = self.client.chat_completion_stream(&messages, &tools).await?;
        let (text, tool_calls) = self.collect_stream(stream, event_sender.as_ref()).await?;
        if tool_calls.is_empty() {
            guard.complete();
            // emit SubagentEnd{success:true}
            self.persist_memory(task, &all_text);
            return Ok(SubagentResult { subagent_id, role_name, output: all_text, last_text, iterations_used, success });
        }
        // Execute tools via a contained ToolOrchestrator (remapping AgentEvent → Subagent* variants)
        let results = { let mut orchestrator = ToolOrchestrator { registry: &self.registry, /* ... */ }; orchestrator.execute_tools(&tool_calls, /* event mapper */).await };
    }
    guard.complete();
    // emit SubagentEnd{success:false} (max iterations reached)
    // ...
}
```

### 3e. Result formatting — `Auto`/`Full`/`Summary`

```rust
// tools/subagent.rs:618-682
impl SpawnResult {
    fn format_output(&self, strategy: ResultStrategy) -> String {
        match strategy {
            ResultStrategy::Full => { /* return last-turn text + tool_summary */ }
            ResultStrategy::Summary => { /* return ONLY last-turn text */ }
            ResultStrategy::Auto => self.summary(),  // all_text + tool_summary
        }
    }
}
```

### 3f. Tools available to subagents: NO skill tools, NO script tools, NO memory tools by default

The `ToolRegistry::from_names` factory only knows these tool names:

```rust
// core/src/tools/mod.rs:282-296
pub fn build_tool_by_name(name: &str) -> Option<Box<dyn Tool>> {
    match name {
        "read_file" => Some(Box::new(read_file::ReadFileTool)),
        "write_file" => Some(Box::new(write_file::WriteFileTool)),
        "edit" => Some(Box::new(edit::EditTool)),
        "grep" => Some(Box::new(grep::GrepTool)),
        "glob" => Some(Box::new(glob::GlobTool)),
        "bash" => Some(Box::new(bash::BashTool::new())),
        "webfetch" => Some(Box::new(webfetch::WebFetchTool::new())),
        "tavily_search" => tavily_search::TavilySearchTool::from_env().map(Box::new),
        _ => None,  // ← unknown names (skill.*, mcp_*, subagent, etc.) are silently skipped
    }
}
```

So **subagents spawned via the `subagent` or `subagents` tool CANNOT invoke skills, scripts, MCP tools, or memory tools** — even if the parent has them. The `available_tools.contains(t)` filter in `spawn_single` would also exclude `"skill_list"`, `"subagent"` etc. (they're not in the parent's tool list passed to subagent tools — see `Brain::build_tool_registry`).

The one exception is the **PLAN-0009 path**: when a workflow's `execute_agent_node` constructs a subagent, it can either use `ToolRegistry::from_names(&def.tools)` or **inherit the entire Build-mode `ToolRegistry`** (which includes all skills/scripts/MCP tools):

```rust
// workflow/executor.rs:433-437
let registry = if def.tools.is_empty() {
    brain.build_tool_registry(AgentMode::Build)
} else {
    ToolRegistry::from_names(&def.tools)
};
```

And `run_agent_standalone` (Tauri command) does the same:

```rust
// app/src-tauri/src/lib.rs:1297-1302
let registry = if def.tools.is_empty() {
    brain.build_tool_registry(agent_core::AgentMode::Build)
} else {
    agent_core::ToolRegistry::from_names(&def.tools)
};
```

### 3g. Concurrent-execution safety — no global CWD mutation

```rust
// subagent/mod.rs:152-167
fn wire_working_dir(mut registry: ToolRegistry, config: &SubagentConfig) -> ToolRegistry {
    if let Some(ref wd) = config.working_dir {
        if registry.has("bash") {
            registry.register(Box::new(
                crate::tools::bash::BashTool::with_default_working_dir(
                    Some(wd.to_string_lossy().to_string()),
                ),
            ));
        }
    }
    registry
}
```

The comment in `run_with_sender` is explicit:
> "NOTE: We intentionally do NOT touch the process-global CWD here. Modifying std::env::set_current_dir() races with concurrent subagents sharing the same tokio runtime. The subagent's working directory is instead plumbed into the BashTool via `default_working_dir`."

## 4. How Skills Are Loaded and Made Available (Or Not) To Subagents

### Two distinct skill flows

**A. The "catalog" flow (for normal Runs in the parent agent):**
- At boot, `Brain::build_skill_manager` calls `SkillManager::with_defaults()` which collects search dirs from: `.agent/skills`, `.claude/skills`, `skills/` (cwd), `~/.agent/skills`, `~/.agents/skills`, `~/.claude/skills`, `~/.agverse/skills`, `~/.agverse/plugins/**/skills/`, then `AGVERSE_BUILTIN_SKILLS` / `AGVERSE_APP_RESOURCES/builtin-skills`.
- `mgr.scan()` reads every `SKILL.md` in those dirs and parses it into a `SkillManifest`.
- Per-turn, `Run::refresh_context_segments` calls `sync_skill_scripts` which **dynamically registers `skill.<name>.<script>` tools** for every active skill, and writes the catalog + active content into Segment 6 (`set_loaded_skills`).
- The Run's `ToolRegistry` gets `skill_list`, `skill_load`, `skill_deactivate`, `skill_reload` registered once at boot via `register_skill_tools`.
- Auto-trigger: in `run/lifecycle.rs`, the user message is checked against `mgr.check_triggers()` and any `@skill:name` tags — matched skills are `activate`d before the loop starts.

**B. For subagents spawned by `subagent`/`subagents`/`task_execute`:**
- **No `SkillManager` reference is given to the subagent.**
- The `SubagentConfig` has a `skills: Vec<String>` field (PLAN-0009), but the `spawn_single` function in `tools/subagent.rs` does NOT pass anything into `config.skills`:

```rust
// tools/subagent.rs:833-843
let config = SubagentConfig {
    system_prompt,
    tools: final_tool_names,
    max_iterations,
    max_context_tokens: model_config.max_context_tokens,
    result_strategy,
    working_dir: Some(workspace_root.clone()),
    ..SubagentConfig::default()   // ← skills is left as default (empty Vec)
};
```

- The subagent's `ToolRegistry` is built via `ToolRegistry::from_names(&final_tool_names)`, which can only construct the **8 builtin tools**. The `skill.*` tools are not constructable from a name string.
- Result: **subagents spawned via the conversational `subagent`/`subagents` tools have NO access to skills**. They only have the literal builtin 8 tools (read/write/edit/grep/glob/bash/webfetch/tavily).

**C. For workflow nodes and `run_agent_standalone` (PLAN-0009 path):**
- The `AgentDef.skills` list is honored — but **via system-prompt injection, not via a `SkillManager`**:

```rust
// workflow/executor.rs:530-544 and app/src-tauri/src/lib.rs (injected into system_prompt)
fn inject_skill_content(brain: &Brain, skills: &[String], system_prompt: &str) -> String {
    let mut prompt = system_prompt.to_string();
    if let Some(ref sm) = brain.skill_manager {
        let mgr = sm.lock();
        for name in skills {
            if let Ok(Some(content)) = mgr.load_skill_context(name) {
                if !prompt.is_empty() { prompt.push_str("\n\n"); }
                prompt.push_str(&content);
            }
        }
    }
    prompt
}
```

So workflow-agent skills are baked into the **system_prompt string** (the SKILL.md body + script list) rather than being dynamically loadable. **The script tools are NOT registered** unless the subagent inherits the entire Brain registry (`def.tools.is_empty()` path) — and even then, only active-skills' scripts that match the Brain's currently-active set are registered (because `sync_skill_scripts` runs on the parent Run, not the subagent).

## 5. How Scripts and References Work

### Script execution model

A skill manifest can declare scripts:

```yaml
# in SKILL.md frontmatter
scripts:
  - name: deploy
    description: "Deploy the app"
    file: scripts/deploy.sh
    timeout_secs: 120
    schema: '{"type": "object", "properties": {"env": {"type": "string"}}, "required": ["env"]}'
```

Or scripts can be auto-discovered in `<skill_dir>/scripts/` for extensions `.sh, .bash, .py, .js, .rb, .ts, .go, .rs` (`SkillManager::discover_scripts`). Manifest entries win on name collision.

Each script becomes a `SkillScriptTool` named **`skill.<skill_name>.<script_name>`** — registered into the parent Run's `ToolRegistry` by `sync_skill_scripts` (only when the skill is active). Registered tools are diff'd against the previously-registered set so deactivated skills' tools are unregistered.

### Script execution (`SkillScriptTool::execute_with_stream`)

```rust
// tools/script.rs:180-208
let cli_args = Self::args_to_cli("", &args);    // JSON Value → CLI flags
let script_arg = self.script_path.to_str()?;
let command = std::iter::once(script_arg.to_string()).chain(cli_args).collect::<Vec<_>>().join(" ");
let working_dir = self.skill_dir.to_str()?;
if let Some(ref sup) = self.supervisor {
    run_supervised(sup, &command, working_dir, self.timeout_secs, on_update).await
} else {
    self.run_sync(&command, working_dir, self.timeout_secs).await
}
```

The script is run with `sh -c <script_path> <args>` from the skill's directory as working_dir. The supervised path hooks into `ProcessSupervisor` (process-group kill semantics identical to `BashTool`).

### "Reference" files

There is **no first-class "reference" concept** in this codebase. The closest analogues are:

1. **`SKILL.md` content body** — the prose document that gets injected into Segment 6 when a skill is activated. `SkillManifest::read_body(path)` strips the YAML frontmatter and returns the markdown body.
2. **`content_path`** field on `SkillManifest` — can point to a separate file instead of the SKILL.md body.
3. **`agverse.md`** project instructions (loaded per-turn into Segment 5 "ACTIVE_MEMORY") — global (`~/.agverse/agverse.md`), project (`./agverse.md` or `./AGENTS.md`), local (`./agverse.local.md`), and modular rules (`./.agverse/rules/*.md`).
4. **Per-agent personas** for subagents — `~/.agverse/agents/<id>.md` and `{cwd}/.agverse/agents/<id>.md`, loaded into the subagent system prompt by `spawn_single`.

No "scripts as references", no "reference library" — those terms don't appear in the code.

## 6. Skill-to-Skill Composition Patterns

**There is no `skill` tool that lets one skill invoke another skill.** Searching the entire core for `"skill"` as a tool name or any "skill_call"/"invoke_skill" pattern returns nothing.

The four skill-facing tools are:

| Tool | Behavior |
|---|---|
| `skill_list` | Lists all available skills with names, descriptions, triggers, ACTIVE markers |
| `skill_load` | Loads a single skill by name into Segment 6 (active set) + script tools registered |
| `skill_deactivate` | Removes a skill (or `"all"`) from active set + script tools unregistered |
| `skill_reload` | Re-scans directories for new/changed SKILL.md files; preserves active set |

A skill's `requires: Vec<String>` and `provides_tools: Vec<String>` fields are **declared in the manifest format and parsed successfully** but **never read by any runtime code** — `grep` for `.requires` and `.provides_tools` in `core/src/skills` finds zero usages.

### What composition patterns DO exist?

1. **The model composes skills at runtime via the `skill_load` tool.** Each turn, the LLM sees the catalog (Segment 6) and can call `skill_load("X")` to activate skill X. The next turn, `sync_skill_scripts` registers X's script tools. The LLM can then call `skill.X.<script>` and also `skill_load("Y")` to add another. There is no automatic cascade — composition is **manual and LLM-driven**.

2. **PLAN-0009 workflow executor composes agents (which carry skills) at the DAG level.** A workflow node references an `agent_id`, and that agent's `AgentDef.skills` list is honored via system-prompt injection. So in effect, **skills compose other skills implicitly through the workflow graph** — but only because each agent's skills are baked into its own system prompt; they don't call each other.

3. **The reflector + skill_drafts subsystem (experimental).** `agent_registry::skill_drafts::generate_drafts` analyzes an agent's history and writes draft `SKILL.md` files requiring human approval via `approve_draft`. This is a tool for skill-evolution, not for skill composition.

### The "daily-alpha-pipeline" pattern

There is **no skill called `daily-alpha-pipeline` or similar in this codebase**. The codebase does not ship any SKILL.md files in the project tree (only `app/node_modules/@reduxjs/toolkit/skills/...` which are Redux's skills for *their* tooling, unrelated to agent_core).

The closest thing to a "pipeline" pattern is the **workflow DAG** (PLAN-0009). You could compose a workflow where:
- Node 1 (Agent "research") → produces JSON
- Node 2 (Agent "summarize") consumes Node 1's output → produces summary
- Node 3 (Agent "publish") consumes Node 2's output → publishes

Each agent node is a `Subagent` execution. The "pipeline" is the DAG edges, not skill invocation. This is **explicit in the PLAN-0009 doc**:

> 本方案是**可视化 DAG 工作流编排器**（ closer to Dify/n8n workflow + agent nodes），不是 Google ADK / AutoGen 式的动态多 agent 编排系统。

## 7. Architecture / Data Flow

### 7a. Single Run lifecycle (the "main agent" path)

```
User input
  ↓
RunManager::create_run_with_workdir()
  ↓
Run::new(brain, model, working_dir, history, mode)
  │  - brain.build_tool_registry(mode) ← registers subagent/skill/task tools at the END
  │  - Context::new(identity, max_ctx_tokens)
  │  - 7-segment init
  ↓
Run::run(user_input)
  ↓
[wait for Start command]
  ↓
[skill auto-trigger — check_triggers + @skill:name]
[/goal decomposition if /goal prefix]
  ↓
run_loop() — for turn in 0..max_iter:
  │  1. poll_commands() (cancel/pause/steer/approve)
  │  2. wait_for_resume() if Paused
  │  3. run_turn(turn_index):
  │     a. refresh_context_segments()
  │        - sync_skill_scripts() — register/unregister skill.X.Y tools
  │        - rebuild tool catalog (cached by fingerprint)
  │        - re-read project instructions into Active Memory
  │        - rebuild Loaded Skills (catalog + active content)
  │     b. trim_to_fit() — 5-stage compression if needed
  │     c. model_turn() — chat_completion_stream
  │     d. collect_stream → (text, tool_calls)
  │     e. if no tool_calls → final answer, exit loop
  │     f. else ToolOrchestrator::execute_tools(tool_calls)
  │        - per-tool permission check
  │        - approval flow if needed (uses Run.approval_resolver)
  │        - on_update streaming callback (for bash/script stdout streaming)
  │        - if tool is `subagent` or `subagents` → spawn subagent(s)
  ↓
[completion: write_session_memory, emit RunCompleted]
[cleanup: kill_all_processes, abort_all_tasks, drop approvals]
[offline reflection if Brain.reflector configured]
[DiffObserver may auto-apply skill updates]
```

### 7b. Subagent spawn lifecycle (when LLM calls `subagent` or `subagents` tool)

```
Parent's ToolOrchestrator dispatches tool_call{id, function:{name:"subagent", arguments:...}}
  ↓
SubagentSpawnTool::execute_with_stream(args, on_update, event_sender)
  ↓
spawn_single(args, model_config, available_tools, event_sender, ...) — async
  │
  ├── Find workspace root (walk up for Cargo.toml)
  ├── Load global persona ~/.agverse/agents/<id>.md (if exists)
  ├── Load project persona {cwd}/.agverse/agents/<id>.md (if exists)
  ├── Compose system_prompt = base + persona + "Working Directory: <ws_root>"
  ├── Resolve tool names:
  │     - "all" wildcard → available_tools (parent's filtered tool list)
  │     - default fallback → [read_file, glob, grep, bash, edit, webfetch]
  │     - explicit array → retain only those that exist in available_tools
  │     - if empty → [read_file] (subagent needs at least one tool)
  ├── ToolRegistry::from_names(&final_tool_names)  ← ONLY 8 builtin tools possible
  ├── SubagentConfig { system_prompt, tools, max_iter, max_ctx, working_dir: Some(ws_root), .. }
  ↓
Subagent::new(id, config, model_config, registry, permission_config)
  │  - new OpenAIClient (own model_config clone)
  │  - new ContextEngine (system_prompt, max_context_tokens)
  │  - new PermissionPolicy (inheriting parent config, fresh runtime whitelist)
  │  - new HookRegistry (fresh — subagent does NOT inherit parent's hooks)
  │  - BashTool replaced with default_working_dir=ws_root version
  ↓
Subagent::run_with_sender(task, event_sender)
  │
  ├── inject_memory(task) — if AgentMemoryStore + agent_id set (PLAN-0009 only)
  ├── inject ResultStrategy system message (Summary/Full)
  ├── context.add(Message::user(task))
  ├── emit SubagentStart
  ├── EventGuard (drops emit SubagentEnd{success:false} on panic/early-?)
  ↓
  for iteration in 0..max_iter:
  │   emit SubagentTurnStart
  │   context.trim_to_fit()
  │   chat_completion_stream → collect_stream → (text, tool_calls)
  │   ── stream emits SubagentMessageUpdate{Text|Thinking} events
  │   if tool_calls.is_empty():
  │     guard.complete() + emit SubagentEnd{success:true}
  │     persist_memory(task, output)
  │     return SubagentResult{output, last_text, success}
  │   else:
  │     ToolOrchestrator::execute_tools (with custom event mapper AgentEvent→Subagent* variants)
  │     emit SubagentToolEnd for each result
  │     append tool results to context
  ↓
[on max-iter reached: emit SubagentEnd{success:false}, return "Subagent Suspended" formatted result]
  ↓
Back in parent tool:
  ├── persist messages to ~/.agverse/subagents/<id>_<ts>.messages.json
  ├── session_mgr.save_subagent_with_messages(...)
  └── return format_output(strategy) → parent LLM sees this as the tool_result
```

### 7c. Concurrent subagent batch (`subagents` tool)

```
SubagentSpawnAllTool::execute_with_stream(tasks=[...])
  ↓
Parse each task → (id, task, tools, max_iter, strategy)
  ↓
Emit SubagentStart for ALL tasks up-front (TUI shows all boxes immediately)
  ↓
Spawn JoinSet:
  for each (id, task, ...):
    join_set.spawn(async move {
        let result = spawn_single(...).await;
        persist_subagent_messages(...);
        session_mgr.save_subagent_with_messages(...);
        (id, result, strategy, file_ref)
    });
  ↓
Loop: while let Some(res) = join_set.join_next().await { results.push(res); }
  ────> If parent is cancelled, the JoinSet is dropped, which ABORTS all child tasks
  ↓
Format combined output: "=== Sub-agent Batch Results (N tasks) ===\n[1] <id> — success\n...\n=== End batch results ==="
```

### 7d. Workflow execution (multi-agent DAG)

```
run_workflow(workflow_id, input, session_id, ...)
  ↓
WorkflowExecutor::execute(workflow, input, session_id, cancel_token, event_tx)
  ↓
plan = planner::plan(workflow.nodes, workflow.edges)  ← toposort into parallel Stages
create_run(workflow.id, session_id, input)             ← record run start
emit WorkflowStarted
  ↓
for stage in plan.stages:
  if cancel_token.is_cancelled() → abort
  executable = stage.nodes - skipped
  spawn each node as a parallel task (semaphore=max_concurrent)
  await each node:
     execute_node(node, input, ...)
       match node.node_type:
         Input      → forward workflow input
         Output     → forward resolved input as workflow output
         Transform  → field extraction
         HumanApproval → auto-approve in V1
         Agent:
           execute_agent_node:
             fetch AgentDef from registry (SQLite)
             build_subagent_config(def) → SubagentConfig
             inject_skill_content(brain, def.skills, system_prompt)
             build_model_config(def, brain.config)
             workflow.trust_mode.build_permission_config(...)
             registry = if def.tools.empty() → brain.build_tool_registry(Build)
                        else → ToolRegistry::from_names(def.tools)
             memory = if def.memory_enabled > 0 → Some(build_agent_memory_store(brain, storage))
             subagent = Subagent::new_with_memory(...)
             subagent.run_with_sender(agent_input, event_tx)
             record AgentHistoryEntry (agent_id, session, trigger="workflow", input, output, success, model, latency)
             return JSON {result, success, iterations}
     apply_router(node, output, workflow, skipped)  ← skip downstream nodes not in route targets
     record WorkflowRunNodeResult (input, output, tokens, latency, status)
     emit WorkflowNodeEnded
  ↓
Collect output node value → WorkflowRunResult
finish_run(storage, run_id, status, output, error, tokens_in, tokens_out)
emit WorkflowCompleted
```

## 8. Summary of Key Findings

### Subagent execution system
- **`Subagent` struct** in `subagent/mod.rs` is the runtime execution primitive. It owns its own `OpenAIClient`, `ContextEngine`, `ToolRegistry`, `PermissionPolicy`, `HookRegistry`, and optional `AgentMemoryStore`.
- Subagents spawned via the conversational `subagent`/`subagents` tool **always get a fresh context** (the only "message history" they see is what's prepended into `task` string by the parent — e.g. `task_execute` injects dependency results).
- **No `SubAgent` types like "general"/"explore"** — there is only one `Subagent` type. Differentiation is purely via the `SubagentConfig { system_prompt, tools, model, ... }` passed to `Subagent::new`.
- **Two spawn paths**: (1) ad-hoc via `subagent`/`subagents` tools with user-supplied `id` and `task`, (2) planned DAG via `task_execute`/TaskBoard with auto-resolved dependency context (via `should_use_subagent` heuristic).
- **Result formatting** is governed by `ResultStrategy` enum: `Auto` (all_text + tool_summary), `Full` (last-turn verbatim + tool_summary), `Summary` (only last-turn text after a Summary-instruction system message was injected).

### Subagent lifecycle
- **Spawned** by `Subagent::new`/`new_with_memory` — gets a UUID `id`.
- **Executed** by `run_with_sender` — full turn loop with streaming + tool execution via embedded `ToolOrchestrator`.
- **Concurrent execution** uses `tokio::task::JoinSet` in the `subagents` tool. Dropping the `JoinSet` (e.g. parent cancellation) aborts all child tasks.
- **Max-iterations suspension** is NOT a proper resume mechanism: the subagent returns a "Suspended(max iterations reached)" formatted string and the parent is told to "spawn a new subagent referencing this progress". The full messages are persisted to **`~/.agverse/subagents/<id>_<ts>.messages.json`** for potential out-of-band reference, and the parent's `SessionManager.save_subagent_with_messages` records a session record of type `"subagent"`.
- **Cleanup** has three layers: (1) explicit SubagentEnd events with `success: false` on max-iter, (2) `EventGuard` RAII that fires SubagentEnd{success:false} on panic/early `?`, (3) `ProcessSupervisor` kill_all is implicit (subagent's bash tool uses its own runtime; there is no supervisor on subagents today — they use the legacy `run_sync` path). For workflows, the parent Run's `cancel_and_cleanup` aborts the workflow's `JoinSet` of per-node tasks.

### Task_id resumption
- `task_id` is just a user-supplied identifier string in the `subagent`/`subagents` tool args — it's used as the persona filename lookup, message file naming, and session record key. There is **no first-class resumption**: you can't call `subagent` again with the same `id` to resume from where the prior one stopped — each call is a fresh subagent. Session persistence (`~/.agverse/subagents/<id>_<ts>.messages.json`) preserves history for future use.

### Scripts and references
- Skills can declare `scripts:` in `SKILL.md` frontmatter (or auto-discovered from `<skill>/scripts/`). Each script becomes a dynamically-registered `skill.<name>.<script>` tool that runs `sh -c <script_path> <args>`.
- The script tool uses `ProcessSupervisor` (when attached) for process-group kill semantics.
- Scripts can run from a skill's own directory as the working dir.
- "References" as a concept does not exist. The closest is the SKILL.md content body that gets injected into the system prompt's Segment 6 when a skill is activated.

### Skill-to-skill invocation
- **No `skill` tool** — there is no first-class tool for one skill to invoke another.
- The `SkillManifest.requires` and `provides_tools` fields are parsed but **dead** in runtime — no code reads them.
- Composition is **LLM-mediated**: the LLM sees the skill catalog (Segment 6) and can call `skill_load(X)` to activate X. Composition is manual.
- Skills can compose other skills indirectly via the **PLAN-0009 workflow DAG**: each agent's `AgentDef.skills` are baked into its system prompt via `inject_skill_content(brain, &def.skills, system_prompt)`.

### Context and tool availability per agent type

| Path | Skill tools | Script tools | Memory tools | Subagent tools | Persona file | Full Builtin Tools |
|---|---|---|---|---|---|---|
| **Normal Run (parent)** | ✓ (`register_skill_tools`) | ✓ dynamically registered per active skill | ✓ (Standard/Deep modes) | ✓ (`register_subagent_tools`) | n/a | ✓ |
| **Subagent via `subagent` tool** | ✗ | ✗ | ✗ (unless `new_with_memory`) | ✗ (no recursion) | ✓ via `~/.agverse/agents/<id>.md` | ✓ (default 6: read_file/glob/grep/bash/edit/webfetch) |
| **Subagent via `task_execute`** | ✗ | ✗ | ✗ | ✗ | ✗ (uses default sub-agent prompt) | ✓ (default 5: read_file/glob/grep/bash/edit) |
| **Workflow Agent node** (`def.tools.empty`) | ✓ inherits Brain's Build registry | ✓ (only skills active at Brain boot) | ✗ (no memory tools in Brain's default registry) | ✓ (subagent tools were registered to Brain's registry) | via injected system_prompt | ✓ Build mode's full set |
| **Workflow Agent node** (`def.tools` non-empty) | ✗ (only builtin-8 names can construct) | ✗ | ✗ | ✗ | via injected system_prompt | only the named subset of `build_tool_by_name` matches |
| **`run_agent_standalone` (Tauri)** | Same as workflow agent node | | | | | |

### System-prompt construction for subagents

The subagent system prompt is **composed inline in `tools/subagent.rs::spawn_single`**, not via the 7-segment `ContextEngine`:
1. Base prompt = `args["system_prompt"]` or the hardcoded default.
2. Append persona content read from `~/.agverse/agents/<id>.md` and `{cwd}/.agverse/agents/<id>.md` (only if either exists).
3. Append `"Working Directory: <ws_root>"`.
4. **Skills are NOT injected** in this path — the `SubagentConfig.skills` field stays empty (default).
5. The result strategy's instructional message is added as a `system` Message in the context (NOT as part of the system_prompt string) just before the user task.

For workflow / standalone-agent paths, the system prompt additionally has skill bodies injected via `inject_skill_content` (the SKILL.md body + script list appended as text).

### Subagents and skills — the bottom line

- A subagent spawned by the LLM calling the `subagent` tool gets **only the 8 builtin tools** and **no skills** — even if the parent agent has them loaded.
- Skills are only available to (a) the parent agent's Runs and (b) workflow nodes where `AgentDef.tools` is empty (so they inherit the Brain's full Build-mode registry, which `sync_skill_scripts` had populated based on the Brain's currently-active skill set at the time the parent Run was constructed — note that the subagent inherits a snapshot, not live updates).
- The **PLAN-0009 `AgentDef.skills` field is honored via system_prompt text injection only** — workflow/standalone subagents see the SKILL.md body in their system prompt but cannot dynamically `skill_load` other skills unless they inherit the full Brain registry.
