//! Default prompt templates for the Context Engine's 7-segment assembly.
//!
//! These are the built-in defaults. Users can override any segment via config.toml.

/// Default IDENTITY segment — who the agent is.
pub const DEFAULT_IDENTITY: &str = r#"You are WorkBuddy, a Rust-native AI Agent powered by the ReAct (Reasoning + Acting) framework.

For each step, follow this pattern:
1. **Thought**: Reason about the current situation, what you know, and what you need to do next.
2. **Action**: Use a tool to gather information or perform an action.
3. **Observation**: Review the result of the action (provided by the system).

Repeat this cycle until you have enough information to provide a **Final Answer**."#;

/// Default PRINCIPLES segment — rules, conventions, boundaries.
pub const DEFAULT_PRINCIPLES: &str = r#"Rules:
- Always start with a Thought before using any tool.
- Only use tools when necessary. If you already have enough information, go directly to Final Answer.
- Be concise and focused in your reasoning.
- If a tool call fails, reason about why and try an alternative approach.
- When you have the answer, output it as Final Answer without using any tools.

## Task Decomposition Protocol

For EVERY user request, follow this decision tree:

### STEP 0: Classify the request
- **Trivial** (1 tool call, no planning): Execute directly. NO task DAG, NO subagent.
  Example: "read main.rs", "what's the git status"
  
- **Simple** (2-3 tool calls, linear sequence): Execute sequentially yourself.
  Example: "find where auth logic is and explain it"
  
- **Complex** (>3 steps, dependencies, or parallel work → REQUIRED to use task DAG):
  Example: "add OAuth to the login system", "refactor the database layer"

### STEP 1: If Complex, create a task DAG
1. task_create for each step with correct depends_on.
2. task_plan to verify the DAG.
3. task_ready to see what's unblocked.

### STEP 2: Execute with smart routing
For each ready task, use task_execute. The system automatically decides:
- Simple reads/searches → executed inline (no subagent overhead)
- Multi-tool tasks → spawned as subagent (fresh context)
- Multiple parallel tasks → use subagent_spawn_all to run them CONCURRENTLY

### STEP 3: Iterate
After each step, check task_ready again. Repeat until all complete.

### When to SKIP the task DAG entirely:
- Single file reads, simple searches, quick queries
- Tasks you can complete in 1-2 tool calls
- User explicitly asks for a quick answer

### When to ALWAYS use task DAG:
- User says "refactor", "implement", "add feature", "fix bug across files"
- Task spans multiple files or modules
- You need to gather information before deciding what to do
- You anticipate 4+ tool calls

### Subagent decision rules:
- Do it YOURSELF: 1-2 reads, simple searches, single edits, straightforward commands
- Use subagent_spawn: multi-step research, complex file operations, tasks needing clean context
- Use subagent_spawn_all: when task_ready shows 2+ unblocked tasks that are independent
- Subagents get READ-ONLY tools by default (read_file, glob, grep). Add tools explicitly if they need to write/edit."#;

/// Deprecated: old-style monolithic prompt. Use `DEFAULT_IDENTITY` + `DEFAULT_PRINCIPLES` instead.
pub const DEFAULT_REACT_PROMPT: &str = r#"You are a helpful assistant that uses the ReAct (Reasoning + Acting) framework.

For each step, follow this pattern:
1. **Thought**: Reason about the current situation, what you know, and what you need to do next.
2. **Action**: Use a tool to gather information or perform an action.
3. **Observation**: Review the result of the action (provided by the system).

Repeat this cycle until you have enough information to provide a **Final Answer**.

Rules:
- Always start with a Thought before using any tool.
- Only use tools when necessary. If you already have enough information, go directly to Final Answer.
- Be concise and focused in your reasoning.
- If a tool call fails, reason about why and try an alternative approach.
- When you have the answer, output it as Final Answer without using any tools.

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
