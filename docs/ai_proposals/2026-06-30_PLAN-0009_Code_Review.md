# 2026-06-30_PLAN-0009_Code_Review

**Date**: 2026-06-30
**Subject**: Code Review for PLAN-0009 (User-Defined Agents & Multi-Agent Workflow System)

## Overview
The implementation for `PLAN-0009` successfully translates the proposed architecture into code. The core logic handles the separation of `RecallMemory` and `AgentMemory`, extends `Subagent` natively to support isolated contextual execution, and effectively implements the React Flow-based `WorkflowExecutor`.

The overall architectural alignment is excellent, particularly the decision to natively wrap memory and tools at the `Subagent` level rather than hijacking the `RunManager`.

Here is a detailed breakdown of the review.

---

## 1. Database Schema & Migration (`core/src/memory/storage.rs`)
> [!NOTE]
> The database migration approach is safe and idempotent.

**Strengths:**
* Added `add_column_if_not_exists` to gracefully handle `ALTER TABLE` operations across different client versions.
* Implemented `FTS5` virtual tables (`agent_memory_fts`) and correct SQLite triggers to automatically sync agent memory data on `INSERT`, `UPDATE`, and `DELETE`.
* Clean separation of global tables (`recall_memory`, `archival_memory`) from workflow tables (`agents`, `agent_memory`, `agent_history`, `workflows`, etc.).

**Suggestions for Future Improvement:**
* Currently, SQLite is used heavily in `tokio::task::spawn_blocking` (e.g. inside `executor.rs`). As workflow throughput increases, you may encounter `database is locked` SQLite concurrency errors, even with `WAL` mode. Consider a dedicated async DB connection pool (like `sqlx`) or an actor channel to funnel writes sequentially in later phases.

## 2. Subagent Extension (`core/src/subagent/mod.rs`)
> [!TIP]
> The approach to injecting memory avoids breaking the existing `ContextEngine` model.

**Strengths:**
* The addition of `SubagentConfig` extensions (model override, memory, permission mode) is backward compatible due to `Option<T>` and `#[serde(default)]`.
* `new_with_memory` correctly accepts an optional `AgentMemoryStore`, providing clean dependency injection.
* `inject_memory` fetches top relevant memories and appends them to the `active_memory` segment of the Context Engine perfectly before execution.
* `persist_memory` executes correctly after the conversation turn.

**Minor Issue / Consideration:**
* Inside `persist_memory()`, the errors are logged and swallowed via `tracing::warn!`. While this prevents the subagent from crashing due to a memory insertion failure, consider emitting an event to the frontend or attaching a warning to the `SubagentResult` so the user knows if their agent is failing to learn.

## 3. Workflow Executor (`core/src/workflow/executor.rs`)
> [!IMPORTANT]
> The DAG executor provides a strong foundation for future dynamic routing.

**Strengths:**
* Uses Kahn's algorithm (`planner::plan`) correctly to segment nodes into parallel stages.
* Bounded concurrency via `tokio::sync::Semaphore` prevents the system from getting API rate-limited if a user places 50 agents in one stage.
* `apply_router` safely evaluates the LangGraph-style condition targets and populates a `skipped` set for downstream filtering.
* Correctly propagates the `CancellationToken` into both the executor loop and `execute_agent_node()`.

**Suggestions for Future Improvement:**
* **Output fallback:** In `execute()`, the fallback looks for the last `NodeType::Agent` to return its output if there's no explicitly defined `Output` node. This might be non-deterministic if the last stage has multiple agents running in parallel. It works for V1, but a validation warning during workflow saving to enforce an explicitly connected `Output` node would be safer.

## 4. Trust Mode Integration (`core/src/workflow/trust.rs`)
> [!CAUTION]
> `TrustMode::Trusted` correctly forces `PermissionMode::Yolo`, which bypasses approvals.

**Strengths:**
* Clean logic overriding the base `PermissionConfig`. The test coverage (`trusted_mode_forces_yolo`, etc.) provides great confidence in this critical security feature.
* It guarantees that workflow automation won't stall on coding agents trying to write files.

## 5. Experimental Skill Drafts (`core/src/agent_registry/skill_drafts.rs`)
> [!NOTE]
> The heuristic approach to analyzing agent history is a smart, low-cost (no-LLM) alternative to complex reflections.

**Strengths:**
* Uses basic NLP stop-word filtering to find common keywords and map patterns across failed executions, high-iteration executions, and repeated tasks.
* Outputs standard markdown `SKILL.md` drafts with frontmatter to `drafts_dir`, maintaining consistency with the existing SkillManager.

## Summary
The commit is large but highly cohesive. The backend changes are robust, the memory scope isolation is strictly maintained, and the integration points respect the boundaries defined in the plan. 

**Conclusion:** 
The code is fully approved for V1. The implementation aligns perfectly with `PLAN-0009`. 
