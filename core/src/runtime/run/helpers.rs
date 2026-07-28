//! Run helpers — session memory, cleanup, and teardown.

use parking_lot::MutexGuard;

use crate::runtime::event::RunEvent;
use crate::runtime::supervisor::ProcessSupervisor;

use super::Run;

impl Run {
    pub(super) fn write_session_memory(&self, _final_text: &str) {
        let global_path = crate::paths::get_global_agverse_md_path();

        // If the file already exists, we do nothing. We let the Agent intelligently manage it via tools.
        if global_path.exists() {
            return;
        }

        if let Some(parent) = global_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let template = "\
# OS-Level Memory Architecture

You operate using an OS-level memory management system. Proactively manage your own context window to prevent memory bloat and maintain long-term reasoning capabilities.

## Core Memory (RAM)
This file (`~/.agverse/agverse.md`) is your Global Memory. It is 100% injected into your context on every turn as cross-project background. It is the source of truth for the Known Projects catalog, Coding Conventions, and User Preferences — not for the currently active repo.

## Active Project Rule (CRITICAL)
The active project is determined ONLY by the Working Directory and any project-local `agverse.md` / `AGENTS.md` for that cwd. Entries under Known Projects are a catalog. Never assume a catalog entry is current unless cwd matches.

## Archival Memory (Disk)
An underlying SQLite database contains historical conversation logs (Recall Memory) and long-term knowledge (Archival Memory). Use `conversation_search` and `archival_memory_search` tools to retrieve information when needed.

## Memory Management Directives (CRITICAL)
Your primary responsibility is to keep this Global Memory highly condensed, strictly structured, and up-to-date. You are forbidden from allowing this file to become a dump for raw conversational logs.

When you interact with the user, silently evaluate if the new information alters the global state:
- **Write/Replace:** If the user updates a global preference, coding convention, or the project catalog, use the `edit` tool to update the corresponding section below. Prefer replace over append when facts conflict.
- **Compaction (Delete):** If an existing rule is deprecated, overridden, or resolved, delete or overwrite the old information to free up Core Memory space. Do not append contradictory rules.
- **Offload to Archival (Ignore):** If the information is a specific debugging step, file:line audit, compile/test snapshot, or transient thought, do NOT write it here. Trust that Archival Memory will capture it for future retrieval.
- **Project-local:** Architecture and conventions for a specific repo belong in that repo's `agverse.md`, not here.
- **Pending Notes:** Staging only — not injected into every turn. Promote into a real section or delete.

Never mention your memory management process to the user unless explicitly asked. Maintain a seamless conversational flow while quietly updating this file in the background.

---

# Active Project Rule (CRITICAL)
The active project is determined ONLY by the Working Directory (and Project Instructions for that cwd).

# Known Projects (catalog)

# Project Overview

# Tech Stack & Commands

# Architecture Decisions

# Coding Conventions

# User Preferences

# Agent Instructions
";

        if let Err(e) = std::fs::write(&global_path, template) {
            tracing::warn!("failed to write agverse.md template: {e}");
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────

    /// Cancel-path cleanup: kill all processes, abort all tasks.
    pub(super) async fn cancel_and_cleanup(&mut self) {
        // 1. Trigger cancellation (propagates to model stream + tool exec)
        self.cancel.cancel();

        // 2. Abort all background tasks (subagent, memory consolidation, etc.)
        self.join_set.abort_all();
        while self.join_set.join_next().await.is_some() {}

        // 3. Kill all child processes
        {
            let mut sup: MutexGuard<'_, ProcessSupervisor> = self.supervisor.lock();
            sup.kill_all();
        }

        // 4. Drop all pending approvals (resolvers get a dropped error)
        self.approval_resolver.clear();
        self.input_resolver.clear();

        // 5. Cancel all remaining steering messages (notify frontend)
        let cancelled = self.steering.close();
        for entry in cancelled {
            self.emit(RunEvent::SteerCancelled {
                steer_id: entry.id,
                reason: "Run cancelled".to_string(),
            });
        }
    }

    /// Final cleanup (called on all exit paths, idempotent).
    pub(super) fn cleanup_on_exit(&mut self) {
        // Kill any remaining processes (idempotent if already killed)
        {
            let mut sup: MutexGuard<'_, ProcessSupervisor> = self.supervisor.lock();
            sup.kill_all();
        }

        // Abort any remaining tasks
        self.join_set.abort_all();

        // Drop pending approvals
        self.approval_resolver.clear();
        self.input_resolver.clear();

        // Cancel any remaining steering messages (normal completion path
        // where steer messages were queued but the run hit max iterations
        // or otherwise stopped before injecting them).
        let cancelled = self.steering.close();
        for entry in cancelled {
            self.emit(RunEvent::SteerCancelled {
                steer_id: entry.id,
                reason: "Run ended".to_string(),
            });
        }
    }
}
