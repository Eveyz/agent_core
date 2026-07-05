I'll systematically read and analyze all the requested files. Let me start by exploring the directory structure and reading files in parallel batches.
Let me read the files in parallel batches, starting with several at once:
Now let me read the remaining files I haven't fully covered:
Now I have read all the requested files thoroughly. Here is my comprehensive analysis:

---

# Comprehensive Analysis of Core Modules

## 1. `session.rs` (1121 lines)

**Purpose:** Session persistence layer — saves/restores full conversation histories to SQLite. Sessions are "what I was doing" (resumable), distinct from Memory ("what I know").

**Key Types:**
- `SessionMeta` — metadata struct (id, title, summary, start/end time, message_count, cwd, model, tags, archived, parent_session_id, session_type, timing fields, mode)
- `Session` — full session = meta + messages + event_log
- `SessionManager` — owns a `Storage` backend, provides CRUD
- `SessionCounts` — total/active/archived counts
- `EventLogEntry` — turn_index, event_type, payload, started_at, ended_at
- `SubagentResultLike` trait — bridges subagent results to session save

**Notable Patterns:**
- `META_SELECT` as a shared SQL constant with `COALESCE` for backward-compatible column reads
- `row_to_meta` as a reusable row mapper
- Explicit deadlock avoidance comment in `resume()` — scoping the db lock before calling `get_event_log` (mutex non-reentrancy)
- `truncate_payload` recursively truncates JSON string values to 2000 chars, with special exemption for "assistant" event type
- `auto_title` extracts first 80 chars of first user message

**Code Quality Issues:**
- `display_line` slices `self.start_time` with `..self.start_time.len().min(16)` — byte slicing on a UTF-8 string, panics on multi-byte chars at boundary
- `save_subagent` ignores the `subagent_id` parameter for session ID (always creates new with `None`)
- `RoleExt::from_str` is a private trait — could use `FromStr` standard trait instead
- All `i32`/`i64` casts for SQLite integer columns are manual and repetitive

**Bugs/Concerns:**
- `display_line` byte-slice panic risk on non-ASCII timestamps (though RFC3339 is ASCII, the concern is structural)
- `save_full` doesn't update `summary`, `tags`, or `session_type` on existing session update — only message_count, timestamps, cwd, model
- `purge_archived` counts first, then deletes — race condition if sessions are archived concurrently (though SQLite serializes)
- No transaction wrapping in `save_full` — DELETE + INSERT messages are not atomic

---

## 2. `prompt.rs` (247 lines)

**Purpose:** Default prompt templates and legacy prompt assembly utilities for the Context Engine.

**Key Types:**
- `PromptBuilder` — legacy builder (identity, principles, core_memory, user_context)
- `PromptSection` — name, content, priority
- `PromptAssembler` — priority-sorted section assembly
- Constants: `DEFAULT_IDENTITY`, `DEFAULT_PRINCIPLES`, `DEFAULT_REACT_PROMPT`, `MEMORY_PROMPT_*`

**Notable Patterns:**
- Builder pattern with `with_*` methods
- Priority-based section sorting in `assemble()`
- `== Name ==` / `== End Name ==` delimiters for segment boundaries
- `memory_mode_prompt()` maps `MemoryMode` to static prompt strings

**Code Quality Issues:**
- `DEFAULT_REACT_PROMPT` is marked "Deprecated" but still present — dead code risk
- `PromptBuilder` and `PromptAssembler` are both legacy, creating maintenance burden
- `add_tools` in `PromptAssembler` just lists tool names as a string — not JSON schema, limiting usefulness

**Bugs/Concerns:**
- No validation that section names are unique in `PromptAssembler` — duplicate names produce duplicate sections
- `remove_section` removes ALL sections with matching name (correct but undocumented)

---

## 3. `subagent/mod.rs` (608 lines)

**Purpose:** Subagent execution — isolated agent loops with their own context, tools, and permissions. Supports per-agent memory injection/persistence (PLAN-0009).

**Key Types:**
- `ResultStrategy` — Auto/Full/Summary — controls how output is formatted for parent
- `SubagentConfig` — system_prompt, tools, max_iterations, max_context_tokens, model override, skills, permission_mode, memory_enabled, temperature, result_strategy
- `SubagentResult` — subagent_id, role_name, output (all text), last_text (final turn), iterations_used, success
- `Subagent` — owns client, context, registry, permission_policy, hook_registry, optional memory_store
- `SubagentManager` — registry of named SubagentConfigs

**Notable Patterns:**
- RAII `EventGuard` to guarantee `SubagentEnd` is emitted even on early return/panic
- Event remapping: `AgentEvent::ToolExecutionStart` → `SubagentToolStart` for the parent's event stream
- Strategy-specific system message injection before task
- `collect_stream` with `TokenAccumulator` and `ToolCallAccumulator` for streaming
- Per-agent memory: `inject_memory` before run, `persist_memory` after run

**Code Quality Issues:**
- `collect_stream` has massive code duplication — `TextDelta` and `ThinkingDelta` handlers are near-identical (each ~20 lines repeated)
- `flush_tokens` closure is defined but the inline flush code is duplicated instead of calling it
- `all_text` truncation at 1000 chars on max-iterations is arbitrary and may lose important data
- `SubagentManager` is very thin — just a HashMap wrapper with no lifecycle management

**Bugs/Concerns:**
- Each iteration creates a NEW `CancellationToken` (line 293) — this token is never linked to a parent, so cancelling the parent Run doesn't cancel subagent tools
- `persist_memory` stores `agent_id` as both the store key and agent_id — if multiple subagents share an agent_id, they overwrite each other's memories
- The `is_error` check (`result.starts_with("Error")`) is fragile — a tool returning "Error: ..." in success output would be misclassified
- `last_text` is assigned `text.clone()` but on max-iterations path, `last_text` may be stale from a previous iteration if the final turn produced no text

---

## 4. `agent/executor.rs` (482 lines)

**Purpose:** ToolOrchestrator — executes tool calls with permission checks, hooks, DAG-based scheduling, and cancellation support.

**Key Types:**
- `ToolOrchestrator<'a>` — registry, permission_policy, hook_registry, tool_execution_mode, cancel_token, approval_resolver, session_id

**Notable Patterns:**
- Two-phase execution: sequential preflight (permission + hooks) then DAG-scheduled execution
- Layered permission: Deny → Ask (with approval oneshot) → Allow
- `scoped_pattern` — approvals are scoped to exact command/path/host, not bare tool name
- `tokio::select!` for cancellation during approval wait
- `FuturesUnordered` for concurrent DAG node execution
- Tool-internal event forwarding via unbounded channel + `tokio::join!`

**Code Quality Issues:**
- Heavy use of `eprintln!` for debug logging (lines 71-80, 91-93, 124-128, 251-253) — should use `tracing`
- The approval flow has extensive duplicated cleanup code (resolver.remove vs global_pending_approvals)
- `execute_single_tool` injects `_session_id` into args — this modifies the tool's input without the tool's knowledge

**Bugs/Concerns:**
- **Duplicate id check** (lines 97 and 102) — `if response.id == Some(id)` appears twice consecutively in `request()`, the second is dead code (this is in `mcp/transport.rs` actually, but the executor has a similar pattern with the duplicate approval cleanup)
- `execute_single_tool` uses `tokio::join!(tool_fut, drain_fut)` but then only takes `.0` — the drain future is never properly completed if the tool future completes first (though `tokio::join!` does wait for both)
- The `cancel_token` in `run_node` is cloned and passed to `execute_single_tool`, but `execute_single_tool` also references `self.cancel_token` — redundant cancellation checks
- Post-tool hooks only fire for non-error results, but the `is_error` check is string-prefix based (`starts_with("Error")`)

---

## 5. `agent/scheduler.rs` (318 lines)

**Purpose:** DAG-based tool scheduler — builds a dependency graph from resource conflicts and runs independent calls concurrently.

**Key Types:**
- `SchedNode` — idx, tool_name, tool_call_id, args, mutations, reads
- `ResourceKey` — Path/BashProgram/Host
- `DepGraph` — dependents (adjacency list), indegree

**Notable Patterns:**
- `classify_resources` maps tool names to mutation/read resource keys
- `shares_mutable_resource` treats mutate-read as a conflict (conservative)
- Index-order tiebreak ensures DAG by construction (no cycle detection needed)
- `normalize_path` collapses `.`/`..` for consistent hashing
- `leading_program` extracts basename for coarse bash conflict grouping

**Code Quality Issues:**
- `classify_resources` uses a match on tool name strings — no registration mechanism, new tools must be manually added
- `normalize_path` doesn't handle symlinks or case-insensitive filesystems
- `host_of` URL parsing is naive — doesn't handle ports, auth, or query strings properly

**Bugs/Concerns:**
- `grep`/`glob` with no path defaults to `ResourceKey::Path("/")` — this conflicts with EVERY file mutation, serializing all grep/glob calls with any write
- `bash` is always classified as a mutation keyed by program — `git status` and `git diff` (reads) are mutations, serializing them unnecessarily with other git commands
- Unknown tools with a `path` field are treated as mutations — conservative but may over-serialize
- `shares_mutable_resource` uses `Vec::contains` (O(n)) — fine for small batches but could be O(n²) for large tool call batches

---

## 6. `runtime/brain.rs` (520 lines)

**Purpose:** Brain — reusable, stateless shared state across all Runs. Holds config, memory, skills, reflector, todo, hooks.

**Key Types:**
- `Brain` (Clone via Arc) — config, memory (Option<Arc<Mutex<MemoryManager>>>), reflection_daemon, skill_manager, todo_list, reflector, current_model_name, current_mode, hook_registry

**Notable Patterns:**
- `from_config` builds all subsystems with graceful degradation (memory/skills fail → None, not error)
- `build_client` constructs fallback chain up to 3 levels deep
- `build_tool_registry` filters tools by mode BEFORE registering subagent tools
- BM25 and HNSW indexes built from existing recall records on startup
- `build_reflection_daemon` only spawns in Deep memory mode

**Code Quality Issues:**
- Mixes `parking_lot::Mutex` and `std::sync::Mutex` — `current_mode` uses `StdMutex` to satisfy `#[derive(Clone)]` but `memory`/`skill_manager` use `parking_lot::Mutex`
- `build_skill_manager` takes `_config` but doesn't use it — placeholder for future config-driven skill dirs
- `update_config` replaces the entire config but doesn't rebuild memory/skills/reflector — stale subsystems after config change

**Bugs/Concerns:**
- `switch_model` takes `&mut self` but `Brain` is shared via `Arc<Brain>` — callers must use `Arc::make_mut` (which requires full clone). The `RunManager::switch_model` does this correctly but it's a footgun.
- `build_bm25_index` and `build_hnsw_index` acquire `storage_conn()` which returns a Mutex guard — if the MemoryManager is dropped while the guard is held, undefined behavior (though Rust's lifetime system should prevent this)
- `build_client_for` doesn't build fallback chains — side-channel queries have no resilience
- `identity_text` and `principles_text` always use `DEFAULT_IDENTITY`/`DEFAULT_PRINCIPLES` — model-specific system_prompt only overrides identity, not principles

---

## 7. `runtime/manager.rs` (690 lines)

**Purpose:** RunManager — creates and tracks Runs, routes commands, manages lifecycle. Primary frontend/CLI interface.

**Key Types:**
- `RunHandle` — id, session_id, cmd_tx, event_tx, join_handle, state (Arc<RwLock<RunState>>), approval_resolver, cancel_token, context_snapshot
- `RunManager` — brain (Arc<Brain>), runs (Mutex<HashMap<RunId, RunHandle>>)

**Notable Patterns:**
- Two-phase cancellation: CancellationToken (fast) + RunCommand::Cancel (slow, proper state transition)
- Approval bypass: `resolve_approval` goes directly through `ApprovalResolver`, not command channel — avoids actor deadlock
- Logging subscriber task + state mirror task spawned alongside Run task
- Diff Observer: checks previous run's files for manual edits before starting new run
- Offline reflection: analyzes event log after Run completes, auto-applies safe suggestions
- Worktree isolation: `create_run_in_worktree` creates git worktree + Run with working_dir

**Code Quality Issues:**
- `create_run_with_workdir` is very long (~200 lines) — should be decomposed
- The spawned closure captures many clones (`event_tx_clone`, `log_run_id`, `seq_for_reflect`, etc.) — could use a struct
- `reap_completed` uses `runs.retain` which doesn't call Drop on the removed RunHandle — join_handle is dropped without being awaited (task may leak)
- `runs_mut` exposes the internal mutex guard — breaks encapsulation

**Bugs/Concerns:**
- **Potential deadlock**: `resolve_approval` locks `self.runs` then calls `handle.approval_resolver.resolve()` which locks the approval map. If another path locks these in reverse order, deadlock. Currently safe but fragile.
- `create_run_with_workdir` spawns the Run task which holds a reference to `brain_for_reflect` — if the Brain is updated via `update_config` while a Run is active, the Run sees stale config
- The Diff Observer check at line 246-266 reads the runs directory synchronously in `create_run_with_workdir` — blocks Run creation on disk I/O
- `cancel_all` queues `RunCommand::Cancel` via `try_send` but ignores the result — if the channel is full, the cancel command is silently lost (though the CancellationToken already fired)
- No limit on number of concurrent Runs — could exhaust resources

---

## 8. `runtime/supervisor.rs` (376 lines)

**Purpose:** ProcessSupervisor — owns all child processes spawned by a Run. Uses process groups (Unix) for proper tree kills.

**Key Types:**
- `SupervisedChild` — child (Option<Child>), pgid, label, spawned_at
- `ProcessSupervisor` — HashMap<String, SupervisedChild>

**Notable Patterns:**
- Process group isolation: `process_group(0)` → `setpgid(0, 0)` makes child a group leader
- `killpg(pgid, SIGKILL)` kills entire process tree including grandchildren
- RAII Drop: kills all children on drop
- `kill_on_drop(false)` — manages lifecycle explicitly
- `try_wait()` in `kill_child` to reap zombies without async

**Code Quality Issues:**
- `truncate_label` uses `floor_char_boundary` (unstable Rust feature?) — actually it's a nightly-only method, may not compile on stable
- `kill_child` calls `try_wait()` which is non-blocking — in Drop context, zombies may not be fully reaped
- `derive_pgid` assumes `pgid == pid` — true for `process_group(0)` but not documented in the function

**Bugs/Concerns:**
- **Windows support is stubbed**: `apply_process_group` and `derive_pgid` are no-ops on non-Unix — child processes will leak on Windows
- `kill_child` uses `unsafe { libc::killpg(pgid, libc::SIGKILL) }` — no error checking, silently fails if pgid is invalid
- `spawn_bash` uses `sh -c` — assumes POSIX shell, won't work on Windows (cmd.exe needed)
- Drop calls `kill_all` which calls `kill_child` for each — if any `kill_child` panics, remaining children leak
- No stdout/stderr capture in `spawn_bash` beyond piping — output must be read separately or it blocks

---

## 9. `runtime/approval.rs` (129 lines)

**Purpose:** Per-Run approval channel — replaces global pending approvals map. Each Run owns its own approval resolver.

**Key Types:**
- `PendingApprovalMap` — HashMap<String, oneshot::Sender<ApprovalChoice>>
- `ApprovalResolver` — Arc<Mutex<PendingApprovalMap>> (Clone)

**Notable Patterns:**
- Simple, clean design: insert/remove/resolve/clear
- `clear()` drops all senders → receivers get `RecvError` → executor treats as denial
- Clone shares state via Arc

**Code Quality Issues:**
- Uses `parking_lot::Mutex` while the executor also uses `parking_lot::Mutex` for hooks — consistent
- `is_empty()` calls `len()` which locks — minor overhead, could check directly

**Bugs/Concerns:**
- No timeout mechanism — a tool waiting for approval blocks forever if the user never responds (though the executor has a cancellation select)
- No deduplication — same prompt_id can be inserted twice, overwriting the first sender (first receiver never gets a response)

---

## 10. `runtime/event.rs` (622 lines)

**Purpose:** RunEvent enum + Envelope wrapper. Events emitted by a Run to frontend/CLI/event log.

**Key Types:**
- `RunEvent` — ~30 variants covering lifecycle, turns, model, messages, tools, approvals, steering, subagents, processes, planning, goals, cache, mode
- `Envelope` — seq, event_id, run_id, turn_id, parent_call_id, ts, event (flattened)
- `CacheMetrics` — cumulative cache hit/miss tracking
- `TodoItemPayload` — lightweight todo for frontend

**Notable Patterns:**
- `#[serde(flatten)]` on Envelope.event — JSON has event tag alongside envelope fields
- `from_agent_event` / `to_agent_event` — bidirectional legacy bridge
- `seq` is monotonic per-Run (shared AtomicU64)
- `event_id` is UUID stable across transport/log/replay

**Code Quality Issues:**
- `from_agent_event` has a massive match with many `return None` arms — hard to verify completeness
- `to_agent_event` has a catch-all `_ => return None` — silently drops events like `RunPaused`, `RunResumed`, `ContextCompacted`, `CacheInfo`, `CacheSummary`, `ModeChanged`, all steering events, all process events, `TodoUpdated`, `GoalSet`, `GoalCompleted`
- `ToolUpdate` in `to_agent_event` sets `tool_name: String::new()` — empty string is a poor sentinel
- `RunEvent::RunCompleted` maps to `AgentEvent::AgentEnd { messages: vec![] }` — loses final_text

**Bugs/Concerns:**
- The `ts` field uses `#[serde(default = "chrono::Utc::now")]` — deserialized events get the current time, not the original timestamp, if `ts` is missing from JSON
- `CacheMetrics::record` uses `f64` for cumulative_hit_rate — floating point accumulation errors over many turns

---

## 11. `runtime/event_log.rs` (298 lines)

**Purpose:** Append-only JSONL persistence for Run events. Enables replay, fork, audit, and reflector analysis.

**Key Types:**
- `EventLog` — run_id, path, entries (in-memory Vec), writable

**Notable Patterns:**
- Best-effort persistence: IO failures logged but never block
- In-memory copy serves as backup + fast query
- `replay_since` for frontend resync after broadcast lag
- `list_runs` scans directory for .jsonl files

**Code Quality Issues:**
- `append` opens the file on EVERY write (line 59) — no buffered writer, terrible performance for high-frequency events
- `entries` Vec grows unbounded — memory leak for long-running Runs
- `load` and `replay_since` read the entire file into memory — won't scale for large traces
- `list_runs` returns sorted run IDs but sorting is lexicographic, not chronological

**Bugs/Concerns:**
- **Performance**: opening/flushing/closing the file per event is extremely I/O intensive
- `replay_since` silently skips unparseable lines (`Err(_) => continue`) — data loss is hidden
- No file locking — concurrent Runs writing to the same directory are fine (different files), but concurrent readers of the same file could see partial lines
- No rotation or size limit — long-running Runs could fill disk

---

## 12. `runtime/guard.rs` (96 lines)

**Purpose:** RAII EventGuard — guarantees every Start event is paired with an End event, even on early return or panic.

**Key Types:**
- `EventGuard<E>` — completed (bool), on_incomplete (Arc<dyn Fn() -> E>)

**Notable Patterns:**
- Closure-based: `new(on_incomplete)` creates the guard, `complete()` marks success
- Drop fires `on_incomplete` if not completed
- `Arc<dyn Fn>` allows the guard to be Clone-able in theory (though EventGuard itself isn't Clone)

**Code Quality Issues:**
- `on_incomplete` return type `E` is never used — the closure returns a value but `Drop` discards it. The type parameter is unnecessary.
- Could be simplified to `EventGuard { completed: bool, on_incomplete: Arc<dyn Fn() + Send + Sync> }`

**Bugs/Concerns:**
- If `on_incomplete` panics, Drop aborts — could crash the agent. Should use `catch_unwind`.
- Not `Send` unless `E: Send` — but `E` is never used, so this constraint is pointless

---

## 13. `runtime/state.rs` (100 lines)

**Purpose:** Run lifecycle state machine — 8 states with terminal/blocked/alive helpers.

**Key Types:**
- `RunState` — Created, Running, AwaitingApproval, AwaitingInput, Paused, Completed, Cancelled, Failed

**Notable Patterns:**
- `is_terminal`, `is_blocked`, `is_alive` convenience methods
- `Display` impl for human-readable strings
- `Default` is `Created`

**Code Quality Issues:**
- No transition validation in this file — transitions are enforced elsewhere (Run)
- Could use a `transition(from, to) -> Result` method for centralized validation

**Bugs/Concerns:**
- None — simple, correct enum

---

## 14. `runtime/command.rs` (109 lines)

**Purpose:** Commands sent TO a Run (from frontend/CLI/RunManager).

**Key Types:**
- `RunCommand` — Start, Pause, Resume, Cancel, Steer, CancelSteer, Approve, Answer, SetMode
- `SteerEntry` — id, message, raw_text, timestamp

**Notable Patterns:**
- `#[serde(tag = "type", rename_all = "snake_case")]` for JSON discriminant
- `SteerEntry::from_text` convenience constructor with UUID + timestamp

**Code Quality Issues:**
- `steer_message` is a static method that just calls `Message::user` — unnecessary indirection
- `SetMode` on the command channel is redundant with `RunManager::set_mode` (which goes through Brain)

**Bugs/Concerns:**
- `Steer` command uses `message: String` but `SteerEntry` uses `message: Message` — inconsistency between the command and the internal representation

---

## 15. `hooks/mod.rs` (415 lines)

**Purpose:** Hook system — pre/post tool use, session/turn lifecycle, model call interception.

**Key Types:**
- `HookEvent` — PreToolUse, PostToolUse, SessionStart/End, TurnStart/End, BeforeModel, AfterModel
- `HookAction` — Continue, Veto, ModifyInput, ModifyOutput, SkipModel
- `Hook` trait — name() + handle(event)
- `HookRegistry` — Vec<Box<dyn Hook>>
- `PreToolResult` — Proceed(Value) or Veto(String)
- `LoggingHook` — simple eprintln hook

**Notable Patterns:**
- Chained input modification: each hook can modify the input, next hook sees modified input
- `SkipModel` for testing/caching/short-circuiting LLM calls
- `fire_pre_tool_use` returns `PreToolResult` (veto short-circuits)

**Code Quality Issues:**
- `fire_pre_tool_use` ignores `ModifyOutput` and `SkipModel` actions (lines 96-97) — silently swallowed
- `fire_post_tool_use` ignores all actions except `ModifyOutput` (line 121 `_ => {}`)
- No hook ordering guarantee beyond insertion order
- `LoggingHook` uses `eprintln!` — should use `tracing`
- `HookRegistry` doesn't implement `Default` explicitly (derives via `new()`)

**Bugs/Concerns:**
- A hook returning `SkipModel` during `fire_pre_tool_use` is silently ignored — the model call proceeds anyway
- Hooks are `Box<dyn Hook>` — no way to remove a specific hook by name (only `remove_section` in PromptAssembler, not here)
- Hook panics propagate to the caller — could crash the agent loop

---

## 16. `hygiene.rs` (227 lines)

**Purpose:** History hygiene — truncates oversized tool results and long tool args before sending to model API.

**Key Types:**
- (module-level functions, no structs)
- `TOOL_ARG_MAX_CHARS = 200`

**Notable Patterns:**
- Delegates tool-result truncation to `policy::truncate_content` (shared with compressor)
- Tool args truncated to `[args truncated: N bytes]` placeholder
- `sanitize` returns count of modified messages

**Code Quality Issues:**
- `truncate_tool_result` clones the content string unnecessarily (line 44)
- `sanitize` counts both truncations separately — a message modified by both functions counts as 2

**Bugs/Concerns:**
- `truncate_tool_args` checks `tc.function.arguments.len()` — this is byte length, not char length. A 200-byte multi-byte string would be truncated even if it's <200 chars.
- Truncated args replace the ENTIRE arguments string with a placeholder — the model loses all argument context, making it hard to retry

---

## 17. `hygiene/policy.rs` (229 lines)

**Purpose:** Shared tool-result truncation policy — single source of truth for L2 (hygiene) and L3 (compressor).

**Key Types:**
- `TruncationKind` — Instruction, ActivelyRead, Incidental
- Constants: `INCIDENTAL_MAX_CHARS = 16_000`, `ACTIVE_READ_MAX_CHARS = 24_000`, `SUBAGENT_RESULT_MAX_CHARS = 64_000`

**Notable Patterns:**
- Three-tier classification: Instruction (never cut), ActivelyRead (char cap only), Incidental (head/tail/signal)
- Signal line preservation: error/warning/failed/denied keywords
- `floor_char_boundary` for UTF-8-safe truncation

**Code Quality Issues:**
- `floor_char_boundary` is a manual implementation — Rust's std `str::floor_char_boundary` is available since 1.73
- `INSTRUCTION_TOOLS` and `ACTIVE_READ_TOOLS` are hardcoded arrays — no registration mechanism
- `truncate_head_tail` recomputes `lines.len()` comparison — minor

**Bugs/Concerns:**
- `classify` uses exact string matching — `skill_load` is instruction, but `skill_reload` or `skill_deactivate` are incidental
- `ACTIVE_READ_TOOLS` includes `"subagent"` and `"subagents"` but not `"task_execute"` which also spawns subagents
- Signal keywords are case-sensitive after `to_lowercase()` — correct, but `SIGNAL_KEYWORDS` array is lowercase, so it works

---

## 18. `skills/mod.rs` (647 lines)

**Purpose:** SkillManager — loads SKILL.md files from search directories, manages activation/deactivation, builds context/catalog.

**Key Types:**
- `SkillManager` — search_dirs, manifests (Vec<LoadedSkill>), active_skills (HashSet)
- `LoadedSkill` — manifest, source_dir
- `SkillLoader` — backward-compat alias for SkillManager

**Notable Patterns:**
- `with_defaults` scans multiple standard directories (cwd, home, .agent, .claude, .agverse, plugins)
- `collect_skills_dirs` recursively finds directories containing SKILL.md children
- Deduplication by skill name (first directory wins)
- Priority-sorted manifests (descending)
- `check_triggers` skips already-active skills
- `build_catalog` lists ALL skills with [ACTIVE] markers
- `build_active_context` loads full content for active skills only

**Code Quality Issues:**
- Tests use `/tmp/` paths directly — not sandboxed, may conflict with concurrent test runs
- `find_by_trigger` is separate from `check_triggers` — duplicated trigger-matching logic
- `load_content` reads from disk on every call — no caching
- `build_active_context` calls `load_content` for each active skill — N disk reads per turn

**Bugs/Concerns:**
- `scan` clears manifests before rescanning — if scan fails midway, skills are lost
- `activate` returns true even if the skill is already active (idempotent but undocumented)
- No hot-reload notification — `scan` replaces manifests but active_skills may reference skills that no longer exist
- `collect_skills_dirs` doesn't follow symlinks — may miss skills in symlinked directories

---

## 19. `skills/manifest.rs` (418 lines)

**Purpose:** SkillManifest — parses SKILL.md frontmatter (YAML-like) without serde_yaml dependency.

**Key Types:**
- `SkillManifest` — name, description, version, triggers, tags, read_when, requires, provides_tools, priority, content_path

**Notable Patterns:**
- Custom YAML frontmatter parser (`parse_yaml_frontmatter`) — handles inline lists `[a, b, c]` and block lists (`- item`)
- `from_markdown` splits on `---` delimiters
- `read_body` extracts content after frontmatter
- `catalog_line` produces one-line summary for system prompt
- `matches_trigger` checks triggers, read_when, and name (all case-insensitive)

**Code Quality Issues:**
- Custom YAML parser is fragile — doesn't handle nested objects, multiline strings, anchors, or complex YAML
- `parse_inline_list` doesn't handle escaped quotes inside items
- `from_markdown` uses `content[3..]` to skip `---` — byte slicing, panics if content is <3 bytes
- `parse_yaml_frontmatter` uses `line.strip_prefix("triggers:")` etc. — a field named `triggers_extra:` would match

**Bugs/Concerns:**
- The frontmatter parser doesn't handle `---` appearing inside a string value — premature split
- `read_body` returns the full content if no closing `---` is found — may return frontmatter as body
- No validation that `name` is non-empty or a valid identifier
- `priority` defaults to 0 if parse fails — silently low priority

---

## 20. `tasks/mod.rs` (1014 lines)

**Purpose:** Task DAG tools — task_create, task_batch_create, task_update, task_list, task_get, task_plan, task_ready, task_execute. Tools for the model to manage multi-step plans.

**Key Types:**
- 8 tool structs (TaskCreateTool, TaskBatchCreateTool, etc.)
- `detect_cycle`, `build_dependency_context`, `should_use_subagent`, `topological_sort` helper functions

**Notable Patterns:**
- `task_batch_create` resolves local_ids to UUIDs within a batch — allows referencing dependencies before they're created
- `task_execute` uses `should_use_subagent` heuristic to decide inline vs subagent
- `build_dependency_context` injects dependency results into the task prompt
- `topological_sort` uses DFS with visited/visiting sets for cycle detection

**Code Quality Issues:**
- `detect_cycle` is defined but never called from `task_create` (the comment explains why: new UUIDs can't create cycles)
- `task_execute` has a logic error in the readiness check: `*task.status() != TaskStatus::Ready && *task.status() == TaskStatus::Pending` — this only bails for Pending non-Ready tasks; Blocked, InProgress, Completed are handled by subsequent checks, but the logic is confusing
- `should_use_subagent` keyword list includes generic words like "read", "check", "find" — high false positive rate
- `topological_sort` silently skips cycles (`return` instead of error) — may produce incomplete orderings

**Bugs/Concerns:**
- `task_execute` inline path (line 583) just returns `"[Task '{}' - inline] Goal: {}"` — doesn't actually accomplish the task, just marks it completed with a placeholder
- `task_execute` holds the board lock while checking readiness, releases it, then re-acquires to mark in-progress — race condition: another execution could change the task state between locks
- Subagent created in `task_execute` has no event sender — no streaming events for the parent
- `should_use_subagent` checks `parallel_count >= 1` — if ANY other task is ready, ALL tasks use subagents, even trivial ones

---

## 21. `tasks/board.rs` (410 lines)

**Purpose:** TaskBoard — in-memory + JSONL-persisted task DAG with dependency resolution.

**Key Types:**
- `TaskStatus` — Pending, Ready, InProgress, Blocked, Completed, Failed
- `TaskRecord` — id, title, goal, status, blocked_by, assigned_to, result, timestamps
- `TaskBoard` — tasks HashMap, optional storage_path

**Notable Patterns:**
- `update_dependents` propagates completion/failure: failed deps → Blocked, all deps met → Ready
- `persist` writes all tasks as JSONL (one JSON per line), sorted by created_at
- `summary` produces ASCII-art board with status icons `[ ]`, `[>]`, `[~]`, `[!]`, `[x]`, `[-]`

**Code Quality Issues:**
- `truncate` in `summary` uses byte slicing `&s[..max_len]` — panics on multi-byte char boundary
- `persist` is called after every mutation — could batch writes
- `with_storage` reads the entire file synchronously on construction — blocks startup
- `update` sets `assigned_to = Some("agent")` for InProgress — hardcoded string, not a real agent ID

**Bugs/Concerns:**
- `truncate` byte-slice panic risk (line 327)
- `update_dependents` iterates ALL tasks every time ANY task completes — O(n) per completion, O(n²) for batch completion
- `persist` uses `std::fs::write` which truncates and rewrites the entire file — data loss if the process crashes during write
- No file locking — concurrent TaskBoards on the same file will corrupt data

---

## 22. `workflow/mod.rs` (28 lines)

**Purpose:** Module root — re-exports workflow types.

**Key Types:** Re-exports from context, definition, executor, planner, trust, validate.

**Code Quality:** Clean, well-organized.

---

## 23. `workflow/context.rs` (565 lines)

**Purpose:** WorkflowContext — structured JSON state passed between nodes. Includes LangGraph-style conditional routing.

**Key Types:**
- `WorkflowContext` — node_outputs (RwLock<HashMap>), shared (RwLock<Value>), input
- `DataMapping` — pass_through, source_field, target_field
- `RouterConfig` — rules (Vec<RouterRule>), default targets
- `RouterRule` — condition (ConditionExpr), targets
- `ConditionExpr` — field, op, value

**Notable Patterns:**
- `resolve_input` merges upstream outputs via data mapping rules
- `RouterConfig::route` returns first matching rule's targets, or default
- `ConditionExpr::evaluate` supports ==, !=, >, >=, <, <=, contains
- `get_by_path` supports dot-paths including `.length` for arrays
- `SharedContext = Arc<WorkflowContext>` type alias

**Code Quality Issues:**
- `ConditionExpr` with empty field returns `true` — surprising default
- Unknown operator defaults to `true` with a warning — should probably default to `false`
- `get_by_path` only supports `.length` as a special property — no `.size`, `.first`, etc.
- `num_cmp` silently returns `false` if either value isn't a number — could be surprising

**Bugs/Concerns:**
- `resolve_input` uses `edge.label` if non-empty, else `edge.source_node_id` as the key — if two edges have the same label, the second overwrites the first
- `update_shared` silently does nothing if shared state isn't a JSON object
- No validation that router targets reference existing nodes

---

## 24. `workflow/definition.rs` (712 lines)

**Purpose:** Workflow, node, edge definitions + SQLite CRUD operations.

**Key Types:**
- `NodeType` — Input, Output, Agent, Transform, HumanApproval
- `NodeDef` — id, workflow_id, node_type, label, agent_id, config, position
- `EdgeDef` — id, source/target_node_id, handles, label, condition, data_mapping
- `TrustMode` — Inherit, Trusted, Readonly
- `OnNodeFailure` — Abort, Continue, Skip
- `WorkflowDef` — id, name, description, input_schema, trust_mode, max_concurrent, on_node_failure, config, nodes, edges
- `WorkflowRun`, `WorkflowRunNodeResult` — execution records

**Notable Patterns:**
- Transaction-based CRUD: `create` and `save` use transactions for atomicity
- `save` (upsert) deletes all nodes/edges then re-inserts — simple but destructive
- `from_str` / `as_str` for enum serialization (custom, not serde derive)
- `default_data_mapping` is `{"pass_through": true}`

**Code Quality Issues:**
- `from_str` methods use `_ => Self::Input` as default — typos silently become Input nodes
- `list` returns WorkflowDefs without nodes/edges — partial data, could confuse callers
- `save` deletes and re-inserts all nodes/edges even if only one changed — inefficient for large workflows
- No validation in `create` or `save` — invalid workflows (cycles, missing agents) can be persisted

**Bugs/Concerns:**
- `get` uses `.context("workflow not found")` which converts the SQL error — if the query fails for other reasons (e.g., column mismatch), the error message is misleading
- `create_run` inserts with `output: '{}'` and `error: ''` — empty strings as sentinels
- `record_node_result` doesn't use a transaction — partial writes if the process crashes

---

## 25. `workflow/executor.rs` (567 lines)

**Purpose:** WorkflowExecutor — runs a planned DAG stage-by-stage with parallel node execution, routing, and per-node result recording.

**Key Types:**
- `WorkflowExecutor` — storage, brain
- `WorkflowRunResult` — run_id, status, output, error, token totals

**Notable Patterns:**
- Semaphore-bounded parallel execution within stages
- `apply_router` marks non-targeted downstream nodes as skipped
- `spawn_blocking` for all SQLite operations (non-blocking async)
- Per-node token tracking (V1: hardcoded to 0,0)
- `emit` helper swallows send errors

**Code Quality Issues:**
- `execute_agent_node` uses `_cancel_token` and `_run_id` (prefixed with underscore) — cancel token is ignored, subagent can't be cancelled
- `execute_node` for `HumanApproval` auto-approves — not actually human approval
- `execute_node` for `Transform` is pass-through with optional field extraction — very limited
- Token tracking is V1 stub (always 0,0)

**Bugs/Concerns:**
- **Subagent cancellation not wired**: `execute_agent_node` receives `cancel_token` but prefixes it with `_` — the subagent runs to completion even if the workflow is cancelled
- **No deadlock risk with semaphore**: `semaphore.acquire()` returns a permit that is dropped at end of scope — correct
- `apply_router` adds ALL non-targeted downstream nodes to `skipped` — but if a later node's router re-enables them, there's no un-skip mechanism
- `execute` awaits handles sequentially (`for handle in handles { handle.await }`) — nodes within a stage are spawned in parallel but awaited sequentially, which is correct but could miss early failures
- `build_agent_memory_store` creates a new EmbeddingModel per agent node — expensive if multiple agent nodes need memory

---

## 26. `workflow/planner.rs` (201 lines)

**Purpose:** Workflow planner — Kahn's algorithm for topological ordering grouped into parallel stages.

**Key Types:**
- `Stage` — nodes (Vec<String>)
- `ExecutionPlan` — stages (Vec<Stage>)

**Notable Patterns:**
- Edges referencing non-existent nodes are silently skipped (frontend may have stale edges)
- Cycle detection via Kahn's: empty stage with remaining nodes = cycle
- `saturating_sub` on indegree to prevent underflow

**Code Quality Issues:**
- `incoming_edges` and `outgoing_edges` are O(n) scans — fine for small graphs
- `node_ids` iterates all stages — could cache

**Bugs/Concerns:**
- Self-loops (edge from A to A) are counted as a dependency — A would never have indegree 0, causing a false cycle detection
- Duplicate edges (two edges from A to B) would double-count indegree — B would never become ready

---

## 27. `workflow/trust.rs` (110 lines)

**Purpose:** TrustMode — overrides per-agent permission posture at the workflow level.

**Key Types:** `TrustMode` impl block with `build_permission_config`.

**Notable Patterns:**
- Trusted → Yolo + Destructive auto-allow
- Readonly → Paranoid + ReadOnly auto-allow
- Inherit → keep agent's own config

**Code Quality Issues:**
- Clean, well-tested.

**Bugs/Concerns:**
- Trusted mode sets `auto_allow_up_to = Some(DangerLevel::Destructive)` — this auto-allows destructive operations without user approval, which could be dangerous in automated workflows
- Readonly mode sets `PermissionMode::Paranoid` — this is correct but Paranoid may prompt for every read operation, which is annoying for automated workflows

---

## 28. `workflow/validate.rs` (258 lines)

**Purpose:** Workflow validation — checks for cycles, orphan nodes, missing agents, missing I/O nodes, dangling edges.

**Key Types:**
- `ValidationIssue` — severity, code, message, node_ids
- `Severity` — Error, Warning
- `ValidationResult` — valid, issues

**Notable Patterns:**
- Delegates cycle detection to `planner::plan`
- Orphan nodes are warnings (not errors) unless only 1 node exists
- Missing input/output nodes are warnings
- `valid = !issues.any(error)`

**Code Quality Issues:**
- Clean, well-structured, well-tested.

**Bugs/Concerns:**
- Orphan node check allows single-node workflows to have no edges — but a single Input node with no edges is probably useless
- No check for multiple Input or Output nodes (could produce ambiguous behavior)

---

## 29. `mcp/mod.rs` (426 lines)

**Purpose:** MCP (Model Context Protocol) client — connects to MCP servers over stdio/SSE, discovers tools, exposes them as native agent tools.

**Key Types:**
- `McpConfig` — servers (Vec<McpServerConfig>)
- `McpServerConfig` — name, transport, command, args, url, env, enabled
- `McpToolDef` — server, name, description, parameters
- `McpClientManager` — servers, connections (HashMap<String, McpConnection>)
- `Transport` — enum(Stdio, Sse)

**Notable Patterns:**
- `connect_all` drains servers and connects sequentially, collecting errors per-server
- Tool naming: `mcp__<server>__<tool>` to avoid conflicts
- `connect_one` does initialize handshake → tools/list discovery
- Transport dispatch functions (`transport_request`, `transport_notify`, etc.)

**Code Quality Issues:**
- `connect_all` clears `self.servers` after draining — if connection fails, server config is lost
- `Drop for McpClientManager` is a no-op comment — relies on `kill_on_drop` but doesn't call `shutdown_all`
- `eprintln!` for logging (lines 188, 192)

**Bugs/Concerns:**
- `connect_all` is sequential — slow if many servers are configured; could parallelize
- `call_tool` checks `transport_is_alive` before each call — but the check is racy (process could die between check and call)
- `Drop` doesn't call `shutdown_all` — stdio child processes are killed by `kill_on_drop` but SSE connections aren't cleaned up
- No reconnection logic — if a server dies, all its tools fail permanently

---

## 30. `mcp/protocol.rs` (293 lines)

**Purpose:** JSON-RPC 2.0 protocol types for MCP communication.

**Key Types:**
- `JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`
- `InitializeParams/Result`, `ClientCapabilities`, `ServerCapabilities`
- `ToolsListResult`, `ToolDef`, `ToolCallParams`, `ToolCallResult`
- `ContentItem` — Text, Image, Resource

**Notable Patterns:**
- Standard JSON-RPC 2.0 with `jsonrpc: "2.0"` field
- `#[serde(rename = "camelCase")]` for MCP's camelCase JSON conventions
- `into_result()` converts error responses to anyhow errors
- `ContentItem` is tagged enum (`#[serde(tag = "type")]`)

**Code Quality Issues:**
- Clean, well-structured, well-tested.

**Bugs/Concerns:**
- `JsonRpcResponse.id` is `Option<u64>` — some servers use string IDs; this would fail to deserialize
- No support for JSON-RPC batch requests

---

## 31. `mcp/transport.rs` (160 lines)

**Purpose:** StdioTransport — spawns a process and communicates via line-delimited JSON-RPC.

**Key Types:** `StdioTransport` — child, reader, writer, next_id (all Mutex).

**Notable Patterns:**
- `kill_on_drop(true)` for child process
- Request/response matching by ID
- Notification (no-ID) messages skipped during response scan
- `max_attempts = 100` safety limit

**Code Quality Issues:**
- `request` holds the reader lock for the entire response scan loop — concurrent requests would serialize
- `max_attempts = 100` is arbitrary — if 100 notifications arrive before the response, the request fails

**Bugs/Concerns:**
- **Duplicate check**: lines 97 and 102 both check `if response.id == Some(id)` — the second is dead code (copy-paste error)
- **Concurrent request deadlock**: `request` locks writer, then locks reader — if two concurrent requests interleave, they could deadlock (though the locks are scoped)
- **No timeout**: `read_line` blocks forever if the server never responds
- **Notification handling**: notifications are read and discarded, but if a notification has the same ID as a pending request (impossible by protocol, but a buggy server could do it), it would be misinterpreted

---

## 32. `mcp/sse.rs` (247 lines)

**Purpose:** SSE transport for remote MCP servers — HTTP/SSE hybrid.

**Key Types:** `SseTransport` — base_url, post_url, next_id, client.

**Notable Patterns:**
- `connect` discovers POST URL from first SSE `endpoint` event
- Relative URL resolution from base_url
- `request` sends POST and expects synchronous JSON-RPC response in body (hybrid mode)
- 100KB safety limit for SSE endpoint discovery

**Code Quality Issues:**
- `connect` drops the SSE stream after finding the endpoint — the stream is never used for receiving responses (hybrid mode, not true SSE)
- `is_connected` just checks if post_url is set — doesn't verify the connection is alive
- `shutdown` is a no-op

**Bugs/Concerns:**
- **Not true SSE**: the `request` method uses POST and expects response in body, but the SSE stream (opened in `connect`) is abandoned — this won't work with MCP servers that only return responses via SSE
- **No reconnection**: if the POST URL becomes invalid, all subsequent requests fail
- **URL resolution** for relative paths is manual and fragile — doesn't handle all edge cases (ports, query strings, fragments)
- `parse_sse_endpoint` only checks for `event: endpoint` — other event types are ignored

---

## 33. `mcp/channel.rs` (57 lines)

**Purpose:** McpChannel — lightweight handle to a specific MCP server. Backward-compat wrapper.

**Key Types:** `McpChannel` — server, manager (Arc<Mutex<McpClientManager>>).

**Code Quality:** Clean, thin wrapper. No issues.

**Bugs/Concerns:** `invoke` locks the manager for the entire call — blocks other channels during execution.

---

## 34. `mcp/tool.rs` (85 lines)

**Purpose:** Bridge: register MCP tools into the agent's ToolRegistry.

**Key Types:** `McpTool` — qualified_name, description, parameters, server, tool_name, manager.

**Notable Patterns:**
- `register_all` uses `try_lock` — non-blocking, silently skips if locked
- Tool name: `mcp__<server>__<tool>`

**Code Quality Issues:**
- `register_all` uses `try_lock` and silently returns if locked — tools won't be registered if the manager is in use

**Bugs/Concerns:**
- `execute` locks the manager for the entire call — serializes all MCP tool invocations across all servers
- No error handling for unknown server/tool — relies on `call_tool` to error

---

## 35. `reflector/mod.rs` (522 lines)

**Purpose:** Offline reflection framework — reads execution traces and produces improvement suggestions with strict safety model.

**Key Types:**
- `Reflector` — skills_dir, max_suggestions
- `TraceRecord` — ts, event (raw JSON Value)
- `ToolEndSnapshot` — tool_name, is_error, result

**Notable Patterns:**
- **Safety model**: only AppendSkill is auto-applied; security fields (api_key, permissions, etc.) are FORBIDDEN even with approval
- `load_event_log` bridges Runtime EventLog format to digester's DigestEvent
- `enrich_with_llm` is cosmetic-only — cannot change suggestion kind/target/safety
- `write_skill` generates SKILL.md with frontmatter (name, description, triggers, priority)
- `apply` hard-guards security fields before any other logic

**Code Quality Issues:**
- `load_event_log` uses `chrono::Utc::now()` for all events — loses original timestamps from the log
- `write_skill` sanitizes filename by replacing non-alphanumeric chars with `-` — but doesn't prevent path traversal (e.g., `..` becomes `-` which is safe, but worth noting)
- `enrich_with_llm` prompt has weird indentation (lines 191-205) — may confuse the model

**Bugs/Concerns:**
- `load_trace` and `load_event_log` are separate methods with similar logic — could be unified
- `TraceRecord::event_tag` only works for externally-tagged enums with exactly 1 key — silently returns None for other shapes
- `write_skill` doesn't check for existing files — overwrites previous reflector-generated skills with the same ID
- The reflector can auto-write skill files to disk without any user interaction — even though it's "safe" (append-only), this is autonomous filesystem modification

---

## 36. `reflector/diff_observer.rs` (106 lines)

**Purpose:** DiffObserver — takes file snapshots before runs and checks for manual edits after.

**Key Types:**
- `DiffObserver` — no fields (unit struct)
- `UserEditDiffEvent` — file_path, diff

**Notable Patterns:**
- `take_snapshot` extracts file paths from tool call args (checks 5 key names: file_path, target_file, path, file, TargetFile)
- Files >1MB are skipped
- `check_for_diffs` uses `similar::TextDiff` for unified diff output
- Snapshot directory is pruned after checking

**Code Quality Issues:**
- `check_for_diffs` reverses the safe_name transformation by replacing `_` with `/` — this is LOSSY: a file path like `my_dir/my_file.rs` becomes `my_dir_my_file.rs` in snapshot, then `my/dir/my/file.rs` on restore — completely wrong path
- `take_snapshot` uses `path_str.replace("/", "_")` for safe_name — but Windows uses `\` as separator

**Bugs/Concerns:**
- **Path round-trip is broken**: `"/a/b_c.rs"` → snapshot as `"_a_b_c.rs"` → restored as `"/a/b/c.rs"` — wrong path, will fail to find the file
- `check_for_diffs` reads files as UTF-8 strings — binary files will fail silently
- No limit on number of files snapshotted — a run that touches 1000 files creates 1000 copies

---

## 37. `reflector/digester.rs` (296 lines)

**Purpose:** Pure, deterministic digester heuristics — detects tool error patterns, loops, and permission denials.

**Key Types:**
- `DigestEvent` — kind, tool_name, args, is_error, message, turn_index, ts
- `DigestEventKind` — TurnStart, ToolStart, ToolEnd, Error
- `DigesterRule` — named rule constants
- `Digester` — stateless analyzer

**Notable Patterns:**
- Three rules: consecutive_tool_errors (3+ same tool), tool_loop (same tool 3+ turns), frequent_denials (3+ permission errors)
- `fired` HashMap prevents duplicate suggestions for the same tool
- Streak resets on success
- `slug` normalizes tool names for suggestion IDs

**Code Quality Issues:**
- `DigesterRule` has `name` field but is never used for dispatch — just documentation
- `tool_loop` counts per-turn, not per-call — a tool called 100 times in one turn counts as 1
- `frequent_denials` checks `message.contains("Permission denied")` — string matching is fragile

**Bugs/Concerns:**
- `consecutive_tool_errors` streak is not reset between turns — errors across turns accumulate
- `tool_loop` uses `current_turn` which is set on TurnStart — if ToolEnd arrives before any TurnStart, it's assigned to `None` turn and not counted
- `frequent_denials` only checks Error events, not ToolEnd events with permission-denied results — may miss denials that come as tool results, not errors

---

## 38. `reflector/suggestion.rs` (145 lines)

**Purpose:** Suggestion model + safety allow-lists.

**Key Types:**
- `SuggestionKind` — AppendSkill, MemoryThreshold, PermissionChange, CredentialChange, BehaviorLimit
- `Suggestion` — id, kind, target, rationale, detected_at, skill_triggers, skill_body
- `SuggestionAction` — Applied, NeedsApproval(String), Forbidden
- `SAFE_AUTO_APPLY` — `[AppendSkill]` only
- `SECURITY_FIELDS` — api_key, base_url, model_id, permissions, mode, blacklist

**Notable Patterns:**
- `touches_security_field` checks kind, not target string — robust against creative target paths
- `diff_preview` produces human-readable change descriptions

**Code Quality Issues:**
- `SECURITY_FIELDS` is defined but never used in `touches_security_field` — the check is purely kind-based. The constant is for documentation only.
- Clean, well-tested.

**Bugs/Concerns:**
- A `MemoryThreshold` suggestion with target `"api_key"` would NOT be flagged as security — `touches_security_field` only checks kind, not target. This is a potential bypass if a future digester rule produces a `MemoryThreshold` suggestion targeting a security field.

---

## 39. `error_recovery/mod.rs` (214 lines)

**Purpose:** RecoveryEngine — determines recovery strategy for API errors (retry, escalate tokens, switch model, compact context).

**Key Types:**
- `RecoveryStrategy` — RetryWithBackoff, TokenEscalation, FallbackModel, ContextCompact, PathSwitch
- `RecoveryContext` — attempt, last_error, token_count, max_tokens, model_name
- `RecoveryEngine` — strategies (unused), fallback_model, max_retries, token_escalation_factor, compact_threshold
- `RecoveryAction` — Retry, EscalateTokens, SwitchModel, CompactContext, Fail

**Notable Patterns:**
- Error string matching: "too long"/"context length" → CompactContext, "rate limit"/"429" → Retry/SwitchModel, "length"/"truncat" → EscalateTokens
- Exponential backoff: `500 * 2^attempt` ms
- Fallback to model switch after max_retries

**Code Quality Issues:**
- `_strategies` field is unused — the strategies are hardcoded in `determine_strategy`
- `RecoveryStrategy` enum is defined but never used by `RecoveryEngine` — dead code
- `PathSwitch` variant in `RecoveryStrategy` is never produced

**Bugs/Concerns:**
- Error string matching is fragile — different API providers use different error messages
- `500 * 2u64.pow(ctx.attempt)` can overflow for large attempt counts (though max_retries caps this)
- "length" substring check (line 113) matches "context length" too — the order of checks matters but is fragile (the "too long"/"context length" check comes first, so this is OK, but it's brittle)
- No actual retry execution — `determine_strategy` returns an action but doesn't execute it

---

## 40. `background/mod.rs` (137 lines)

**Purpose:** BackgroundPool — spawns and tracks background tasks with notification channel.

**Key Types:**
- `Notification` — Completed, Failed, Progress
- `BackgroundTask` — id, description, status
- `BackgroundStatus` — Running, Completed(String), Failed(String)
- `BackgroundPool` — tasks, rx, tx, join_set

**Notable Patterns:**
- Unbounded channel for notifications
- `poll_notifications` drains non-blockingly
- `abort_all` on Drop via join_set

**Code Quality Issues:**
- `BackgroundTask` struct is defined but never used — dead code
- `spawn` uses `tokio::spawn` not `self.join_set.spawn` — the join_set is never populated, so `abort_all` is a no-op
- `tx` field is unused after construction (sender is cloned in `spawn`)

**Bugs/Concerns:**
- **Critical**: `spawn` uses `tokio::spawn` instead of `self.join_set.spawn` — the JoinSet is NEVER populated. `abort_all()` and `Drop` call `join_set.abort_all()` which aborts nothing. Background tasks are leaked.
- `spawn` takes `_description` but doesn't use it — no way to identify tasks by description
- No limit on number of concurrent background tasks
- `poll_notifications` updates task status but doesn't remove completed tasks — unbounded growth

---

## 41. `cron/mod.rs` (117 lines)

**Purpose:** CronJob CRUD — SQLite persistence for scheduled agent jobs.

**Key Types:**
- `CronJob` — id, name, cadence_type, cadence_value, prompt, project, skills, permission_level, max_concurrency, model, enabled, created_at
- `CronJobRun` — id, cronjob_id, session_id, timestamps, status
- `CronjobStore` — static CRUD methods

**Notable Patterns:**
- Skills serialized as JSON array string
- `enabled` stored as i32 (0/1)

**Code Quality Issues:**
- No validation of cadence_type or cadence_value
- No run scheduling logic — just storage

**Bugs/Concerns:**
- `DateTime::parse_from_rfc3339(&created_at_str).unwrap_or_default()` — `unwrap_or_default()` on `DateTime` gives Unix epoch, silently hiding parse errors
- No cascade delete for CronJobRun when CronJob is deleted

---

## 42. `cron/manager.rs` (21 lines)

**Purpose:** CronScheduler — wraps tokio-cron-scheduler.

**Key Types:** `CronScheduler` — scheduler (JobScheduler).

**Code Quality Issues:**
- `new()` calls `unimplemented!()` — this is a stub that will panic at runtime
- `len()` always returns 0 — hardcoded

**Bugs/Concerns:**
- **Critical**: `CronScheduler::new()` panics with `unimplemented!()` — any code that enables cron (`enable_cron: true` in ComprehensiveAgentBuilder) will crash
- The comment says "we will mock this for now" — but it's not a mock, it's a panic

---

## 43. `teams/mod.rs` (97 lines)

**Purpose:** DEPRECATED — agent team communication via MessageBus. Superseded by workflow::WorkflowContext.

**Key Types:**
- `TeamMessage` — id, from, to, content, msg_type, timestamp
- `TeamMessageType` — Request, Reply, Notification, Shutdown
- `AgentTeam` — name, agents, bus

**Notable Patterns:**
- Simple inbox-based messaging
- `create_request` / `create_reply` convenience functions

**Code Quality Issues:**
- Module-level doc comment is indented incorrectly (leading whitespace before `//!`)
- `#![allow(deprecated)]` suppresses warnings for the entire module

**Bugs/Concerns:**
- Deprecated and not wired into new functionality — may be removed in V3
- No message ordering guarantee
- No message persistence

---

## 44. `teams/bus.rs` (113 lines)

**Purpose:** MessageBus — shared inboxes for agent-to-agent communication.

**Key Types:** `MessageBus` — inboxes (Arc<Mutex<HashMap<String, Vec<TeamMessage>>>>).

**Notable Patterns:**
- `register`/`unregister` for agent enrollment
- `send` auto-creates inbox if not registered
- `receive` drains inbox (take)
- `peek` non-destructive read

**Code Quality Issues:**
- Clean, well-tested.

**Bugs/Concerns:**
- `send` to an unregistered agent creates an inbox silently — messages accumulate with no consumer
- No message size limit — could grow unbounded
- No broadcast mechanism

---

## 45. `trace/mod.rs` (267 lines)

**Purpose:** TraceCollector — high-fidelity JSONL recording of AgentEvent stream.

**Key Types:**
- `TraceCollector` — file, task_id, max_line_chars (default 64KB)

**Notable Patterns:**
- Best-effort: errors swallowed (eprintln)
- Lines exceeding `max_line_chars` replaced with truncated stub (keeps file parseable)
- `record_raw` escape hatch for synthetic lines
- `expand_tilde` for `~` and `~/` path expansion
- `event_tag` for identifying event variants in truncated stubs

**Code Quality Issues:**
- `expand_tilde` uses `HOME` or `USERPROFILE` env vars — doesn't use `dirs::home_dir()` like the rest of the codebase
- `event_tag` is a massive match — must be updated when new AgentEvent variants are added
- `eprintln!` for error logging — should use `tracing`

**Bugs/Concerns:**
- `record` serializes the event to `Value` first, then to string — double serialization
- `format_line` re-serializes to check length — could check length after first serialization
- File handle is not buffered — each `writeln!` is a syscall
- No file rotation — long-running traces can fill disk

---

## 46. `mode.rs` (88 lines)

**Purpose:** AgentMode — Ask (read-only), Plan (research + plan), Build (full access).

**Key Types:** `AgentMode` — Ask, Plan, Build.

**Notable Patterns:**
- `system_prompt_override` returns mode-specific instructions
- `tools Plan (research + plan), Build (full access).

**Key Types:** `AgentMode` — Ask, Plan, Build.

**Notable Patterns:**
-_to_remove` returns tool names to filter from registry
 `system_prompt_override` returns mode-specific instructions
- `tools- Modes are cumulative: Ask removes everything Plan removes, plus plan-specific tools_to_remove` returns tool names to filter from registry
- Modes are cumulative: Ask
- `is_write_allowed` convenience method

**Code Quality removes everything Plan removes, plus plan-specific tools
- `is_write_allowed Issues:**
- Clean, simple, correct.

**Bugs/Concerns:` convenience method

**Code Quality**
- `tools_to_remove` for Ask includes `" Issues:**
- Clean, simple, correct.

**Bugs/Concerns:task"` but the actual tool names are `task_create`, `task_update`, etc**
- `tools_to_remove` for Ask includes `". — `"task"` won't match any registered tooltask"` but the actual tool names are `task_create`, `task_update`, etc
- Plan mode removes `git_commit. — `"task"` won't match any registered tool` but not `git_status`/`git_diff` — correct
- Plan mode removes `git_commit, but the list must be` but not `git_status`/`git_diff` — correct, but the list must be maintained manually

---

## 47. `paths.rs` (69 lines)

**Purpose:** Path utilities — centralizes all `.agverse` directory paths.

**Key Types:** (module maintained manually

---

## 47. `paths.rs` (69 lines)

**Purpose:** Path utilities — centralizes functions)

**Notable Patterns:**
- All paths all `.agverse` directory paths.

**Key Types:** (module rooted at `~/.agverse functions)

**Notable Patterns:**
- All paths`
- `redirect_if_artifact` moves system artifacts (plan rooted at `~/.agverse`
- `redirect_if_artifact` moves system.md, media files) to session chat folder artifacts (plan.md, media files)

**Code Quality Issues:**
- `get_agverse_dir` uses to session chat folder

**Code Quality Issues:**
- `get_ag `dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))`verse_dir` uses `dirs::home_dir().unwrap_or_else(|| Path — `~` is not a valid path on Windows
Buf::from("~"))` — `~` is not- `redirect_if a valid path on Windows
- `redirect_if_artifact` uses string matching for file extensions — could use a set

**Bugs/Concerns:**
- `redirect_if_artifact` creates the chat directory as a side effect — callers may not expect filesystem writes
- No path canonicalization — relative paths and symlinks could cause issues
- `get_cli_history_dir` uses a DIFFERENT base (`~/.agverse_history`) than other paths (`~/.agverse`) — inconsistent

---

## 48. `project.rs` (259 lines)

**Purpose:** ProjectManager — organizes sessions under local directories (like VS Code workspaces).

**Key Types:**
- `Project` — id, name, path, timestamps
- `ProjectManager` — storage backend

**Notable Patterns:**
- `create` checks for existing project with same path (dedup)
- `__adhoc_chat__` is a reserved project ID — can't be renamed or deleted
- `delete` cascades to sessions and session_messages
- `from_path` auto-generates name from directory basename

**Code Quality Issues:**
- `create` uses a separate `SELECT` + `INSERT` instead of `INSERT OR IGNORE` or `UPSERT` — race condition between check and insert
- `delete` manually deletes session_messages and sessions instead of using FK CASCADE

**Bugs/Concerns:**
- **Race condition**: two concurrent `create` calls with the same path could both pass the existence check and insert duplicates
- `delete` doesn't delete session_event_log entries — orphaned event log data
- `list_sessions` uses `META_SELECT` with `WHERE project_id = ?1` — but `META_SELECT` doesn't include `project_id` in its WHERE clause, it's appended manually. The SQL injection risk is mitigated by parameterized queries, but the pattern is fragile.

---

## 49. `worktree/mod.rs` (140 lines)

**Purpose:** WorktreeManager — creates/removes git worktrees for task isolation.

**Key Types:**
- `WorktreeRecord` — id, task_id, path, branch, created_at, status
- `WorktreeStatus` — Active, Merged, Removed
- `WorktreeManager` — repo_root, worktrees (Vec, in-memory only)

**Notable Patterns:**
- `create` tries `git worktree add -b <branch>`, falls back to `git worktree add <branch>` if "already exists"
- `remove` calls `git worktree remove`, falls back to `rm -rf`
- Worktrees stored in `.worktrees/` under repo root

**Code Quality Issues:**
- `worktrees` is in-memory only — no persistence, lost on restart
- `create` uses `worktree_path.to_str().unwrap()` — panics on non-UTF8 paths
- `remove` uses `record.path.to_str().unwrap()` — same panic risk
- No `list_all` returning owned data — returns `&[WorktreeRecord]` which borrows self

**Bugs/Concerns:**
- **No branch conflict handling**: if the branch already exists, the fallback `git worktree add <path> <branch>` checks out the existing branch, which may not be what the caller wants
- `remove` marks status as Removed but doesn't remove from the Vec — `list_active` filters correctly but `list_all` returns removed records
- `create` doesn't check if the worktree path already exists — `git worktree add` will fail, but the error message may be confusing
- No `git worktree prune` — stale worktree metadata in git may accumulate
- Tests don't actually create worktrees (they'd need a real git repo) — only test empty manager

---

## 50. `comprehensive/mod.rs` (500 lines)

**Purpose:** ComprehensiveAgentBuilder — builder that wires together all optional subsystems (memory, permissions, hooks, todo, tasks, background, cron, skills, teams, worktree, reflector).

**Key Types:**
- `ComprehensiveAgentBuilder` — 15+ enable/disable flags
- `ComprehensiveAgent` — agent + all optional subsystems
- `ReflectionReport` — applied, needs_approval, forbidden

**Notable Patterns:**
- Builder pattern with fluent `memory(bool)`, `permission(bool)`, etc.
- `reflect_on` — end-to-end reflection: load trace → analyze → apply safe → collect approvals
- `status()` — human-readable summary of all subsystems

**Code Quality Issues:**
- `CronScheduler::new()` panics with `unimplemented!()` — if `enable_cron` is true, `build()` will panic
- `status()` method is long and repetitive — could use a macro
- `ComprehensiveAgent` has 11 `Option<...>` fields — could use a struct per subsystem
- `reflect_on` borrows `self.agent.client_for_reflection()` — coupling between ComprehensiveAgent and Agent internals

**Bugs/Concerns:**
- **Cron panic**: `CronScheduler::new()` panics — `enable_cron(true)` in the builder will crash `build()`
- `reflect_on` with `enrich=true` calls `enrich_with_llm` which makes LLM API calls — if the model endpoint is unreachable, this blocks the reflection
- No way to disable individual subsystems after build — `ComprehensiveAgent` fields are `pub` but removing a subsystem would require reconstructing
- `BackgroundPool::spawn` doesn't use `join_set.spawn` (as noted in background/mod.rs) — `abort_all` is a no-op, so background tasks in ComprehensiveAgent are leaked

---

# Summary of Critical Bugs

1. **`cron/manager.rs`**: `CronScheduler::new()` calls `unimplemented!()` — will panic if cron is enabled
2. **`background/mod.rs`**: `spawn` uses `tokio::spawn` not `join_set.spawn` — `abort_all` is a no-op, tasks are leaked
3. **`reflector/diff_observer.rs`**: Path round-trip is broken (`/` → `_` → `/` is lossy) — file diffs fail silently
4. **`mcp/transport.rs`**: Duplicate `if response.id == Some(id)` check — dead code, and no timeout on `read_line`
5. **`subagent/mod.rs`**: Each iteration creates a new CancellationToken not linked to parent — subagent tools can't be cancelled by parent Run cancellation
6. **`tasks/board.rs`**: `truncate` byte-slices strings — panics on multi-byte chars
7. **`session.rs`**: `display_line` byte-slices timestamp — panic risk (though RFC3339 is ASCII)
8. **`workflow/executor.rs`**: `execute_agent_node` ignores cancel_token — workflow cancellation doesn't cancel subagents
9. **`comprehensive/mod.rs`**: Enabling cron in the builder causes a panic at build time
