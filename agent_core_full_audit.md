# Agverse Agent Core — Full Architecture & Code Audit

**Date:** 2026-07-05  
**Auditor:** Agverse (self-audit)  
**Scope:** Complete workspace (`core`, `cli`, `app/src-tauri`) — 44,078 lines of Rust source (excluding `target/`)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Architecture Overview](#2-architecture-overview)
3. [Subsystem Deep-Dive](#3-subsystem-deep-dive)
4. [Concurrency & Async Analysis](#4-concurrency--async-analysis)
5. [KV-Cache Strategy Analysis](#5-kv-cache-strategy-analysis)
6. [Error Handling & Recovery](#6-error-handling--recovery)
7. [Permission & Security](#7-permission--security)
8. [Code Quality Findings](#8-code-quality-findings)
9. [Bug Register](#9-bug-register)
10. [Performance Analysis](#10-performance-analysis)
11. [Improvements & Roadmap](#11-improvements--roadmap)
12. [Verdict](#12-verdict)

---

## 1. Executive Summary

Agverse is a **Rust-native autonomous AI agent framework** with a sophisticated 7-segment context engine, 5-stage compression pipeline, layered permission system, and a Brain→RunManager→Run execution architecture that supports per-Run isolation, mid-run steering, concurrent worktree execution, and subagent delegation. It ships with three frontends: a CLI (ratatui TUI), a Tauri desktop app, and a library API.

**Strengths:**
- Exceptionally well-architected context engine with KV-cache-first design (stable/dynamic segment split, fingerprint verification, cache telemetry)
- 5-stage compression pipeline with cache-aware chunked-drop strategy
- Layered permission system (sandbox → blacklist → whitelist → config → builtin → default) with audit logging
- Brain/Run separation provides clean isolation per request
- Multi-modal memory (BM25 + HNSW + embedding + salience + RRF fusion)
- Comprehensive event system (broadcast channel, JSONL event log, trace collector)
- Circuit breaker + exponential backoff + fallback chain for LLM client resilience

**Key Risks:**
- Process-global CWD mutation in subagent tool (concurrency hazard)
- `std::sync::Mutex` with `.unwrap()` in Brain (poison panic risk)
- 416 `unwrap()` calls (many in production paths)
- Blocking `std::fs` I/O in async contexts
- `eprintln!` debug output in production code paths
- System prefix budget at 8% of context window is very tight for tool catalog
- `RecoveryAction::SwitchModel` is a no-op (gives up instead of switching)

**Verdict:** Production-quality architecture with several implementation-level issues that should be addressed before scale deployment. The codebase demonstrates strong systems design but needs a hardening pass for concurrency safety, async correctness, and error resilience.

---

## 2. Architecture Overview

### 2.1 Workspace Structure

```
agent_core/
├── core/           # Main library crate (agent_core)
│   └── src/
│       ├── agent/          # Agent builder + executor + scheduler
│       ├── client/         # OpenAI-compatible HTTP client (resilience, streaming)
│       ├── context.rs      # 7-segment context engine
│       ├── compressor.rs   # 5-stage compression pipeline
│       ├── config.rs       # Config loading (TOML), model auto-detection
│       ├── hooks/          # Hook system (pre/post tool, before/after model)
│       ├── hygiene.rs      # Request-boundary message sanitization
│       ├── mcp/            # Model Context Protocol client
│       ├── memory/         # Memory system (core, recall, archival, BM25, HNSW)
│       ├── permission/     # Layered permission system
│       ├── runtime/        # Brain, RunManager, Run, events, supervisor
│       ├── skills/         # Skill manager (auto-trigger, catalog)
│       ├── tools/          # 14 built-in tools + registry
│       ├── workflow/       # DAG workflow engine
│       └── ...
├── cli/           # CLI frontend (ratatui TUI)
│   └── src/
│       ├── main.rs         # CLI entry point + REPL
│       ├── tui/            # Terminal UI (state, render, input, widgets)
│       └── cli_completer.rs
└── app/src-tauri/ # Tauri v2 desktop app
    └── src/
        ├── main.rs         # Entry point (6 lines)
        └── lib.rs          # Full Tauri bridge (1796 lines, ~40 commands)
```

### 2.2 Size Distribution (top 15 source files)

| File | Lines | Role |
|------|-------|------|
| `app/src-tauri/src/lib.rs` | 1,796 | Tauri command bridge |
| `core/src/runtime/run.rs` | 1,726 | Run execution loop |
| `cli/src/main.rs` | 1,712 | CLI entry + REPL |
| `core/src/context.rs` | 1,384 | 7-segment context engine |
| `cli/src/tui/state.rs` | 1,204 | TUI state management |
| `core/src/permission/mod.rs` | 1,194 | Permission system |
| `core/src/session.rs` | 1,121 | Session persistence |
| `core/src/agent/mod.rs` | 1,089 | Agent builder + facade |
| `core/src/tasks/mod.rs` | 1,014 | Task DAG board |
| `core/src/config.rs` | 998 | Config loading |
| `cli/src/tui/widgets/blocks.rs` | 939 | TUI rendering widgets |
| `core/src/tools/webfetch.rs` | 865 | Web fetch tool |
| `core/src/tools/subagent.rs` | 838 | Subagent spawning tool |
| `core/src/memory/mod.rs` | 741 | Memory manager |
| `core/src/compressor.rs` | 732 | Compression pipeline |

### 2.3 Dependency Graph (core)

```
Config ──→ Brain ──→ RunManager ──→ Run (per request)
              │           │              │
              │           │              ├── ContextEngine (7 segments)
              │           │              ├── ToolRegistry (14 tools)
              │           │              ├── PermissionPolicy (layered)
              │           │              ├── OpenAIClient (retry+fallback)
              │           │              ├── RecoveryEngine
              │           │              ├── ProcessSupervisor (child kill)
              │           │              └── HookRegistry
              │           │
              ├── MemoryManager (SQLite + BM25 + HNSW + Embeddings)
              ├── SkillManager (auto-trigger)
              ├── ReflectionDaemon (Deep mode)
              └── Reflector (offline analysis)
```

---

## 3. Subsystem Deep-Dive

### 3.1 Context Engine (`context.rs`, 1384 lines)

**Design:** 7-segment semantic prompt assembly with per-segment refresh policies and KV-cache stability classification.

| Segment | Name | Budget (tokens) | Refresh | Stability |
|---------|------|-----------------|---------|-----------|
| 1 | IDENTITY | 200 | Never | Stable |
| 2 | PRINCIPLES | 400 | OnEvent | Stable |
| 3 | ENVIRONMENT | 200 | PerTurn | SemiStable |
| 4 | TOOL CATALOG | dynamic | OnRegister | Stable |
| 5 | ACTIVE MEMORY | 600 | PerTurn | Dynamic |
| 6 | LOADED SKILLS | 2000 | PerTurn | Dynamic |
| 7 | EXECUTION PLAN | 300 | PerTurn | Dynamic |

**Key mechanism:**
- `assemble_system_prompt()` — frozen prefix from Stable segments only → cacheable
- `assemble_context_injection()` — dynamic segments injected as trailing `<context_injection>` user message → doesn't invalidate prefix cache
- `stable_prefix_fingerprint()` — hash-based drift detection across turns
- `cache_hint()` — provides KV cache strategy hints ("full"/"partial"/"none")

**Observations:**
- `system_prefix_budget` is set to `max_tokens * 0.08` (8%). For a 128K context, that's ~10K tokens. The tool catalog alone can easily exceed this (14 tools × ~50 tokens each = 700 tokens, but descriptions push it higher). The LOADED SKILLS segment has a 2000-token budget but is Dynamic, so it goes in the injection, not the system prompt. This is correct.
- Token counting uses `tiktoken-rs` (o200k_base BPE) with a `OnceLock` cached singleton — good.
- `truncate_to_token_budget` uses binary search over char boundaries — O(n log n) but correct.
- The `micro_compact` and `trim_to_fit_legacy` methods are still present (backward compat) but superseded by `chunked_drop` and `trim_to_fit`.

### 3.2 Compression Pipeline (`compressor.rs`, 732 lines)

**5-stage pipeline:**

| Stage | Name | Method | LLM? | Cache Impact |
|-------|------|--------|------|--------------|
| 1 | snipCompact | Truncate tool results > per-tool-kind budget | No | Low (content shrinks) |
| 2 | dedupCompact | Collapse consecutive identical tool outputs | No | Low |
| 3 | chunkCompact | Merge tool_call+result pairs into system chunks | No | Medium (changes message structure) |
| 4 | summaryCompact | LLM-generated structured summary of old turns | Yes | High (replaces prefix) |
| 5 | gradientCompact | Age-tiered: recent=raw, old=summary | Yes | High |

**Cache-optimized compaction strategy** (in `Run::maybe_compact()`):
1. **Tier 1 — Chunked drop** (preferred): Batch-remove oldest 50% of turns. Single-turn cache miss, then 10+ turns of full cache hits. Zero LLM overhead.
2. **Tier 2 — LLM summarize** (fallback): If chunked drop insufficient, use `force_compact()`.
3. **Tier 3 — micro_compact** (last resort): Naive head/tail preview compression.

**Observations:**
- `chunk_compact` protects the last 8 messages to preserve API-required tool_call/tool_result pairing — correct.
- `dedup_compact` skips results < 50 chars — reasonable heuristic.
- `snip_compact` delegates to `hygiene::policy::truncate_content()` which has per-tool-kind budgets (Instruction-class tools like `skill_load` are never truncated; `read_file` is ActivelyRead with a 24K char cap; incidental tools like `bash` get 16K).
- `run_pipeline()` only runs stages 1-3; stages 4-5 are invoked separately by the caller. This is correct design — LLM-dependent stages shouldn't be in the synchronous pipeline.

### 3.3 Runtime Architecture (`runtime/`)

**Three-tier:**

```
Brain (shared, stateless)
  ├── Config, Memory, Skills, Reflector, Hooks
  └── RunManager (owns Brain, tracks Runs)
        └── Run (per-request, isolated)
              ├── Owns Context, Tools, Permissions, Client, Cancel
              ├── ProcessSupervisor (child process kill on cancel)
              ├── JoinSet (background tasks aborted on cancel)
              └── ApprovalResolver (per-Run, no global map)
```

**Run lifecycle:**
1. `create_run()` → RunId (Created state)
2. `command(Start)` → Run begins (Running state)
3. Turn loop: `poll_commands()` → `refresh_context_segments()` → `model_turn()` → `execute_tools()` → repeat
4. Terminal: Completed / Cancelled / Failed
5. RAII cleanup: supervisor + join_set + cancel all fire on drop

**Key design decisions:**
- Per-Run `ApprovalResolver` replaces deprecated global pending-approvals map (eliminates actor deadlock)
- `EventGuard` RAII pattern ensures orphaned tool blocks get `ToolEnded{is_error:true}` even on panic
- Broadcast channel (1024 capacity) for events, mpsc (64 capacity) for commands
- Event logging runs as a separate subscriber task — persists streaming events that bypass `emit()`
- `context_snapshot` (RwLock) refreshed at turn boundaries for side-channel `/btw` queries

**Observations:**
- The Agent wrapper (`agent/mod.rs`) delegates to RunManager but then manually syncs messages back from the Run after completion (lines 546-558). This is a somewhat awkward impedance mismatch — the Agent's ContextEngine is repopulated from the Run's final messages.
- `run_manager.runs_mut()` returns a `tokio::sync::MutexGuard` — the Agent wrapper removes the handle after the run to `join()` it. This means the runs map is briefly locked during handle removal, but this is acceptable since it's post-completion.

### 3.4 Permission System (`permission/`, 1194+ lines)

**Layered evaluation (first match wins):**

```
Sandbox (path boundary)       ← hard deny, even in Yolo mode
  ↓
Yolo mode                     ← allow everything
  ↓
Blacklist (config)            ← unconditional deny
  ↓
Whitelist (session + config)  ← unconditional allow
  ↓
auto_allow_up_to (config)     ← allow if danger ≤ threshold (except builtin_deny)
  ↓
Mode-based (Paranoid/Standard/Developer/Permissive)
  ↓
Config rules (config.toml)    ← user-defined overrides
  ↓
Built-in rules (rules.rs)     ← default posture
  ↓
Default: Ask                   ← catch-all
```

**Key safety features:**
- Destructive shell commands (`rm -rf`, `mkfs`, `sudo`, etc.) are **hard-denied** by built-in rule, bypassable only by explicit whitelist entry — `auto_allow_up_to = Destructive` cannot bypass this.
- Sandbox path checking canonicalizes both target and sandbox roots, handles non-existent paths (write_file creating new files) by canonicalizing the parent directory.
- Audit logging records every permission decision with tool name, input, rule source, danger level, and reason.

**Observations:**
- `PermissionPolicy::check()` takes `&mut self` — this is required because `danger_level_for()` accesses `self.sandbox_paths` and the method records audit entries. However, this means the permission check cannot be done in parallel across multiple tool calls. The `ToolOrchestrator` handles this by checking permissions sequentially before parallel execution.
- `is_destructive_command()` and `is_readonly_command()` are pattern-matching heuristics — they could miss novel destructive commands (e.g., `dd if=/dev/zero of=/dev/sda`).

### 3.5 Memory System (`memory/`, 741+ lines in mod.rs)

**Three-tier memory:**

| Tier | Type | Storage | Search Method |
|------|------|---------|---------------|
| Core | Manual notes (RAM) | SQLite | Direct read |
| Recall | Conversation history | SQLite + embeddings | Vector + BM25 + HNSW + RRF |
| Archival | Long-term knowledge | SQLite + embeddings | Vector search |

**Search pipeline (hybrid):**
1. BM25 keyword retrieval (tantivy) → top 150 candidates
2. HNSW vector search (instant-distance) → top 150 candidates
3. RRF (Reciprocal Rank Fusion) merges both lists
4. Salience scoring (recency decay × importance × memory strength × category weight)
5. Final reranking → top_k results

**Memory modes:**
- **Stateless**: No memory, no SQLite, no recall
- **Standard**: Core + recall + archival, BM25 + HNSW
- **Deep**: Standard + background reflection daemon + advanced recall

**Observations:**
- `search_conversation_precomputed()` accepts a pre-computed query embedding to avoid blocking the memory lock during embedding computation — good design.
- `store_conversation()` syncs to both BM25 and HNSW indexes. The HNSW sync computes embeddings synchronously inside the lock — this could block for 10-50ms per store.
- `MemoryConsolidator` runs O(n²) cosine similarity dedup every 20 turns, but correctly clones the consolidator before releasing the lock and runs the heavy work via `spawn_blocking`.

### 3.6 Tool System (`tools/`)

**14 built-in tools:**

| Tool | Danger Level | Execution Mode | Notes |
|------|-------------|----------------|-------|
| read_file | ReadOnly | Parallel | Binary detection, 1MB limit |
| write_file | ReadWrite | Parallel | Creates parent dirs |
| edit | ReadWrite | Parallel | Exact string match, backup |
| grep | ReadOnly | Parallel | Regex search |
| glob | ReadOnly | Parallel | Glob pattern match |
| bash | System/Destructive | Sequential | ProcessSupervisor, timeout |
| webfetch | Network | Parallel | robots.txt, readability |
| tavily_search | Network | Parallel | API key from env |
| recall_memory | ReadOnly | Parallel | Conversation search |
| archival_memory_* | ReadOnly | Parallel | Insert/search/delete |
| core_memory | ReadOnly | Parallel | Read/replace blocks |
| todo | ReadOnly | Parallel | Write/update todo list |
| skill | ReadOnly | Parallel | List/load/activate |
| subagent | System | Sequential | Spawns sub-agents |

**Tool trait:**
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<String>;
    async fn execute_with_stream(&self, args: Value, on_update: Option<ToolUpdateFn>, 
                                  event_sender: Option<EventSender>) -> Result<String>;
    fn execution_mode(&self) -> Option<ToolExecutionMode>;
}
```

**Observations:**
- `ToolRegistry::resolve_execution_mode()` forces the entire batch to Sequential if any single tool requests it — correct for safety.
- JSON schema validation is performed before execution via `jsonschema` crate — good defense against malformed LLM arguments.
- `try_lock_memory()` helper uses `try_lock_for(3s)` to avoid deadlocks when memory is consolidating — returns a JSON busy-message to the LLM.

### 3.7 Client & Streaming (`client/`)

**OpenAIClient features:**
- Chain-of-fallbacks: up to 3 fallback models, traversed on circuit breaker open or retry exhaustion
- Circuit breaker: 5 failures → open (60s reset timeout) → half-open (single probe)
- Exponential backoff with jitter: 500ms base, 10s max, 3 retries
- Retry-After header respected for 429 responses
- SSE streaming parser with `ToolCallAccumulator` and `TokenAccumulator` (50ms/256char batching)
- `${VAR}` env-var resolution for API keys (plaintext never persisted in Config)
- Cache usage telemetry extraction (`prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`)

**Observations:**
- `build_http_client()` calls `.no_proxy()` — this forces all traffic direct, bypassing corporate proxies. This is likely intentional for security but could break deployments behind corporate firewalls.
- `.no_gzip().no_deflate().no_brotli()` disables compression — unusual for an LLM client. Possibly to avoid CPU overhead on streaming, or because some providers don't support compressed streaming.
- The `expect("failed to build http client")` on line 68 will panic if the client builder fails (e.g., TLS backend issue). This is acceptable for a startup-time construct.

### 3.8 Hook System (`hooks/`)

**8 hook events:** PreToolUse, PostToolUse, SessionStart, SessionEnd, TurnStart, TurnEnd, BeforeModel, AfterModel

**5 hook actions:** Continue, Veto, ModifyInput, ModifyOutput, SkipModel

**Key use cases:**
- `SkipModel` — short-circuit LLM call with preset answer (used in tests, could be used for caching)
- `Veto` — block a tool call
- `ModifyInput` — rewrite tool arguments before execution

**Observations:**
- Hooks are registered globally in `HookRegistry` (shared via `Arc<Mutex<HookRegistry>>`).
- The `fire_*` methods take `&self` — hooks are read-only during firing, which is correct for concurrent access.
- The `LoggingHook` uses `eprintln!` for all events — this should use `tracing` instead.

### 3.9 Skills System (`skills/`)

**SkillManager:**
- Scans `~/.agverse/skills/*/SKILL.md` for skill manifests
- Auto-triggers based on keyword matching against user input
- `@skill:name` explicit activation syntax
- Skills are injected into the LOADED SKILLS context segment (Dynamic, 2000-token budget)
- Skill content is loaded on-demand, not at scan time

### 3.10 Workflow Engine (`workflow/`)

**DAG-based workflow execution:**
- Workflows defined as JSON with nodes (LLM call, tool call, sub-workflow, condition)
- Dependencies between nodes form a DAG
- `WorkflowExecutor` runs nodes in topological order
- Trust system controls which workflows auto-execute vs require approval
- Validation ensures no cycles, all references resolve

### 3.11 Session Management (`session.rs`)

- Full conversation snapshots saved to SQLite (`sessions` + `session_messages` tables)
- Auto-title generation from first user message
- Session search by title/summary keyword
- Subagent sessions linked to parent via `parent_session_id`
- Session types: `main`, `subagent`

---

## 4. Concurrency & Async Analysis

### 4.1 Lock Inventory

| Lock Type | Location | Purpose | Risk |
|-----------|----------|---------|------|
| `parking_lot::Mutex<MemoryManager>` | Brain, Agent, tools | Memory operations | Medium — long-held during embedding |
| `parking_lot::Mutex<SkillManager>` | Brain, Agent, Run | Skill catalog | Low — brief operations |
| `parking_lot::Mutex<TodoList>` | Brain | Todo state | Low |
| `parking_lot::Mutex<HookRegistry>` | Brain, Run | Hook firing | Low — read-only during fire |
| `parking_lot::Mutex<PermissionPolicy>` | Run | Permission checks | Medium — `&mut self` for check() |
| `parking_lot::Mutex<ProcessSupervisor>` | Run | Child process tracking | Low |
| `parking_lot::Mutex<TraceCollector>` | Agent | Trace recording | Low |
| `std::sync::Mutex<AgentMode>` | Brain | Mode get/set | **High — `.unwrap()` on poison** |
| `parking_lot::RwLock<RunState>` | RunHandle | State querying | Low |
| `parking_lot::RwLock<Vec<Message>>` | RunHandle | Context snapshot | Low |
| `tokio::sync::Mutex<HashMap<RunId, RunHandle>>` | RunManager | Run tracking | Low — async-aware |
| `tokio::sync::broadcast::Sender` | Run | Event broadcasting | Low |
| `tokio::sync::mpsc::Sender` | RunHandle | Command channel | Low |

### 4.2 Concurrency Issues

**BUG-001: Process-global CWD mutation in subagent tool**
- `tools/subagent.rs:647,655`: `std::env::set_current_dir()` mutates process-global state
- If two subagents run concurrently (parallel tool execution), they will race on the process CWD
- **Severity: HIGH** — can cause file operations in the wrong directory
- **Fix:** Use `Command::current_dir()` instead of `std::env::set_current_dir()` for spawned processes

**BUG-002: `std::sync::Mutex` with `.unwrap()` in Brain**
- `runtime/brain.rs:292,298`: `*self.current_mode.lock().unwrap()`
- If any code panics while holding this lock, subsequent calls will panic with poison error
- **Severity: MEDIUM** — mode is rarely written, but poison would crash the agent
- **Fix:** Use `parking_lot::Mutex` (no poison) or handle the `PoisonError`

**BUG-003: Memory lock held during embedding computation**
- `memory/mod.rs:174-183`: `store_conversation()` calls `model.embed_single(content)` while holding no explicit lock, but the caller (Run) holds `mem.lock()`
- In `run.rs:760-761`: `let m = mem.lock(); let _ = m.store_conversation(...);` — the lock is held during the entire store, including embedding computation (10-50ms)
- **Severity: MEDIUM** — blocks all other memory operations during embedding
- **Fix:** Compute embedding before acquiring lock, or use `search_conversation_precomputed` pattern

### 4.3 Async Correctness

**ISSUE-001: Blocking `std::fs` in async contexts**
- `runtime/run.rs:1325-1370`: `refresh_context_segments()` reads 4+ files synchronously via `std::fs::read_to_string()` inside an async function
- `tools/edit.rs:49,65`: `std::fs::read_to_string()` and `std::fs::write()` in async tool execute
- `tools/grep.rs:92,108`: `std::fs::read_to_string()` and `std::fs::read_dir()` in async
- `tools/read_file.rs:97,109`: `std::fs::metadata()` and `std::fs::File::open()` in async
- **Impact:** Each blocking call ties up a tokio worker thread. For fast SSDs this is ~1ms per call, but on network filesystems or slow disks it can be 100ms+.
- **Fix:** Use `tokio::fs::read_to_string()` or `spawn_blocking` for file I/O in hot paths

**ISSUE-002: `eprintln!` in production code paths**
- `runtime/run.rs:651,665,674`: Debug output in `resolve_approval()` — prints to stderr on every approval resolution
- `agent/executor.rs:71,91,124,251`: Debug output in tool orchestrator
- `hooks/mod.rs:215-241`: `LoggingHook` uses `eprintln!` for all hook events
- `mcp/mod.rs:188,192,323`: MCP server lifecycle logging
- **Impact:** Performance (I/O on hot path), log pollution, no level control
- **Fix:** Replace all with `tracing::debug!()` or `tracing::info!()`

---

## 5. KV-Cache Strategy Analysis

### 5.1 Architecture

The context engine is explicitly designed for DeepSeek prefix caching (and OpenAI prompt caching). The strategy is:

```
[System Message (Stable segments only)]     ← FROZEN, CACHEABLE
[Conversation History (untouched)]           ← CACHEABLE (append-only)
[<context_injection>Dynamic segments</context_injection>]  ← NOT CACHEABLE (changes every turn)
```

### 5.2 Stability Classification

| Segment | Stability | In System Prompt? | Cacheable? |
|---------|-----------|-------------------|------------|
| Identity | Stable | ✅ | ✅ |
| Principles | Stable | ✅ | ✅ |
| Environment | SemiStable | ❌ (injection) | ❌ |
| Tool Catalog | Stable | ✅ | ✅ |
| Active Memory | Dynamic | ❌ (injection) | ❌ |
| Loaded Skills | Dynamic | ❌ (injection) | ❌ |
| Execution Plan | Dynamic | ❌ (injection) | ❌ |

**Observation:** The Environment segment is classified as SemiStable but is NOT included in the system prompt (only Stable segments are). It goes into the context injection. This means the CWD, OS, and time are injected fresh every turn — correct for time (changes every turn) but means the system prompt is purely identity + principles + tool catalog.

### 5.3 Drift Detection

- `stable_prefix_fingerprint()` computes a `DefaultHasher` hash of all Stable segment names + contents
- Checked at the start of each turn in `run_turn()` (line 708)
- If fingerprint changes, emits `CacheInfo { hit_rate: -1.0 }` as a "prefix drifted" signal
- `last_prefix_fingerprint` is updated after detection

### 5.4 Cache Telemetry

- `StreamEvent::CompleteWithUsage` extracts `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens` from the final SSE chunk
- `CacheMetrics` accumulates per-Run hit/miss stats
- `CacheSummary` event emitted before Run completion with cumulative hit rate
- Idle detection: if > 240 seconds since last turn, warns that cache likely expired (DeepSeek ~5-10 min timeout)

### 5.5 Compaction Cache Impact

| Strategy | Cache Miss Turns | Recovery | LLM Cost |
|----------|-----------------|----------|----------|
| Chunked drop (preferred) | 1 | Immediate | 0 |
| LLM summarize | 1 | 2-5s latency | 1 API call |
| micro_compact | 1 | Immediate | 0 |
| snip/dedup/chunk (stages 1-3) | 0-1 | Immediate | 0 |

The chunked-drop strategy is specifically optimized for DeepSeek: one cache miss, then 10+ turns of full cache hits. The code comments explain this reasoning clearly.

### 5.6 Concerns

**CONCERN-001: System prefix budget too tight**
- `system_prefix_budget = max_tokens * 0.08` → 10,240 tokens for 128K context
- Identity (200) + Principles (400) + Tool Catalog (dynamic) = ~600+ tokens minimum
- If tool catalog grows (MCP tools, many skills), it could be truncated
- **Impact:** Truncated tool catalog → LLM doesn't know about available tools
- **Fix:** Increase to 15-20% or make configurable

**CONCERN-002: Tool catalog rebuilt every turn**
- `refresh_context_segments()` calls `set_tool_catalog()` every turn
- `set_tool_catalog()` has a content-equality check to skip no-op updates — good
- But the `build_danger_map()` and `build_tool_catalog_string()` are still called every turn
- **Impact:** Minor CPU overhead per turn (~0.1ms)
- **Fix:** Cache the catalog string and only rebuild when tools or permissions change

---

## 6. Error Handling & Recovery

### 6.1 Error Types

The codebase uses `anyhow::Result<T>` throughout for application-level error handling. This is consistent with the user's coding convention preference. Specific error types:

- `RunError` (enum: `Cancelled`, `Failed(String)`) — Run execution errors
- `PermissionDecision` (enum: `Allow`, `Ask(String, ApprovalPrompt)`, `Deny(String)`) — permission outcomes
- `RecoveryAction` (enum: `CompactContext`, `EscalateTokens`, `Retry`, `SwitchModel`, `Fail`) — recovery strategies
- `PreToolResult` (enum: `Proceed(Value)`, `Veto(String)`) — hook results

### 6.2 Recovery Engine

`RecoveryEngine` implements loop-level error recovery (distinct from HTTP-level retry/fallback in `OpenAIClient`):

1. **Context too long** → `CompactContext { target_ratio }` → `force_compact()` → retry
2. **Token limit exceeded** → `EscalateTokens { new_max_tokens }` → increase max_tokens → retry
3. **Transient error** → `Retry { delay_ms }` → sleep → retry
4. **Model failure** → `SwitchModel { model }` → **GIVES UP** (TODO: implement)
5. **Unrecoverable** → `Fail` → GiveUp

**Max recovery attempts:** 3 per model_turn()

### 6.3 Error Handling Issues

**BUG-004: `RecoveryAction::SwitchModel` is a no-op**
- `runtime/run.rs:1203-1209`: The SwitchModel recovery action just emits an error and returns `GiveUp`
- Comment says "Model switching at runtime is complex — for now, just give up"
- **Severity: LOW** — fallback chain in OpenAIClient handles HTTP-level failures; SwitchModel is for loop-level model migration
- **Fix:** Implement by rebuilding the client with the new model config

**BUG-005: `resolve_input` is unimplemented**
- `runtime/run.rs:677-679`: `resolve_input()` is a no-op with a TODO comment
- **Severity: LOW** — input request mechanism is a future feature
- **Fix:** Either implement or remove the command variant

**ISSUE-003: Error messages returned to LLM as tool results**
- When a tool fails, the error string is returned as the tool result content (e.g., "Error executing tool 'bash': ...")
- The LLM sees this as a normal tool result and may try to parse it
- **Impact:** LLM may get confused by error messages mixed with success results
- **Fix:** Consider using the `is_error` flag on `ToolEnded` events more consistently, and potentially prefix error results with a clearer marker

### 6.4 HTTP Client Resilience

**Circuit Breaker:**
- 5 failures → Open (block all requests for 60s)
- After 60s → HalfOpen (allow single probe request)
- Probe success → Closed (normal operation)
- Probe failure → Open (another 60s)

**Retry Strategy:**
- 3 retries with exponential backoff (500ms base, 10s max, jitter)
- Retries on: 429 (rate limit), 5xx (server errors), network errors
- Does NOT retry on: 4xx (client errors, except 429)
- `Retry-After` header respected for 429s

**Fallback Chain:**
- Up to 3 fallback models, configured via `fallback_model` in config
- Chain traversed on: circuit breaker open OR retry exhaustion
- Each model in the chain has its own circuit breaker

**Observation:** The resilience design is solid. The circuit breaker prevents cascading failures, the fallback chain provides model redundancy, and the retry strategy with jitter prevents thundering herd.

---

## 7. Permission & Security

### 7.1 Permission Modes

| Mode | Behavior |
|------|----------|
| Paranoid | Everything prompts except explicit config rules |
| Standard | Built-in + config rules, default Ask |
| Developer | ReadOnly auto-allow, others Ask |
| Permissive | ReadOnly + ReadWrite + Network auto-allow, others Ask |
| Yolo | Everything allowed (even Destructive, but sandbox still enforced) |

### 7.2 Security Boundaries

**Sandbox (strongest):**
- Enforced before ALL other layers (including Yolo mode)
- Path canonicalization handles symlinks and relative paths
- Non-existent paths handled by canonicalizing parent directory
- If sandbox_paths is empty, sandbox is disabled (all paths allowed)

**Destructive command deny (strong):**
- `rm -rf`, `mkfs`, `sudo`, `dd`, `shred`, etc. are hard-denied
- Cannot be bypassed by `auto_allow_up_to = Destructive`
- Can only be bypassed by explicit whitelist entry for the specific command

**API key protection:**
- `${VAR}` env-var resolution at point of use
- Plaintext key never stored in Config struct (resolved to `model.api_key` only in `OpenAIClient`)
- Config file can be shared without leaking secrets

### 7.3 Security Concerns

**CONCERN-003: `no_proxy()` on HTTP client**
- `client/mod.rs:66`: `.no_proxy()` forces all traffic direct
- Breaks corporate proxy users who need to route through a proxy
- **Fix:** Make this configurable, default to allowing proxies

**CONCERN-004: `is_destructive_command` is pattern-based**
- Uses string matching against a list of known destructive commands
- Novel destructive commands (e.g., `python -c "import os; os.system('rm -rf /')"` ) would not be caught
- **Fix:** Consider running bash commands in a container/sandbox as defense-in-depth

**CONCERN-005: `webfetch` robots.txt compliance**
- The webfetch tool uses `robotstxt` crate for robots.txt checking — good
- But the `tavily_search` tool makes no such checks (it's a search API, not a crawler — acceptable)

---

## 8. Code Quality Findings

### 8.1 `unwrap()` Analysis

**Total: 416 `unwrap()` calls**

Breakdown by location:
- **Tests:** ~280 calls (67%) — acceptable
- **Production code:** ~136 calls (33%) — needs review

**Critical production `unwrap()` calls:**

| File:Line | Code | Risk |
|-----------|------|------|
| `runtime/brain.rs:292` | `*self.current_mode.lock().unwrap()` | **HIGH** — poison panic |
| `runtime/brain.rs:298` | `*self.current_mode.lock().unwrap() = mode` | **HIGH** — poison panic |
| `agent/mod.rs:831` | `self.config.get_model(&self.current_model_name).unwrap()` | **MEDIUM** — panic if model removed |
| `client/mod.rs:68` | `.expect("failed to build http client")` | **LOW** — startup only |
| `memory/hnsw.rs:76,83,96` | `.expect("HNSW lock poisoned")` | **LOW** — parking_lot (no poison) |

### 8.2 `unsafe` Blocks

**Total: 5 `unsafe` blocks**

| File:Line | Usage | Acceptable? |
|-----------|-------|-------------|
| `config.rs:848` | `std::env::set_var("TEST_API_KEY", ...)` | ✅ Test only |
| `config.rs:851` | `std::env::remove_var("TEST_API_KEY")` | ✅ Test only |
| `config.rs:868` | `std::env::set_var("MY_KEY", ...)` | ✅ Test only |
| `config.rs:890` | `std::env::remove_var("MY_KEY")` | ✅ Test only |
| `runtime/supervisor.rs:246` | Process group manipulation (`setpgid`) | ✅ Required for child process management |

### 8.3 `panic!` Macros

**Total: 10 `panic!` calls** — all in test code (assertions in test helpers). No production panics.

### 8.4 Code Organization

**Good:**
- Clear module boundaries with `mod.rs` re-exports
- Consistent use of `anyhow::Result` for fallible operations
- Comprehensive doc comments on public APIs
- Test coverage in critical modules (compressor, context, hooks, permission)

**Needs improvement:**
- `app/src-tauri/src/lib.rs` is 1,796 lines — should be split into multiple modules
- `runtime/run.rs` is 1,726 lines — the Run struct does too much; consider extracting `TurnExecutor`, `ContextRefresher`, `CompactStrategy`
- Legacy code (`PromptAssembler`, `trim_to_fit_legacy`, `global_pending_approvals`) should be marked for removal

### 8.5 Test Coverage

| Module | Has Tests | Coverage Quality |
|--------|-----------|-----------------|
| compressor.rs | ✅ | Good — all 5 stages tested |
| context.rs | ✅ | Good — segment creation, assembly, token counting |
| hooks/mod.rs | ✅ | Good — veto, modify input, fire order |
| agent/mod.rs | ✅ | Good — builder, context processors, preset answer |
| permission/mod.rs | ✅ | Fair — needs more edge case testing |
| client/streaming.rs | ✅ | Good — accumulator batching |
| hygiene.rs | ✅ | Good — truncation, signal preservation |
| runtime/run.rs | ❌ | **Missing** — no unit tests for Run (only integration via Agent) |
| memory/ | ❌ | **Missing** — no unit tests for MemoryManager |
| tools/ | Partial | Some tools have tests (read_file, glob), others don't |

---

## 9. Bug Register

### Critical

| ID | Description | Location | Impact |
|----|-------------|----------|--------|
| BUG-001 | Process-global CWD mutation in subagent tool | `tools/subagent.rs:647,655` | Concurrent subagents race on process CWD, causing file operations in wrong directory |

### High

| ID | Description | Location | Impact |
|----|-------------|----------|--------|
| BUG-002 | `std::sync::Mutex` with `.unwrap()` in Brain | `runtime/brain.rs:292,298` | Poison panic crashes agent if any panic occurs while holding mode lock |
| BUG-003 | Memory lock held during embedding computation | `runtime/run.rs:760-761`, `memory/mod.rs:174` | Blocks all memory operations for 10-50ms per conversation store |

### Medium

| ID | Description | Location | Impact |
|----|-------------|----------|--------|
| BUG-004 | `RecoveryAction::SwitchModel` is a no-op | `runtime/run.rs:1203-1209` | Model migration recovery doesn't work; gives up instead |
| BUG-005 | `resolve_input` unimplemented | `runtime/run.rs:677-679` | Input request mechanism doesn't work |
| BUG-006 | `agent/mod.rs:831` `unwrap()` on model lookup | `agent/mod.rs:831` | Panic if current model is removed from config at runtime |

### Low

| ID | Description | Location | Impact |
|----|-------------|----------|--------|
| BUG-007 | `eprintln!` in production code | Multiple files | Log pollution, no level control, performance |
| BUG-008 | Blocking `std::fs` in async contexts | `runtime/run.rs`, `tools/*.rs` | Ties up tokio worker threads |
| BUG-009 | `system_prefix_budget` at 8% may truncate tool catalog | `context.rs:215` | LLM may not see all available tools |
| BUG-010 | Tool catalog rebuilt every turn despite no-op check | `runtime/run.rs:1294-1297` | Minor CPU waste |

---

## 10. Performance Analysis

### 10.1 Hot Paths

**Per-turn cost breakdown (estimated):**

| Operation | Time | Blocking? |
|-----------|------|-----------|
| `refresh_context_segments()` | 1-5ms | Yes (std::fs reads for agverse.md) |
| `build_messages()` | <0.1ms | No |
| `hygiene::sanitize()` | <0.1ms | No |
| `client.chat_completion_stream()` | 500-5000ms | No (async streaming) |
| `collect_stream()` | (overlapped with above) | No |
| `execute_tools()` | 1-10000ms | No (async) |
| `refresh_context_snapshot()` | <0.1ms | No |
| `stable_prefix_fingerprint()` | <0.1ms | No |

**Per-store memory cost:**
| Operation | Time | Blocking? |
|-----------|------|-----------|
| `embed_single()` | 10-50ms | Yes (CPU-bound) |
| SQLite INSERT | 1-5ms | Yes (I/O) |
| BM25 insert | 1-5ms | Yes (I/O) |
| HNSW add_fallback | <1ms | No (in-memory) |

### 10.2 Optimization Opportunities

1. **Pre-compute embeddings outside lock** — `store_conversation()` should accept an optional pre-computed embedding (like `search_conversation_precomputed` already does)
2. **Spawn blocking for file reads** — `refresh_context_segments()` reads 4+ files synchronously; use `tokio::fs` or `spawn_blocking`
3. **Cache tool catalog string** — Only rebuild when tool registry or permission policy changes
4. **Lazy BM25/HNSW sync** — Batch-sync indexes every N stores instead of per-store
5. **Token accumulator for tool results** — Large tool results could be streamed/batched like model tokens

### 10.3 Memory Usage

- **SQLite:** Single database file, WAL mode likely (not verified)
- **HNSW index:** In-memory, grows with conversation history. `instant-distance` with fallback pool.
- **BM25 index:** In-memory tantivy index, grows with conversation history.
- **Embeddings:** Stored as BLOB in SQLite, loaded into memory for HNSW.
- **Event log:** JSONL files per Run, written sequentially. No rotation/cleanup mechanism.

**Concern:** For long-running sessions, the BM25 and HNSW indexes grow unboundedly in memory. There's no eviction or size limit.

---

## 11. Improvements & Roadmap

### 11.1 Immediate Fixes (P0)

1. **Fix process-global CWD mutation in subagent tool** — Use `Command::current_dir()` or pass working_dir through the tool execution context
2. **Replace `std::sync::Mutex` with `parking_lot::Mutex` in Brain** — Eliminates poison risk
3. **Move embedding computation outside memory lock** — Use pre-computed embedding pattern
4. **Replace `eprintln!` with `tracing`** — All 20+ instances
5. **Fix `agent/mod.rs:831` unwrap** — Return `Option<&ModelConfig>` or handle gracefully

### 11.2 Short-term Improvements (P1)

1. **Use `tokio::fs` or `spawn_blocking` for file I/O in async contexts** — Especially `refresh_context_segments()` and tool execute methods
2. **Increase `system_prefix_budget` to 15-20%** — Or make configurable via config.toml
3. **Implement `RecoveryAction::SwitchModel`** — Rebuild client with new model config on loop-level model failure
4. **Add memory index eviction** — Limit BM25/HNSW index size, evict oldest entries
5. **Add event log rotation** — Clean up old JSONL files beyond a configurable age/count
6. **Split `app/src-tauri/src/lib.rs`** — 1796 lines in one file is too large; extract command groups
7. **Split `runtime/run.rs`** — Extract `ContextRefresher`, `CompactStrategy`, `TurnExecutor`
8. **Add unit tests for `Run`** — Currently only tested via Agent integration
9. **Add unit tests for `MemoryManager`** — Critical system with no tests

### 11.3 Medium-term Improvements (P2)

1. **Container sandbox for bash tool** — Defense-in-depth beyond command pattern matching
2. **Streaming tool results** — Large tool results (e.g., `read_file` on a 10K-line file) could be streamed to the LLM as chunks
3. **Multi-model parallel inference** — For subagents, run multiple model calls in parallel on different models
4. **Configurable compression strategy** — Let users choose between chunked-drop (cache-friendly) and LLM-summarize (information-preserving)
5. **MCP server hot-reload** — Currently MCP servers are connected at startup; support runtime addition/removal
6. **Workflow visualizer** — DAG rendering in TUI/Tauri
7. **Permission policy hot-reload** — Already partially implemented (`update_from_config` is called per-turn), but could be more granular
8. **Embedding model warm-up** — Pre-load the embedding model on startup to avoid cold-start latency on first memory operation

### 11.4 Long-term Vision (P3)

1. **Distributed Runs** — Run Runs on different machines (currently single-process)
2. **Persistent HNSW index** — Serialize/deserialize HNSW to disk for fast restart
3. **Multi-agent orchestration** — Teams of agents with different specializations working together
4. **Self-improvement loop** — Reflector → skill drafts → skill review → skill activation
5. **Tool catalog streaming** — Stream tool definitions as a separate API field to avoid bloating the system prompt
6. **Context window prediction** — Predict when compaction will be needed and proactively compact during idle time

### 11.5 Tool Catalog Assessment

**Current tools (14):** read_file, write_file, edit, grep, glob, bash, webfetch, tavily_search, recall_memory (×2), archival_memory (×3), core_memory, todo (×2), skill (×3), subagent

**Missing tools to consider:**
- **`directory_tree`** — Recursive directory listing (currently users must chain `glob` + `read_file`)
- **`git_diff`** — Show uncommitted changes (currently users must use `bash` for this)
- **`git_commit`** — Stage and commit changes (currently users must use `bash`)
- **`http_request`** — Generic HTTP request (more flexible than `webfetch`)
- **`code_search`** — Semantic code search (more powerful than `grep`)
- **`file_watch`** — Watch for file changes (useful for long-running tasks)
- **`timer`** — Set a timer/reminder (useful for background tasks)

### 11.6 Agent Efficiency Assessment

**Token efficiency:**
- Tool catalog in system prompt: ~700 tokens for 14 tools — reasonable
- Context injection per turn: ~1000-3000 tokens (environment + memory + skills + plan) — acceptable
- Hygiene truncation: 16K char cap for incidental tools, 24K for read_file — good
- Compression: chunked-drop preferred, zero LLM cost — excellent

**Turn efficiency:**
- Parallel tool execution by default — good
- Sequential mode forced for bash and subagent — correct for safety
- Steering queue processed one-per-turn-boundary — prevents overwhelming LLM

**Context window utilization:**
- Auto-compact at 80% threshold — good
- Chunked-drop keeps 50% or 20 messages (whichever is smaller) — reasonable
- Recovery compaction at 60% target ratio — good fallback

---

## 12. Verdict

### Overall Assessment: **B+ (Strong architecture, needs hardening)**

The Agverse agent core demonstrates **exceptional systems design** — the 7-segment context engine with KV-cache-first thinking, the 5-stage compression pipeline with cache-aware chunked-drop, the layered permission system with sandbox enforcement, and the Brain→RunManager→Run execution architecture are all well-conceived and well-implemented.

The codebase would benefit from a **hardening sprint** focused on:

1. **Concurrency safety** — Fix the process-global CWD mutation, replace std::sync::Mutex with parking_lot::Mutex, move embedding computation outside locks
2. **Async correctness** — Replace blocking std::fs calls with tokio::fs or spawn_blocking
3. **Error resilience** — Remove production unwrap()s, implement SwitchModel recovery, replace eprintln! with tracing
4. **Test coverage** — Add unit tests for Run, MemoryManager, and tool execution paths
5. **Code organization** — Split the two 1700+ line files, remove legacy code

The architecture is sound enough to build upon. The implementation needs polish.

---

*End of Audit Report*