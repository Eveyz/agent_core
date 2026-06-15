# Future Improvements — Agent Harness

> Generated: 2026-06-15  
> Status: Living document — update as progress is made

---

## Summary

The agent harness (`agent_core`) is a full-featured Rust agent runtime with a Tauri GUI frontend. The core ReAct loop, permission model, and context engine are solid. The main gap is between the **breadth of modules** and their **depth of integration** — subsystems exist as sidecars but aren't yet woven into the agent's operational lifecycle.

---

## P0 — Critical

### 1. Fix Parallel Tool Execution

**Problem:** The `TODO` in `execute_tools_with_hooks` says it all: `&self` borrow constraints on `ToolRegistry` prevent true `JoinSet` parallelism. Every tool call blocks the next. For I/O-bound tools (webfetch, file reads), this is a significant throughput gap.

**Solution:** Wrap `ToolRegistry` in `Arc<Mutex<ToolRegistry>>` or refactor to `&mut` per-tool dispatch so tools can run concurrently via `tokio::task::JoinSet`.

**Files:** `core/src/agent.rs` (tool execution path), `core/src/tools/mod.rs`

---

### 2. Add Proper Cancellation Token Propagation

**Problem:** The `abort_flag` (`AtomicBool`) is checked at the loop level, but individual tools — especially `run_command` and subagents — don't respect it. A tool that spawns a long-running process will keep running even after the user aborts.

**Solution:** Replace `AtomicBool` with `tokio_util::sync::CancellationToken` (or a similar cooperative cancellation pattern). Thread the token into every tool's `execute` method. For `run_command`, kill the child process on cancellation.

**Files:** `core/src/agent.rs`, `core/src/tools/mod.rs`, `core/src/tools/run_command.rs`, `core/src/subagent/mod.rs`

---

## P1 — High Priority

### 3. Scope Subagent Permissions

**Problem:** Subagents inherit the parent's tool registry but don't get their own `PermissionPolicy`. A compromised or confused subagent could execute destructive tools without independent approval.

**Solution:** Each subagent should receive a scoped/restricted `PermissionPolicy` — either derived from the parent's policy with additional constraints, or a fresh minimal policy. The `SubagentConfig` should accept an optional `PermissionPolicy`.

**Files:** `core/src/subagent/mod.rs`, `core/src/tools/subagent.rs`

---

### 4. Decompose the Agent God Struct

**Problem:** `Agent` (~700 lines) handles config, LLM streaming, tool execution, permission, hooks, context management, memory, skill auto-trigger, steering, follow-ups, abort, and model switching. The agent loop itself is only ~50 lines; the rest is supporting infrastructure.

**Solution:** Decompose into focused components:
- `AgentLoop` — the core turn-based loop (stream → collect → execute → repeat)
- `ToolOrchestrator` — permission check → pre-hooks → execute → post-hooks
- `PermissionGate` — standalone permission decision engine
- `HookPipeline` — pre/post tool hook execution chain
- `ContextManager` — context segments, trimming, compaction

`Agent` becomes a thin facade that composes these.

**Files:** `core/src/agent.rs` → split into `core/src/agent/mod.rs` + submodules

---

### 5. Add Retry/Backoff/Circuit Breaker to LLM Client

**Problem:** The main `run_loop` returns an error string on LLM failure. There's no retry with backoff, no fallback model, no circuit breaker. For production use, the agent should degrade gracefully rather than just stop.

**Solution:**
- Add exponential backoff retry (configurable: max retries, base delay) in `OpenAIClient::chat_completion_stream`
- Implement a simple circuit breaker: after N consecutive failures, pause and report degraded state
- Optional: fall back to a cheaper/faster model if the primary model is unavailable

**Files:** `core/src/client/mod.rs`, `core/src/client/streaming.rs`

---

## P2 — Medium Priority

### 6. Wire ComprehensiveAgent Subsystems into Agent Lifecycle

**Problem:** `ComprehensiveAgent` holds 10 `Option<...>` fields but delegates almost everything to `self.agent.run()`. The subsystems (todo, tasks, background, cron, teams, worktree) aren't integrated into the agent loop — they're just sidecars.

**Solution:**
- **Cron → Agent runs:** CronScheduler should be able to trigger `agent.run()` on schedule
- **Task board → Follow-ups:** Completed background tasks should inject follow-up messages into the agent
- **Teams → Message bus:** Team messages should be surfaced as steering messages
- **Worktrees → Context:** Active worktree path should be reflected in the Environment context segment

**Files:** `core/src/comprehensive/mod.rs`, `core/src/cron/mod.rs`, `core/src/tasks/mod.rs`, `core/src/teams/mod.rs`, `core/src/worktree/mod.rs`

---

### 7. Add `tracing` Instrumentation

**Problem:** No structured logging, no metrics, no trace IDs for correlating tool calls across turns. For debugging agent behavior at scale, you need spans per turn and per tool call.

**Solution:**
- Add `tracing` crate dependency
- Create a `tracing::span` per turn (with turn_index) and per tool call (with tool_name, tool_call_id)
- Add structured events for: LLM request/response, tool execution start/end, permission decisions, context compaction
- Optional: export to OpenTelemetry for distributed tracing

**Files:** All modules in `core/src/`

---

### 8. Real Streaming for Long-Running Tools

**Problem:** `collect_stream` buffers all tool execution updates and flushes them after the tool completes. For long-running tools (like subagents), the user sees nothing until the tool finishes.

**Solution:** Replace the buffered update pattern with real-time event emission. Tool execution updates should be streamed to the frontend as they arrive, not batched post-completion. The `ToolUpdateFn` callback already exists — wire it to emit `AgentEvent::ToolExecutionUpdate` immediately instead of buffering.

**Files:** `core/src/agent.rs` (`execute_single_tool`), `core/src/tools/mod.rs`

---

## P3 — Nice to Have

### 9. Memory Consolidation Feedback Loop

**Problem:** `maybe_consolidate` spawns a `tokio::spawn` and doesn't propagate results. If consolidation fails silently, you could accumulate unbounded stale memory.

**Solution:**
- Consolidation results should feed back into the agent's state (e.g., emit an `AgentEvent::MemoryConsolidated`)
- Add bounded retry for consolidation failures
- Log meaningful diagnostics on failure

**Files:** `core/src/agent.rs`, `core/src/memory/consolidation.rs`

---

### 10. Config Hot-Reload

**Problem:** Config is loaded once at startup. Changing model, providers, or permissions requires restarting the app.

**Solution:** Watch `~/.agverse/config.toml` for changes (using `notify` crate) and hot-reload the config into the running agent. Emit an `AgentEvent::ConfigReloaded` so the frontend can update its state.

**Files:** `core/src/config.rs`, `app/src-tauri/src/lib.rs`

---

### 11. Session Branching / Checkpointing

**Problem:** Sessions are linear — you can't branch from a previous turn or checkpoint and explore an alternative path.

**Solution:**
- Add `session.checkpoint()` — saves the current context state as a named branch point
- Add `session.branch(checkpoint_id)` — creates a new session forked from that checkpoint
- Allow the user to switch between branches in the UI

**Files:** `core/src/session.rs`, `core/src/context.rs`

---

### 12. Agent Templates / Presets

**Problem:** Every new session starts from the same default configuration. There's no way to save a "template" (e.g., "code reviewer", "doc writer") with specific system prompt, tool allowlist, and permission mode.

**Solution:**
- Add `AgentTemplate` type: `{ name, system_prompt, tools, permission_mode, model }`
- Store templates in `~/.agverse/templates/`
- UI: "New Session from Template" dropdown

**Files:** New module `core/src/template.rs`, `core/src/config.rs`

---

### 13. Multi-Model Routing

**Problem:** The agent uses a single model per session. Complex tasks might benefit from routing different steps to different models (e.g., cheap model for tool execution, expensive model for reasoning).

**Solution:**
- Add a `ModelRouter` that selects models based on task characteristics (tool-heavy → fast model, reasoning-heavy → capable model)
- Integrate with the existing `switch_model` infrastructure
- Config: `model_routing_rules` in `config.toml`

**Files:** New module `core/src/router.rs`, `core/src/agent.rs`, `core/src/config.rs`

---

## Progress Tracker

| # | Improvement | Priority | Status |
|---|------------|----------|--------|
| 1 | Parallel tool execution | P0 | ❌ Not started |
| 2 | Cancellation token propagation | P0 | ❌ Not started |
| 3 | Subagent permission scoping | P1 | ❌ Not started |
| 4 | Agent god struct decomposition | P1 | ❌ Not started |
| 5 | LLM client retry/backoff/circuit breaker | P1 | ❌ Not started |
| 6 | ComprehensiveAgent lifecycle integration | P2 | ❌ Not started |
| 7 | `tracing` instrumentation | P2 | ❌ Not started |
| 8 | Real streaming for long-running tools | P2 | ❌ Not started |
| 9 | Memory consolidation feedback loop | P3 | ❌ Not started |
| 10 | Config hot-reload | P3 | ❌ Not started |
| 11 | Session branching / checkpointing | P3 | ❌ Not started |
| 12 | Agent templates / presets | P3 | ❌ Not started |
| 13 | Multi-model routing | P3 | ❌ Not started |

---

*This document should be updated whenever a item is started, completed, or reprioritized.*
