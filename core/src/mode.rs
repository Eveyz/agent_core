//! Agent working mode — determines which tools are available to the model.
//!
//! Each [`Run`](crate::runtime::Run) is created in a specific mode that is
//! immutable for the Run's lifetime. Mode changes take effect on the next Run.

use serde::{Deserialize, Serialize};

/// The agent's working mode — controls tool availability.
///
/// - **Ask**: Read-only Q&A. Read files, search web, memory, read-only subagents.
///   No writes, no commands, no todos, no plan artifacts.
/// - **Plan**: Research & write a reviewable markdown plan (`plan.md`).
///   `write_file` only for planning artifacts; no shell / source edits / todos.
/// - **Build**: Full access. All tools available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Read-only Q&A — answer by reading files, searching web.
    Ask,
    /// Research & plan — write `plan.md` for user review.
    Plan,
    /// Full access — read, write, edit, execute.
    Build,
}

impl Default for AgentMode {
    fn default() -> Self {
        Self::Build
    }
}

impl AgentMode {
    /// Mode-specific instructions injected into the system prompt
    /// (Segment 2: PRINCIPLES).
    pub fn system_prompt_override(&self) -> &'static str {
        match self {
            Self::Ask => {
                "\
MODE: Ask — Read-only Q&A.
You can read files, use git status/diff/log/show, search the web,
search memory, and spawn read-only subagents.
You CANNOT write or edit files, run shell commands, or manage todo/plan tools.
Answer questions thoroughly by reading source code and documentation.
When the answer is ready, respond in plain text and stop.
If the user wants a written plan or code changes, tell them to switch to Plan or Build mode.
Do not invent or call tools that are not in your available tool list."
            }

            Self::Plan => {
                "\
MODE: Plan — Research & plan mode.
You can read files, search the web, spawn read-only subagents for research,
and write planning artifacts with write_file.
You CANNOT edit application source, run shell commands, or use todo tools.
Deliverable: a markdown plan the user can review in Overview → Artifacts.
When research is enough, write (or overwrite) `plan.md` via write_file.
Optional artifact names: `plan.md`, `implementation_plan.md`, `walkthrough.md`, `task.md`.
Do not modify project source files.
When the plan file is written, summarize briefly and stop so the user can review.
Suggest switching to Build mode only after they accept the plan."
            }

            Self::Build => {
                "\
MODE: Build — Full access.
You can read, write, edit files, execute shell commands, commit
to git, create todo plans, and spawn subagents. Skip todo_write for
simple 1–2 step tasks; use it only for complex multi-step work."
            }
        }
    }

    /// Tool names to remove from the registry for this mode.
    ///
    /// Modes are cumulative: Ask removes everything Plan removes, plus
    /// plan-specific tools.
    pub fn tools_to_remove(&self) -> &'static [&'static str] {
        match self {
            Self::Build => &[],
            Self::Plan => &[
                "edit",
                "sed",
                "shell",
                "repl",
                "git_commit",
                "todo_write",
                "todo_update",
                "todo_delete",
                "todo_read",
                "task",
            ],
            Self::Ask => &[
                "write_file",
                "edit",
                "sed",
                "shell",
                "repl",
                "git_commit",
                "todo_write",
                "todo_update",
                "todo_delete",
                "todo_read",
                "task",
            ],
        }
    }

    /// Returns true if file modifications and command execution are allowed.
    pub fn is_write_allowed(&self) -> bool {
        matches!(self, Self::Build)
    }

    /// Whether this mode may create/update planning markdown artifacts.
    pub fn allows_plan_artifacts(&self) -> bool {
        matches!(self, Self::Plan | Self::Build)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_removes_all_todo_and_write_tools() {
        let removed = AgentMode::Ask.tools_to_remove();
        for name in [
            "write_file",
            "edit",
            "shell",
            "repl",
            "todo_write",
            "todo_update",
            "todo_delete",
            "todo_read",
        ] {
            assert!(removed.contains(&name), "Ask must remove {name}");
        }
    }

    #[test]
    fn plan_keeps_write_file_but_removes_todos_and_shell() {
        let removed = AgentMode::Plan.tools_to_remove();
        assert!(
            !removed.contains(&"write_file"),
            "Plan must keep write_file for plan.md"
        );
        for name in [
            "todo_write",
            "todo_read",
            "edit",
            "shell",
            "repl",
            "git_commit",
        ] {
            assert!(removed.contains(&name), "Plan must remove {name}");
        }
    }

    #[test]
    fn ask_prompt_does_not_name_todo_write() {
        let text = AgentMode::Ask.system_prompt_override();
        assert!(!text.contains("todo_write"));
        assert!(!text.contains("todo_update"));
        assert!(text.contains("Ask"));
    }

    #[test]
    fn plan_prompt_directs_plan_md_not_todo_write() {
        let text = AgentMode::Plan.system_prompt_override();
        assert!(text.contains("plan.md"));
        assert!(text.contains("write_file"));
        // Mentions the ban explicitly is ok; must not instruct to *use* todo_write.
        assert!(!text.contains("Use todo_write"));
        assert!(!text.contains("create plans (todo_write)"));
    }
}
