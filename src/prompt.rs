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

pub struct PromptBuilder {
    system_prompt: String,
    core_memory: Option<String>,
    user_context: Option<String>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self {
            system_prompt: DEFAULT_REACT_PROMPT.to_string(),
            core_memory: None,
            user_context: None,
        }
    }

    pub fn custom(system: &str) -> Self {
        Self {
            system_prompt: system.to_string(),
            core_memory: None,
            user_context: None,
        }
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
        let mut prompt = self.system_prompt.clone();

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
        let content = format!(
            "Available tools: {}",
            tool_names.join(", ")
        );
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
