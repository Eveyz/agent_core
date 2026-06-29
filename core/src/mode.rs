//! Agent working mode — determines which tools are available to the model.
//!
//! Each [`Run`](crate::runtime::Run) is created in a specific mode that is
//! immutable for the Run's lifetime. Mode changes take effect on the next Run.

use serde::{Deserialize, Serialize};

/// The agent's working mode — controls tool availability.
///
/// - **Ask**: Read-only. Read files, search web, git status/diff/log.
///   No writes, no commands, no plans, no subagents.
/// - **Plan**: Research & plan. Read + todo tools + read-only subagents.
///   No writes, no commands.
/// - **Build**: Full access. All tools available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Read-only Q&A — answer by reading files, searching web.
    Ask,
    /// Research & plan — read, create plans, spawn read-only subagents.
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
            Self::Ask => "\
MODE: Ask — Read-only mode.
You can read files, use git status/diff/log/show, search the web,
search memory, and spawn read-only subagents. You CANNOT write files,
execute commands, or create todo plans.
Answer questions thoroughly by reading source code and documentation.
If the user wants changes made, suggest switching to Plan or Build mode.",

            Self::Plan => "\
MODE: Plan — Research & plan mode.
You can read files, search the web, create plans (todo_write), and spawn
read-only subagents for research. You CANNOT write files or execute
shell commands.
Use todo_write to create detailed execution plans. Use subagents
for multi-step research. When your plan is complete, suggest
switching to Build mode to execute it.",

            Self::Build => "\
MODE: Build — Full access.
You can read, write, edit files, execute shell commands, commit
to git, create plans, and spawn subagents. Use todo_write
for complex multi-step tasks.",
        }
    }

    /// Tool names to remove from the registry for this mode.
    ///
    /// Modes are cumulative: Ask removes everything Plan removes, plus
    /// plan-specific tools.
    pub fn tools_to_remove(&self) -> &'static [&'static str] {
        match self {
            Self::Build => &[],
            Self::Plan => &["write_file", "edit", "sed", "bash", "git_commit"],
            Self::Ask => &[
                "write_file",
                "edit",
                "sed",
                "bash",
                "git_commit",
                "todo_write",
                "todo_update",
                "todo_delete",
                "task",
            ],
        }
    }

    /// Returns true if file modifications and command execution are allowed.
    pub fn is_write_allowed(&self) -> bool {
        matches!(self, Self::Build)
    }
}
