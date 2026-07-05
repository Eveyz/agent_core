impl Run {
    fn write_session_memory(&self, _final_text: &str) {
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
This file (`~/.agverse/agverse.md`) is your Core Memory. It is 100% injected into your context on every turn. It is your ultimate source of truth for Project Overview, Architecture Decisions, Coding Conventions, and User Preferences.

## Archival Memory (Disk)
An underlying SQLite database contains historical conversation logs (Recall Memory) and long-term knowledge (Archival Memory). Use `conversation_search` and `archival_memory_search` tools to retrieve information when needed.

## Memory Management Directives (CRITICAL)
Your primary responsibility is to keep this Core Memory highly condensed, strictly structured, and up-to-date. You are forbidden from allowing this file to become a dump for raw conversational logs.

When you interact with the user, silently evaluate if the new information alters the global state:
- **Write/Replace:** If the user makes a new architectural decision, introduces a coding convention, or updates a global preference, use the `edit` tool to update the corresponding section below.
- **Compaction (Delete):** If an existing rule is deprecated, overridden, or resolved, delete or overwrite the old information to free up Core Memory space. Do not append contradictory rules.
- **Offload to Archival (Ignore):** If the information is a specific debugging step, a one-off script, or a transient thought, do NOT write it here. Trust that Archival Memory will capture it for future retrieval.

Never mention your memory management process to the user unless explicitly asked. Maintain a seamless conversational flow while quietly updating this file in the background.

---

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
    async fn cancel_and_cleanup(&mut self) {
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

        // 5. Cancel all remaining steering messages (notify frontend)
        let cancelled: Vec<SteerEntry> = self.steering_queue.drain(..).collect();
        for entry in cancelled {
            self.emit(RunEvent::SteerCancelled {
                steer_id: entry.id,
                reason: "Run cancelled".to_string(),
            });
        }
    }

    /// Final cleanup (called on all exit paths, idempotent).
    fn cleanup_on_exit(&mut self) {
        // Kill any remaining processes (idempotent if already killed)
        {
            let mut sup: MutexGuard<'_, ProcessSupervisor> = self.supervisor.lock();
            sup.kill_all();
        }

        // Abort any remaining tasks
        self.join_set.abort_all();

        // Drop pending approvals
        self.approval_resolver.clear();

        // Cancel any remaining steering messages (normal completion path
        // where steer messages were queued but the run hit max iterations
        // or otherwise stopped before injecting them).
        let cancelled: Vec<SteerEntry> = self.steering_queue.drain(..).collect();
        for entry in cancelled {
            self.emit(RunEvent::SteerCancelled {
                steer_id: entry.id,
                reason: "Run ended".to_string(),
            });
        }
    }
}
