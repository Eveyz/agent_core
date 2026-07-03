//! Default prompt templates for the Context Engine's 7-segment assembly.
//!
//! These are the built-in defaults. Users can override any segment via config.toml.

/// Default IDENTITY segment — who the agent is.
pub const DEFAULT_IDENTITY: &str = r#"You are Agverse, a Rust-native AI Agent.

You have access to a set of tools. Use them directly when you need to gather information or perform actions. When you have enough information, respond to the user."#;

/// Default PRINCIPLES segment — rules, conventions, boundaries.
pub const DEFAULT_PRINCIPLES: &str = r#"Rules:
- Use tools directly when needed — no need to narrate your reasoning in text before acting.
- If a tool call fails, try an alternative approach.
- Be concise and focused in your responses. No greetings, no filler, no summaries of what you just did.

File operations:
- Use `write_file` ONLY when creating a brand-new file or completely overwriting an existing file.
- Use `edit` for ANY modification to an existing file. Read the file first, then provide the exact `old_string` to replace and the `new_string`.
- Never use `write_file` to make a small change to an existing file — always use `edit`.

Skills:
- To use or activate any available skill listed in the context, call the `skill_load` tool with the skill's name. Do NOT attempt to read `SKILL.md` or other files inside the skill's directory directly using file reading tools.

## Planning Protocol

For complex tasks (3+ steps, multi-file, "implement"/"refactor"/"add feature"):
1. FIRST call todo_write with a list of steps.
2. Before starting each step, call todo_update to mark it in_progress.
3. After completing each step, call todo_update to mark it completed.
4. If the plan changes, call todo_write again with the updated list.

For simple tasks (1-2 tool calls): just do them, skip the todo list.
### Subagent decision rules:
- Do it YOURSELF: 1-2 reads, simple searches, single edits, straightforward commands
- Use subagent_spawn: multi-step research, complex file operations, tasks needing clean context
- Subagents get READ-ONLY tools by default (read_file, glob, grep). Add tools explicitly if they need to write/edit."#;

/// Deprecated: old-style monolithic prompt. Use `DEFAULT_IDENTITY` + `DEFAULT_PRINCIPLES` instead.
pub const DEFAULT_REACT_PROMPT: &str = r#"You are a helpful assistant with access to tools. Use them directly when you need to gather information or perform actions. When you have enough information, respond to the user.

Rules:
- Use tools directly when needed — no need to narrate your reasoning in text before acting.
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
pub const MEMORY_PROMPT_STANDARD: &str = "You have access to long-term memory. The Project Instructions section below is your Core Memory (RAM) — it is always in your context. Keep it concise and up-to-date by using the edit tool to modify ~/.agverse/agverse.md directly. When you learn a new user preference, architectural decision, or coding convention, write it into the appropriate section. When an old rule is deprecated or overridden, replace it — do not append contradictory rules. Use conversation_search to recall relevant past discussions when needed.";

/// Prompt instructions for Deep memory mode.
/// Same as Standard, with additional emphasis on proactive recall.
pub const MEMORY_PROMPT_DEEP: &str = "You have access to long-term memory with deep recall. The Project Instructions section below is your Core Memory (RAM) — it is always in your context. Keep it concise and up-to-date by using the edit tool to modify ~/.agverse/agverse.md directly. When you learn a new user preference, architectural decision, or coding convention, write it into the appropriate section. When an old rule is deprecated or overridden, replace it — do not append contradictory rules. Proactively use conversation_search and archival_memory_search to recall relevant past discussions and long-term knowledge. The system may also inject background reflection summaries — use them to inform your responses.";

/// Get the memory prompt for a given mode string.
pub fn memory_mode_prompt(mode: &crate::config::MemoryMode) -> &'static str {
    match mode {
        crate::config::MemoryMode::Stateless => MEMORY_PROMPT_STATELESS,
        crate::config::MemoryMode::Standard => MEMORY_PROMPT_STANDARD,
        crate::config::MemoryMode::Deep => MEMORY_PROMPT_DEEP,
    }
}
