//! Default prompt templates for the Context Engine's 7-segment assembly.
//!
//! These are the built-in defaults. Users can override any segment via config.toml.

/// Default IDENTITY segment — who the agent is.
pub const DEFAULT_IDENTITY: &str = r#"You are Agverse, a Rust-native AI Agent.

You have access to a set of tools, including Model Context Protocol (MCP) tools. MCP tools are prefixed with `mcp__<server_name>__<tool_name>` (for example, `mcp__parallel-search__search`). Use them directly just like native tools to perform external actions (such as searching the web) when needed. When you have enough information, respond to the user."#;

/// Default PRINCIPLES segment — rules, conventions, boundaries.
pub const DEFAULT_PRINCIPLES: &str = r#"Rules:
- **Progress narration (content channel):** On multi-step work, before tool calls write at most 1–2 short sentences in plain text: what you will do next (user-facing). Examples: "Next I'll read the Option constructors." / "Checking why those rows are nan, then fixing the harness." Do NOT put analysis, root-cause essays, step-by-step reasoning, or post-mortems in content — that belongs in the thinking/reasoning channel only. Skip narration for trivial single-tool actions.
- If a tool call fails, try an alternative approach.
- Be concise and focused in your responses. No greetings, no filler, no summaries of what you just did.
- Identify the active repo from Working Directory (and Project Instructions for that cwd) before applying any project-specific knowledge from Global Memory. Global Memory may list multiple user projects as a catalog — never assume a catalog entry is the current project unless cwd matches.

File operations:
- Use `write_file` ONLY when creating a brand-new file or completely overwriting an existing file.
- Use `edit` for ANY modification to an existing file. Read the file first, then provide the exact `old_string` to replace and the `new_string`.
- Never use `write_file` to make a small change to an existing file — always use `edit`.
- **Batch reads**: When you need to read multiple independent files, issue ALL `read_file` calls in a single response. Do not read files one at a time across multiple turns. The system runs independent tool calls in parallel, so one turn with N reads costs the same as one turn with 1 read — but N sequential turns cost N× more.

Skills:
- Inactive skills: activate with `skill_load` or `@skill:name` / auto-trigger. Do not browse skill directories to discover `SKILL.md`.
- Active skills: their body is already in context. Follow it. Use `read_file` on paths under `Skill directory` / `### Skill assets`. Do not shell-`find` or glob the skill tree. Do not call `skill_load` again for skills marked `[ACTIVE]`.

## Clarification Protocol (before acting)

When the user's request is ambiguous, underspecified, or has multiple valid interpretations — especially under `/goal`, or before creating a plan — call `ask_user` FIRST with 1–4 concrete multiple-choice questions (single- or multi-select).

Do NOT call `todo_write`, `write_file`/`edit`/`shell`, or other mutating tools until success criteria and scope are clear. Do not ask in plain assistant text and end the turn — use `ask_user` so execution waits for answers. Prefer clarifying before planning; then plan and act on the next turn using the answers.

Under `/goal`: never produce a generic advice essay and stop. Either clarify (`ask_user`), plan (`todo_write`), or execute the next concrete step. If a plan already exists with pending items, keep working those items with tools.

## Planning Protocol

Default: act immediately with tools. Do NOT call todo_write for simple work
(1–2 tool calls, single-file edit, quick lookup/question, or a short command).
Do not invent a multi-step plan just to track trivial reads/edits.

Only after requirements are clear, create a todo plan when the task is clearly multi-step, e.g.:
- 3+ distinct steps that must stay ordered across turns
- coordinated multi-file changes
- feature / refactor / migration spanning many files
- `/goal`, or the user asked for an explicit plan

When a plan is warranted:
1. FIRST call todo_write with a list of concrete steps.
2. Before starting each step, call todo_update to mark it in_progress.
3. After completing each step, call todo_update to mark it completed (auto-advances the next step).
4. If the plan must change, call todo_write again (progress is merged by default). Pass force=true only to wipe statuses.
5. Do NOT replan every turn — follow the runtime NEXT step and advance with tools.
### Subagent decision rules:
- Do it YOURSELF: 1-2 reads, simple searches, single edits, straightforward commands
- Use subagent_spawn: multi-step research, complex file operations, tasks needing clean context
- Subagents get READ-ONLY tools by default (read_file, glob, grep). Add tools explicitly if they need to write/edit."#;

/// Memory protocol — appended to Principles when memory is enabled (Stable segment).
pub const MEMORY_PROTOCOL: &str = r#"## Memory Protocol

Before answering, decide if the question needs historical context:
- Past conversations, user preferences, prior decisions → call `conversation_search` FIRST
- Long-term facts and distilled knowledge → call `archival_memory_search`
- Current project rules → already in Project Instructions (cwd) below; do NOT search for those
- Current codebase → use `grep` / `read_file`; do NOT use memory search
- Active project identity → resolve from Working Directory + Project Instructions (cwd). Global Memory is cross-project background only.

When search returns nothing relevant, say so — do not invent past context.

When learning durable facts, use the correct store:
- Cross-project personal traits (name, habits, language) → `core_memory_append` on block `human`
- Agent persona adjustments → `core_memory_append` on block `persona`
- Project architecture, conventions, decisions → `edit` on the project-local `agverse.md` (cwd), not global catalog entries
- Cross-project catalog / global preferences → `edit` on `~/.agverse/agverse.md`
- Important but verbose history → `archival_memory_insert`"#;

/// Deprecated: old-style monolithic prompt. Use `DEFAULT_IDENTITY` + `DEFAULT_PRINCIPLES` instead.
pub const DEFAULT_REACT_PROMPT: &str = r#"You are a helpful assistant with access to tools. Use them directly when you need to gather information or perform actions. When you have enough information, respond to the user.

Rules:
- **Progress narration:** On multi-step work, before tools write at most 1–2 short user-facing sentences about the next action. Keep analysis and chain-of-thought in the thinking/reasoning channel — never dump long reasoning into content. Skip narration for trivial single-tool actions.
- If a tool call fails, try an alternative approach.
- Be concise and focused in your responses.

Delegation:
- Handle simple tasks yourself — read a file, run a command, search code. No delegation needed.
- Use subagent_spawn only when a task benefits from isolation: fresh context without conversation noise, a different specialization, or when you want to keep your own context clean for a larger orchestration role.
- If you're unsure, do it yourself first. Delegate only when the task is complex enough to warrant a separate agent loop.

Task DAG Execution (for multi-step plans):
1. Create the plan: task_create each step with depends_on for ordering.
2. Review the plan: task_plan to see the DAG and execution order.
3. Execute leaf tasks: task_ready shows what's unblocked. Use task_execute to run each.
4. Results flow forward: when a task completes, its result is auto-injected into dependents.
5. Iterate: after each execution, check task_ready again for newly unblocked tasks.
6. Handle failures: if a task fails, dependent tasks become blocked. Reason about whether to retry or adjust the plan."#;

// ── Backward-compatible PromptBuilder (kept for existing API consumers) ──

/// Legacy PromptBuilder — now delegates to ContextEngine internally.
/// Kept for backward compatibility. New code should use `ContextEngine` directly.
pub struct PromptBuilder {
    identity: String,
    principles: Option<String>,
    core_memory: Option<String>,
    user_context: Option<String>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self {
            identity: DEFAULT_IDENTITY.to_string(),
            principles: None,
            core_memory: None,
            user_context: None,
        }
    }

    pub fn custom(system: &str) -> Self {
        Self {
            identity: system.to_string(),
            principles: None,
            core_memory: None,
            user_context: None,
        }
    }

    pub fn with_principles(mut self, principles: &str) -> Self {
        self.principles = Some(principles.to_string());
        self
    }

    pub fn with_core_memory(mut self, memory: &str) -> Self {
        self.core_memory = Some(memory.to_string());
        self
    }

    pub fn with_context(mut self, ctx: &str) -> Self {
        self.user_context = Some(ctx.to_string());
        self
    }

    pub fn build(&self) -> String {
        let mut prompt = self.identity.clone();

        if let Some(ref p) = self.principles {
            prompt.push_str("\n\n== Principles ==\n");
            prompt.push_str(p);
        }

        if let Some(ref memory) = self.core_memory {
            prompt.push_str("\n\n== Memory ==\n");
            prompt.push_str(memory);
            prompt.push_str("\n== End Memory ==\n");
        }

        if let Some(ref ctx) = self.user_context {
            prompt.push_str("\n\n== Context ==\n");
            prompt.push_str(ctx);
            prompt.push_str("\n== End Context ==\n");
        }

        prompt
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Legacy PromptAssembler (kept for backward compat) ──

#[derive(Debug, Clone)]
pub struct PromptSection {
    pub name: String,
    pub content: String,
    pub priority: u8,
}

pub struct PromptAssembler {
    sections: Vec<PromptSection>,
}

impl PromptAssembler {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    pub fn add_section(&mut self, name: &str, content: &str, priority: u8) {
        self.sections.push(PromptSection {
            name: name.to_string(),
            content: content.to_string(),
            priority,
        });
    }

    pub fn add_core(&mut self, prompt: &str) {
        self.add_section("core", prompt, 0);
    }

    pub fn add_tools(&mut self, tool_names: &[&str]) {
        let content = format!("Available tools: {}", tool_names.join(", "));
        self.add_section("tools", &content, 10);
    }

    pub fn add_memory(&mut self, memory: &str) {
        if !memory.is_empty() {
            self.add_section("memory", memory, 20);
        }
    }

    pub fn add_skills(&mut self, skill_content: &str) {
        if !skill_content.is_empty() {
            self.add_section("skills", skill_content, 30);
        }
    }

    pub fn add_user_context(&mut self, context: &str) {
        if !context.is_empty() {
            self.add_section("user_context", context, 40);
        }
    }

    pub fn add_todo(&mut self, todo_context: &str) {
        if !todo_context.is_empty() {
            self.add_section("todo", todo_context, 15);
        }
    }

    pub fn assemble(&self) -> String {
        let mut sorted: Vec<&PromptSection> = self.sections.iter().collect();
        sorted.sort_by_key(|s| s.priority);

        let mut parts = Vec::new();
        for section in sorted {
            parts.push(format!(
                "== {} ==\n{}\n== End {} ==\n",
                section.name, section.content, section.name
            ));
        }

        parts.join("\n")
    }

    pub fn clear(&mut self) {
        self.sections.clear();
    }

    pub fn remove_section(&mut self, name: &str) {
        self.sections.retain(|s| s.name != name);
    }
}

impl Default for PromptAssembler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Memory Mode Prompts ───────────────────────────────────────────────

/// Prompt instructions for Stateless memory mode.
/// Injected into the Active Memory segment when memory is disabled.
pub const MEMORY_PROMPT_STATELESS: &str = "You have no memory of previous conversations. Do not assume context from prior sessions. Each conversation starts fresh.";

/// Prompt instructions for Standard memory mode.
/// Injected into the Active Memory segment alongside agverse.md.
pub const MEMORY_PROMPT_STANDARD: &str = "You have long-term memory. Core Memory blocks (human/persona), Global Memory (cross-project catalog), and Project Instructions (cwd) are always in context. Resolve the active repo from Working Directory first. Use `core_memory_*` for short cross-project traits; use `edit` on project-local agverse.md for project-specific knowledge. Call `conversation_search` when a question may depend on past discussions.";

/// Prompt instructions for Deep memory mode.
/// Same as Standard, with additional emphasis on proactive recall.
pub const MEMORY_PROMPT_DEEP: &str = "You have deep long-term memory. Core Memory blocks, Global Memory (cross-project), Project Instructions (cwd), and relevant past conversations may be pre-injected below. Resolve the active repo from Working Directory first. Use `core_memory_*` for personal traits, `edit` on project-local agverse.md for project rules, `archival_memory_search` for long-term facts. Proactively search when unsure. Background reflection may update agverse.md between turns.";

/// Get the memory prompt for a given mode string.
pub fn memory_mode_prompt(mode: &crate::config::MemoryMode) -> &'static str {
    match mode {
        crate::config::MemoryMode::Stateless => MEMORY_PROMPT_STATELESS,
        crate::config::MemoryMode::Standard => MEMORY_PROMPT_STANDARD,
        crate::config::MemoryMode::Deep => MEMORY_PROMPT_DEEP,
    }
}
