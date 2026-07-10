//! Run-scoped execution state machine.
//!
//! Distinct from [`super::state::RunState`] (run lifetime). This tracks
//! *what phase of the task* the agent is in and which step is active —
//! owned by the runtime, injected every turn, not freely rewritten by the model.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::todo::{TodoList, TodoStatus};

/// Task-execution phase for a single Run (orthogonal to [`super::state::RunState`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    /// Need clarification before planning/acting.
    Clarify,
    /// Building or revising the plan (todo list).
    #[default]
    Plan,
    /// Working the locked plan step-by-step.
    Execute,
    /// Checking results / tests before Done.
    Verify,
    /// Task finished; Final answers allowed.
    Done,
}

impl std::fmt::Display for ExecutionPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clarify => write!(f, "clarify"),
            Self::Plan => write!(f, "plan"),
            Self::Execute => write!(f, "execute"),
            Self::Verify => write!(f, "verify"),
            Self::Done => write!(f, "done"),
        }
    }
}

impl ExecutionPhase {
    /// Whether the model may end the turn with text-only Final.
    pub fn allows_final(self) -> bool {
        matches!(self, Self::Done | Self::Clarify | Self::Plan)
    }

    /// Whether a full plan rewrite (`todo_write` without force) is allowed.
    pub fn allows_replan(self) -> bool {
        matches!(self, Self::Clarify | Self::Plan)
    }
}

/// Runtime-owned execution dashboard for one Run.
#[derive(Debug, Clone, Default)]
pub struct ExecutionState {
    pub phase: ExecutionPhase,
    /// Bumped when the plan is (re)written. Used to detect wipe/replan.
    pub plan_version: u32,
    /// Current step the agent must advance (todo id).
    pub active_step_id: Option<String>,
    /// Recent filesystem / tool facts (newest last, capped).
    pub artifacts: VecDeque<String>,
    /// One-shot resume hint after abort / tool failure (cleared after inject).
    pub resume_hint: Option<String>,
    /// How many times we blocked a premature Final in Execute.
    pub final_blocks: u8,
}

const ARTIFACT_CAP: usize = 12;
const MAX_FINAL_BLOCKS: u8 = 5;

impl ExecutionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_artifact(&mut self, fact: impl Into<String>) {
        let fact = fact.into();
        if fact.is_empty() {
            return;
        }
        // Dedup exact repeats at the tail.
        if self.artifacts.back().map(|s| s == &fact).unwrap_or(false) {
            return;
        }
        self.artifacts.push_back(fact);
        while self.artifacts.len() > ARTIFACT_CAP {
            self.artifacts.pop_front();
        }
    }

    pub fn set_resume_hint(&mut self, hint: impl Into<String>) {
        self.resume_hint = Some(hint.into());
    }

    pub fn take_resume_hint(&mut self) -> Option<String> {
        self.resume_hint.take()
    }

    /// Sync phase + active step from the current todo list.
    pub fn sync_from_todos(&mut self, list: &TodoList) {
        if list.items.is_empty() {
            if self.phase == ExecutionPhase::Execute || self.phase == ExecutionPhase::Verify {
                self.phase = ExecutionPhase::Plan;
            }
            self.active_step_id = None;
            return;
        }

        let all_done = list
            .items
            .iter()
            .all(|i| i.status == TodoStatus::Completed);
        if all_done {
            self.phase = if self.phase == ExecutionPhase::Done {
                ExecutionPhase::Done
            } else {
                ExecutionPhase::Verify
            };
            self.active_step_id = None;
            return;
        }

        // Prefer existing in_progress; else first ready/pending.
        let active = list
            .in_progress()
            .first()
            .map(|i| i.id.clone())
            .or_else(|| list.ready_items().first().map(|i| i.id.clone()))
            .or_else(|| {
                list.items
                    .iter()
                    .find(|i| i.status == TodoStatus::Pending)
                    .map(|i| i.id.clone())
            });

        self.active_step_id = active;
        // Incomplete work ⇒ Execute (unless clarifying). Plan only when no items.
        if self.phase != ExecutionPhase::Clarify && self.phase != ExecutionPhase::Done {
            self.phase = ExecutionPhase::Execute;
        }
    }

    /// Called after a successful plan write.
    pub fn on_plan_written(&mut self, list: &TodoList, forced: bool) {
        self.plan_version = self.plan_version.saturating_add(1);
        if !list.items.is_empty() {
            self.phase = ExecutionPhase::Execute;
            // Ensure one in_progress.
            // Caller may have already set statuses via merge.
            self.sync_from_todos(list);
            if self.active_step_id.is_none() {
                self.sync_from_todos(list);
            }
        }
        if forced {
            self.record_artifact(format!("plan rewritten (force) v{}", self.plan_version));
        } else {
            self.record_artifact(format!("plan set v{}", self.plan_version));
        }
    }

    /// Enter Verify when all todos completed; Done when verify passes (caller).
    pub fn mark_verified_done(&mut self) {
        self.phase = ExecutionPhase::Done;
        self.active_step_id = None;
    }

    pub fn enter_clarify(&mut self) {
        self.phase = ExecutionPhase::Clarify;
    }

    pub fn enter_plan(&mut self) {
        if self.phase != ExecutionPhase::Execute {
            self.phase = ExecutionPhase::Plan;
        }
    }

    /// Whether we should block a text-only Final and nudge continue.
    pub fn should_block_final(&self, list: &TodoList) -> bool {
        if self.final_blocks >= MAX_FINAL_BLOCKS {
            return false;
        }
        match self.phase {
            ExecutionPhase::Execute | ExecutionPhase::Verify => {
                !list.items.is_empty()
                    && list
                        .items
                        .iter()
                        .any(|i| i.status != TodoStatus::Completed)
            }
            ExecutionPhase::Done => false,
            ExecutionPhase::Clarify | ExecutionPhase::Plan => false,
        }
    }

    pub fn note_final_blocked(&mut self) {
        self.final_blocks = self.final_blocks.saturating_add(1);
    }

    /// Build the Segment 7 dashboard text (phase + next + artifacts + rules).
    pub fn to_injection(&self, list: &TodoList) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "== EXECUTION STATE (runtime) ==\n\
             phase: {}\n\
             plan_version: {}\n",
            self.phase, self.plan_version
        ));

        let completed = list
            .items
            .iter()
            .filter(|i| i.status == TodoStatus::Completed)
            .count();
        let total = list.items.len();
        if total > 0 {
            out.push_str(&format!(
                "progress: {completed}/{total} ({:.0}%)\n",
                list.completion_rate() * 100.0
            ));
        }

        if let Some(ref id) = self.active_step_id {
            if let Some(item) = list.get(id) {
                out.push_str(&format!(
                    "NEXT: [{}] {} — {}\n",
                    item.status, item.id, item.description
                ));
            } else {
                out.push_str(&format!("NEXT: step {id}\n"));
            }
        } else if total == 0 {
            out.push_str("NEXT: create a plan with todo_write (or ask_user if unclear)\n");
        } else if list
            .items
            .iter()
            .all(|i| i.status == TodoStatus::Completed)
        {
            out.push_str("NEXT: verify results, then finish\n");
        }

        if let Some(ready) = list.ready_items().first() {
            if self.active_step_id.as_deref() != Some(ready.id.as_str()) {
                out.push_str(&format!(
                    "READY: [{}] {} — {}\n",
                    ready.status, ready.id, ready.description
                ));
            }
        }

        if !self.artifacts.is_empty() {
            out.push_str("Artifacts (recent):\n");
            for a in &self.artifacts {
                out.push_str(&format!("  - {a}\n"));
            }
        }

        out.push_str("Rules:\n");
        match self.phase {
            ExecutionPhase::Clarify => {
                out.push_str(
                    "  - Call ask_user. Do not todo_write a full plan or mutate files yet.\n",
                );
            }
            ExecutionPhase::Plan => {
                out.push_str(
                    "  - Call todo_write once with concrete steps, then execute.\n",
                );
            }
            ExecutionPhase::Execute => {
                out.push_str(
                    "  - Work the NEXT step with tools. Use todo_update to advance.\n\
                      - Do NOT call todo_write to replan unless the plan is wrong (pass force=true).\n\
                      - Do NOT end with prose-only while steps remain.\n",
                );
            }
            ExecutionPhase::Verify => {
                out.push_str(
                    "  - Verify outcomes (tests/read). If ok, finish; if not, fix and todo_update.\n",
                );
            }
            ExecutionPhase::Done => {
                out.push_str("  - Task complete. Summarize briefly.\n");
            }
        }

        if let Some(ref hint) = self.resume_hint {
            out.push_str(&format!("RESUME: {hint}\n"));
        }

        out.push('\n');
        out.push_str(&list.to_context_string());
        out
    }
}

/// Parse tool success into a short artifact fact (best-effort).
pub fn artifact_from_tool(name: &str, args_json: &str, result: &str, is_error: bool) -> Option<String> {
    if is_error || result.starts_with("Aborted") {
        return None;
    }
    match name {
        "write_file" | "edit" => {
            let path = extract_json_str(args_json, "path")
                .or_else(|| extract_json_str(args_json, "file_path"));
            path.map(|p| format!("{name}: {p}"))
        }
        "bash" => {
            let cmd = extract_json_str(args_json, "command").unwrap_or_default();
            if cmd.contains("mkdir") {
                Some(format!("bash mkdir: {}", truncate(&cmd, 80)))
            } else if !cmd.is_empty() {
                Some(format!("bash ok: {}", truncate(&cmd, 60)))
            } else {
                None
            }
        }
        "todo_write" => Some("todo_write ok".into()),
        "todo_update" => {
            let id = extract_json_str(args_json, "id").unwrap_or_default();
            let status = extract_json_str(args_json, "status").unwrap_or_default();
            if id.is_empty() {
                None
            } else {
                Some(format!("todo {id} → {status}"))
            }
        }
        _ => None,
    }
}

fn extract_json_str(args: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args).ok()?;
    v.get(key)?.as_str().map(|s| s.to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todo::TodoItem;

    #[test]
    fn sync_picks_in_progress_then_ready() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "a"));
        list.add(TodoItem::new("2", "b"));
        list.update_status("1", TodoStatus::Completed).unwrap();
        list.update_status("2", TodoStatus::InProgress).unwrap();

        let mut st = ExecutionState::new();
        st.phase = ExecutionPhase::Execute;
        st.sync_from_todos(&list);
        assert_eq!(st.active_step_id.as_deref(), Some("2"));
    }

    #[test]
    fn block_final_in_execute_with_pending() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "a"));
        let mut st = ExecutionState::new();
        st.phase = ExecutionPhase::Execute;
        assert!(st.should_block_final(&list));
        st.phase = ExecutionPhase::Done;
        assert!(!st.should_block_final(&list));
    }

    #[test]
    fn injection_contains_phase_and_next() {
        let mut list = TodoList::new();
        list.add(TodoItem::new("1", "scaffold"));
        list.update_status("1", TodoStatus::InProgress).unwrap();
        let mut st = ExecutionState::new();
        st.phase = ExecutionPhase::Execute;
        st.plan_version = 1;
        st.sync_from_todos(&list);
        st.record_artifact("write_file: src/main.rs");
        let text = st.to_injection(&list);
        assert!(text.contains("phase: execute"));
        assert!(text.contains("NEXT:"));
        assert!(text.contains("Artifacts"));
        assert!(text.contains("Do NOT call todo_write"));
    }
}
