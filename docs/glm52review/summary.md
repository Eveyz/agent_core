I'll conduct this comprehensive audit. Let me start by mapping the project structure and reading the key subsystems in parallel.
Let me read the critical core files in parallel batches.
Continuing the audit. Let me read the remaining critical files in parallel batches.
Continuing the audit. Let me read the remaining critical files in parallel batches, then spawn subagents for deep analysis.
Let me continue reading the remaining critical core files and spawn subagents for the CLI/TUI and Tauri app analysis.
Let me spawn parallel subagents to read and analyze the remaining subsystems while I continue the core analysis.
Let me spawn parallel subagents for the CLI/TUI, Tauri app, and remaining core modules, while I continue the deep analysis myself.
Excellent findings from the subagents. Let me now continue the deep analysis myself — reading remaining critical files and running code quality scans in parallel.
Let me run code quality scans and read remaining critical files in parallel.
I now have comprehensive coverage of the codebase. Let me compile the full audit report.
The full audit is complete. The report has been written to `agent_core_full_audit.md` (850 lines, 45KB).

## Summary of Findings

**Overall Assessment:** This is a production-grade, architecturally sophisticated Rust AI agent framework — 44,078 lines across 3 workspace crates. The design is well above average for an independent project.

### Key Strengths

1. **7-Segment Context Engine** — Semantic system prompt assembly with per-segment refresh policies (Never / OnEvent / PerTurn / OnRegister / OnDemand) and KV-cache stability tracking (Stable / SemiStable / Dynamic). This is genuinely advanced — most agent frameworks just concatenate a flat system prompt.

2. **KV Cache Strategy** — Frozen system prompt (Stable segments only) + untouched conversation history + dynamic context injection as a trailing user message. This maximizes prefix cache hits, which is critical for DeepSeek cost optimization (your primary model). The `stable_prefix_fingerprint()` drift detection and idle-time cache-expiry warnings show deep understanding of real-world API behavior.

3. **Brain / Run Architecture** — Clean separation of reusable state (Brain, shared via `Arc`) from per-request state (Run, isolated). Each Run owns its own `CancellationToken`, `ProcessSupervisor`, `JoinSet`, and `ContextEngine`. RAII cleanup guarantees no resource leaks.

4. **Permission System** — 6-layer evaluation pipeline (Sandbox → Yolo → Blacklist → Whitelist → Config Rules → Builtin Rules → Default Ask) with audit logging. The destructive-command deny is correctly positioned to survive `auto_allow_up_to = Destructive`.

5. **Memory System** — Dual-track (core + recall + archival) with BM25 (tantivy), HNSW (instant-distance), embedding (fastembed/bge-small), salience scoring, RRF fusion, and background reflection daemon. The `try_lock_memory` with 3s timeout prevents consolidation from blocking the hot path.

### Critical Issues Found

| # | Severity | Issue | File |
|---|----------|-------|------|
| 1 | **High** | `std::env::set_current_dir` in subagent.rs is process-global and not thread-safe — concurrent Runs in different worktrees will race | `tools/subagent.rs:647,655` |
| 2 | **High** | Blocking `std::fs::read_to_string` calls in async `refresh_context_segments()` — 7 sync file reads per turn on the async runtime | `runtime/run.rs:1325-1370` |
| 3 | **Medium** | `std::sync::Mutex` used for `current_mode` in Brain instead of `parking_lot::Mutex` — inconsistent with rest of codebase, risk of poison panics | `runtime/brain.rs:292,298` |
| 4 | **Medium** | `unwrap()` on `get_model()` in `current_model_config()` — panics if model is deleted from config while agent is running | `agent/mod.rs:831` |
| 5 | **Low** | `eprintln!` debug statements left in production code (resolve_approval, executor, MCP) | `runtime/run.rs:651,665,674` |
| 6 | **Low** | `RecoveryAction::SwitchModel` is unimplemented — always gives up instead of switching | `runtime/run.rs:1203-1208` |

### Improvement Roadmap

The report includes a detailed prioritized roadmap covering:
- **P0 (Immediate):** Fix `set_current_dir` race, move blocking FS I/O to `spawn_blocking`, add `.clone()` guard on `current_model_config()`
- **P1 (Near-term):** Implement `SwitchModel` recovery, replace `eprintln!` with `tracing::`, unify on `parking_lot::Mutex`
- **P2 (Medium-term):** Structured errors (thiserror for public API), batch session message inserts, `tokio::task::spawn_blocking` for all tool FS operations
- **P3 (Long-term):** Streaming tool results, MCP transport hardening, worktree-isolated subagents, configurable compaction strategy

The full 850-line report is at `agent_core_full_audit.md` with file-level analysis, code examples, and specific fix recommendations for each issue.
