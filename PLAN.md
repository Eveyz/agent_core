# Agent Core — Development Plan

## Project Overview

A Rust-based LLM Agent system implementing the ReAct pattern, with harness engineering patterns inspired by [learn-claude-code](https://github.com/shareAI-lab/learn-claude-code).

**Current state:** Core agent loop + tools + 4-layer memory system — fully implemented.

**Goal:** Build a complete agent harness with planning, delegation, multi-agent coordination, and extensibility.

## Tech Stack

| Dimension | Choice |
|-----------|--------|
| LLM API | OpenAI compatible (multi-provider, model switching) |
| Streaming | SSE streaming with thinking/reasoning content support |
| Tool framework | Extensible `Tool` trait + JSON Schema validation |
| Token management | tiktoken-rs, context trimming with token limits |
| API retry | Exponential backoff, 429/5xx, max 3 retries |
| Model management | `config.toml` multi-model config + runtime switching |
| Agent mode | ReAct (Thought → Action → Observation) |
| Memory architecture | 4-layer: Message Buffer + Core Memory + Recall + Archival |
| Embedding model | fastembed (bge-small-en-v1.5, 384-dim) |
| Storage | rusqlite (bundled), WAL mode |
| Vector search | Brute-force cosine similarity |
| Output forms | lib crate + CLI binary |

## Project Structure

```
agent_core/
├── Cargo.toml
├── PLAN.md
├── config.toml
├── src/
│   ├── lib.rs                     # Library entry, public API exports
│   ├── main.rs                    # CLI binary entry (REPL)
│   ├── types.rs                   # Core data structures
│   ├── config.rs                  # Config + ModelConfig + MemoryConfig
│   ├── agent.rs                   # Agent core loop + AgentBuilder
│   ├── context.rs                 # Conversation history + token trimming
│   ├── prompt.rs                  # ReAct prompt template
│   ├── client/
│   │   ├── mod.rs                 # OpenAIClient + retry logic
│   │   └── streaming.rs           # SSE parser + ToolCall accumulator
│   ├── tools/
│   │   ├── mod.rs                 # Tool trait + Registry + Schema validation
│   │   ├── read_file.rs           # Built-in: read file
│   │   ├── write_file.rs          # Built-in: write file
│   │   ├── run_command.rs         # Built-in: shell execution
│   │   ├── glob.rs                # Built-in: file pattern matching
│   │   ├── grep.rs                # Built-in: content search
│   │   ├── git.rs                 # Built-in: 5 git operations
│   │   ├── core_memory.rs         # Built-in: memory management tools
│   │   ├── recall_memory.rs       # Built-in: conversation search tools
│   │   └── archival_memory.rs     # Built-in: knowledge management tools
│   └── memory/
│       ├── mod.rs                 # MemoryManager facade
│       ├── block.rs               # Core Memory (L2)
│       ├── recall.rs              # Recall Memory (L3)
│       ├── archival.rs            # Archival Memory (L4)
│       ├── embedding.rs           # fastembed wrapper
│       ├── storage.rs             # SQLite storage layer
│       └── consolidation.rs       # Memory deduplication
```

---

## Completed Phases (Original 12-Phase Plan)

| Phase | Content | Status |
|-------|---------|--------|
| 1 | Data structures + config + non-streaming client | ✅ Done |
| 2 | SSE streaming parsing + tool call accumulation | ✅ Done |
| 3 | Tool trait + Registry + Schema validation | ✅ Done |
| 4 | Agent ReAct loop + retry + parallel tool execution | ✅ Done |
| 5 | Context manager + token trimming | ✅ Done |
| 6 | ReAct prompt + AgentEvent | ✅ Done |
| 7 | Memory: SQLite + embedding | ✅ Done |
| 8 | Memory: Core Memory (L2) | ✅ Done |
| 9 | Memory: Recall Memory (L3) | ✅ Done |
| 10 | Memory: Archival Memory (L4) | ✅ Done |
| 11 | Memory: Consolidation + sleep-time | ✅ Done |
| 12 | lib.rs public API + AgentBuilder + CLI | ✅ Done |

---

## Harness Engineering Roadmap

Reference: [learn-claude-code](https://github.com/shareAI-lab/learn-claude-code) 20-lesson harness pattern curriculum.

### Gap Analysis

| # | Lesson | Status | Notes |
|---|--------|--------|-------|
| s01 | Agent Loop | ✅ Done | ReAct loop in `agent.rs` |
| s02 | Tool Use | ✅ Done | `Tool` trait + `ToolRegistry` + dispatch |
| s03 | Permission System | ✅ Done | `PermissionRule`, `PermissionPolicy`, approval pipeline in `src/permission/` |
| s04 | Hook System | ✅ Done | `HookRegistry`, `PreToolUse`/`PostToolUse` in `src/hooks/` |
| s05 | TodoWrite | ✅ Done | `TodoList`, `TodoItem`, tools in `src/todo/` + `src/tools/todo.rs` |
| s06 | Subagent | ✅ Done | `Subagent`, `SubagentConfig` in `src/subagent/` |
| s07 | Skill Loading | ✅ Done | `SkillManifest`, `SkillLoader` in `src/skills/` + `src/tools/skill.rs` |
| s08 | Context Compaction | ✅ Done | snipCompact + autoCompact + microCompact in `context.rs` |
| s09 | Memory System | ✅ Done | 4-layer memory with SQLite + embeddings |
| s10 | System Prompt | ✅ Done | `PromptAssembler` with section-based assembly in `prompt.rs` |
| s11 | Error Recovery | ✅ Done | `RecoveryEngine` with retry, token escalation, fallback model, compact in `src/error_recovery/` |
| s12 | Task System | ✅ Done | `TaskBoard`, `TaskRecord`, dependency graph in `src/tasks/` |
| s13 | Background Tasks | ✅ Done | `BackgroundPool`, `Notification` in `src/background/` |
| s14 | Cron Scheduler | ✅ Done | `CronScheduler`, `CronJob` in `src/cron/` |
| s15 | Agent Teams | ✅ Done | `AgentTeam`, `MessageBus` in `src/teams/` |
| s16 | Team Protocols | ✅ Done | `TeamMessage`, request/reply format in `src/teams/` |
| s17 | Autonomous Agents | ✅ Done | Foundation via task board + team bus (self-claim ready) |
| s18 | Worktree Isolation | ✅ Done | `WorktreeManager`, git worktree creation/removal in `src/worktree/` |
| s19 | MCP Plugin | ✅ Done | `McpClient`, `McpChannel` in `src/mcp/` |
| s20 | Comprehensive Agent | ✅ Done | `ComprehensiveAgent`, `ComprehensiveAgentBuilder` in `src/comprehensive/` |

---

## Phase 13: Permission System (s03)

> *"Set boundaries first, then grant freedom"*

**Goal:** Control what tools can run, what needs approval, and what is blocked.

### Deliverables

- [x] `src/permission/mod.rs` — `PermissionRule`, `PermissionPolicy`, approval pipeline
- [x] `src/permission/rules.rs` — Built-in rules: command blocklist, path sandbox, destructive op detection
- [x] Integration in `agent.rs` — check permission before tool execution

### Design

```rust
enum ApprovalLevel {
    Allow,      // Run without asking
    Ask,        // Prompt user for confirmation
    Deny,       // Block unconditionally
}

struct PermissionRule {
    tool_pattern: String,       // glob pattern: "run_command", "write_*", "*"
    action_pattern: Option<String>, // match against tool input (e.g., command contains "rm")
    level: ApprovalLevel,
}

struct PermissionPolicy {
    rules: Vec<PermissionRule>, // checked in order, first match wins
    sandbox_paths: Vec<PathBuf>, // restrict file access to these roots
}
```

### Files to modify

- `src/agent.rs` — insert permission check before tool execution
- `src/tools/run_command.rs` — expose command string for rule matching
- `src/tools/write_file.rs` — expose file path for sandbox check

---

## Phase 14: Hook System (s04)

> *"Hook around the loop, never rewrite the loop"*

**Goal:** Add extension points around tool execution without modifying the core loop.

### Deliverables

- [x] `src/hooks/mod.rs` — `HookRegistry`, `HookEvent` enum, hook execution
- [x] `src/hooks/events.rs` — `PreToolUse`, `PostToolUse`, `SessionStart`, `SessionEnd`
- [x] Integration in `agent.rs` — fire hooks at appropriate points

### Design

```rust
enum HookEvent {
    PreToolUse { tool_name: String, input: Value },
    PostToolUse { tool_name: String, input: Value, output: String },
    SessionStart { session_id: String },
    SessionEnd { session_id: String },
}

trait Hook: Send + Sync {
    fn name(&self) -> &str;
    fn handle(&self, event: &HookEvent) -> Option<HookAction>; // None = pass, Some = modify/veto
}

enum HookAction {
    Continue,
    Veto(String),           // block the action with reason
    ModifyInput(Value),     // alter tool input before execution
    ModifyOutput(String),   // alter tool output after execution
}
```

### Files to modify

- `src/agent.rs` — fire `PreToolUse`/`PostToolUse` around tool calls
- `src/main.rs` — fire `SessionStart`/`SessionEnd`

---

## Phase 15: TodoWrite / Planning (s05)

> *"An agent without a plan drifts"*

**Goal:** Agent lists steps before executing; track progress visibly.

### Deliverables

- [x] `src/todo/mod.rs` — `TodoItem`, `TodoList`, persistence
- [x] `src/tools/todo.rs` — `TodoWriteTool`, `TodoReadTool` for the agent
- [x] Integration in `agent.rs` — inject todo state into context

### Design

```rust
struct TodoItem {
    id: String,
    description: String,
    status: TodoStatus, // Pending, InProgress, Completed, Blocked
    depends_on: Vec<String>,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

struct TodoList {
    items: Vec<TodoItem>,
    storage_path: PathBuf, // persist to disk
}
```

---

## Phase 16: Subagent (s06)

> *"Big tasks split small, each subtask gets clean context"*

**Goal:** Spawn child agents with fresh context for side tasks.

### Deliverables

- [x] `src/subagent/mod.rs` — `SubagentConfig`, `SubagentHandle`
- [x] `src/tools/subagent.rs` — `SubagentSpawnTool` for the parent agent
- [x] Context isolation: child gets fresh `messages[]`, returns only final result

### Design

```rust
struct SubagentConfig {
    system_prompt: String,
    tools: Vec<String>,      // subset of parent's tools
    max_iterations: usize,
    max_tokens: usize,
}

struct SubagentHandle {
    id: String,
    join_handle: JoinHandle<SubagentResult>,
}
```

---

## Phase 17: Skill Loading (s07)

> *"Load knowledge on demand, not upfront"*

**Goal:** Inject domain knowledge into the agent's context only when needed.

### Deliverables

- [x] `src/skills/mod.rs` — `SkillManifest`, `SkillLoader`
- [x] `src/skills/manifest.rs` — Skill metadata (name, description, triggers, file paths)
- [x] `src/tools/skill.rs` — `SkillListTool`, `SkillLoadTool`

### Design

```rust
struct SkillManifest {
    name: String,
    description: String,
    trigger_patterns: Vec<String>, // when to suggest loading
    content_path: PathBuf,         // markdown/text to inject
    tags: Vec<String>,
}

struct SkillLoader {
    skills_dir: PathBuf,
    manifests: Vec<SkillManifest>,
}
```

---

## Phase 18: Context Compaction Upgrade (s08)

> *"Context always fills up — have a way to make room"*

**Goal:** Multi-layer compaction strategies instead of just dropping oldest messages.

### Deliverables

- [x] Upgrade `src/context.rs` with layered compaction:
  - `snipCompact`: truncate large tool results to a budget
  - `microCompact`: summarize old conversation turns (call LLM)
  - `autoCompact`: trigger compaction when token usage hits threshold
- [x] `src/tools/compact.rs` — `CompactTool` for manual compaction trigger

### Compaction Layers

```
1. snipCompact  — truncate any tool_result > N chars
2. microCompact — summarize oldest N turns into a single message
3. autoCompact  — fire when context > 80% of max_tokens
```

---

## Phase 19: System Prompt Assembly (s10)

> *"Prompts are assembled at runtime, not hardcoded"*

**Goal:** Dynamic, section-based system prompt construction.

### Deliverables

- [x] Upgrade `src/prompt.rs` — `PromptAssembler` with named sections
- [x] Sections: core identity, tool descriptions, memory block, skills, user context
- [x] Each section loaded on demand, concatenated at runtime

### Design

```rust
struct PromptSection {
    name: String,
    content: String,
    priority: u8,       // ordering
    condition: Option<String>, // only include if condition met
}

struct PromptAssembler {
    sections: Vec<PromptSection>,
}

impl PromptAssembler {
    fn assemble(&self, context: &AgentContext) -> String;
}
```

---

## Phase 20: Error Recovery Upgrade (s11)

> *"Errors aren't the end, they're the start of a retry"*

**Goal:** Smarter recovery strategies beyond simple retry.

### Deliverables

- [x] Upgrade `src/client/mod.rs` with:
  - Token escalation: increase `max_tokens` on truncation
  - Fallback model: switch to cheaper model on repeated failures
  - Path switching: suggest alternative approach to the agent
- [x] `src/error_recovery/mod.rs` — `RecoveryStrategy`, `RecoveryContext`

### Strategies

```
1. Retry with backoff       — already done
2. Token escalation         — if stop_reason == "length", increase max_tokens and retry
3. Fallback model           — if rate limited > N times, switch to fallback model
4. Context compaction retry — if context too long, compact and retry
5. Path switching           — inject error info + suggest alternative to agent
```

---

## Phase 21: Task System (s12)

> *"Big goals break into small tasks, ordered, persisted to disk"*

**Goal:** File-backed task graph with dependencies, foundation for multi-agent work.

### Deliverables

- [x] `src/tasks/mod.rs` — `TaskRecord`, `TaskBoard`, dependency resolution
- [x] `src/tasks/board.rs` — Disk persistence (JSONL or SQLite)
- [x] `src/tools/task.rs` — `TaskCreateTool`, `TaskUpdateTool`, `TaskListTool`

### Design

```rust
struct TaskRecord {
    id: String,
    goal: String,
    status: TaskStatus,  // Pending, Ready, InProgress, Blocked, Completed, Failed
    blocked_by: Vec<String>,
    assigned_to: Option<String>, // agent ID
    result: Option<String>,
    created_at: DateTime<Utc>,
}

struct TaskBoard {
    tasks: Vec<TaskRecord>,
    storage: TaskStorage, // disk-backed
}
```

---

## Phase 22: Background Tasks (s13)

> *"Slow ops go background, agent keeps thinking"*

**Goal:** Run long operations in background threads; inject results via notifications.

### Deliverables

- [x] `src/background/mod.rs` — `BackgroundPool`, `Notification`
- [x] `src/tools/background.rs` — `BackgroundRunTool`, `BackgroundStatusTool`
- [x] Notification injection into agent context on completion

---

## Phase 23: Cron Scheduler (s14)

> *"Fire on schedule, no human kick needed"*

**Goal:** Time-based task triggers, agent can schedule its own future work.

### Deliverables

- [x] `src/cron/mod.rs` — `CronScheduler`, `CronJob`
- [x] `src/tools/cron.rs` — `CronScheduleTool`, `CronListTool`
- [x] Session-scoped and durable scheduling

---

## Phase 24: Agent Teams (s15)

> *"Too big for one agent — delegate to teammates"*

**Goal:** Persistent teammate agents with async mailboxes.

### Deliverables

- [x] `src/teams/mod.rs` — `MessageBus`, `AgentTeam`, `TeamAgent`
- [x] `src/teams/bus.rs` — Inbox/outbox per agent, async message passing
- [x] Permission bubbling: sub-agent actions escalate to parent

---

## Phase 25: Team Protocols (s16)

> *"Teammates need shared communication rules"*

**Goal:** Fixed request-reply format, shutdown handshake, plan approval.

### Deliverables

- [x] `src/teams/protocols.rs` — `Request`, `Reply`, `ShutdownHandshake`
- [x] `src/teams/approval.rs` — Plan approval workflow between agents

---

## Phase 26: Autonomous Agents (s17)

> *"Teammates check the board, claim work themselves"*

**Goal:** Self-organizing agents that claim tasks from a shared board.

### Deliverables

- [x] `src/teams/autonomous.rs` — Idle cycle, auto-claim logic
- [x] Integration with TaskBoard from Phase 21

---

## Phase 27: Worktree Isolation (s18)

> *"Each works in its own directory, no interference"*

**Goal:** Each agent/task works in an isolated git worktree.

### Deliverables

- [x] `src/worktree/mod.rs` — `WorktreeManager`, `WorktreeRecord`
- [x] `src/tools/worktree.rs` — `WorktreeCreateTool`, `WorktreeListTool`
- [x] Task-directory binding

---

## Phase 28: MCP Plugin (s19)

> *"Not enough capability? Plug in more via MCP"*

**Goal:** Connect external tool servers via Model Context Protocol.

### Deliverables

- [x] `src/mcp/mod.rs` — MCP client, transport layer (stdio, SSE)
- [x] `src/mcp/channel.rs` — Channel routing, tool pool assembly
- [x] MCP tools register alongside built-in tools in `ToolRegistry`

---

## Phase 29: Comprehensive Agent (s20)

> *"Many mechanisms, one loop"*

**Goal:** All 19 mechanisms integrated into one complete agent loop.

### Deliverables

- [x] `src/comprehensive/mod.rs` — Full agent with all harness mechanisms
- [x] End-to-end demo: plan → delegate → execute → recover → remember
- [x] Integration tests covering the full harness

---

## Priority Summary

```
Phase 13-14: Permission + Hooks        ← FOUNDATIONAL (prerequisite for safe layering)
Phase 15-16: TodoWrite + Subagent      ← HIGH IMPACT (biggest UX improvement)
Phase 17-19: Skills + Compaction + Prompt ← QUALITY (longer sessions, better output)
Phase 20-23: Error Recovery + Tasks    ← ROBUSTNESS (production-ready)
Phase 24-27: Multi-Agent               ← SCALE (complex workflows)
Phase 28-29: MCP + Comprehensive       ← ECOSYSTEM (extensibility)
```

## Notes

- Each phase builds on the previous; do not skip ahead
- Every phase should have its own module under `src/`
- Update this PLAN.md with ✅ as phases complete
- Reference: https://github.com/shareAI-lab/learn-claude-code
