use crate::types::Message;
use tiktoken_rs::cl100k_base;

pub struct Context {
    system_prompt: String,
    core_memory: String,
    messages: Vec<Message>,
    max_tokens: usize,
    tool_result_budget: usize,
    auto_compact_threshold: f64,
}

impl Context {
    pub fn new(system_prompt: &str, max_tokens: usize) -> Self {
        Self {
            system_prompt: system_prompt.to_string(),
            core_memory: String::new(),
            messages: Vec::new(),
            max_tokens,
            tool_result_budget: 4000,
            auto_compact_threshold: 0.8,
        }
    }

    pub fn set_core_memory(&mut self, memory: &str) {
        self.core_memory = memory.to_string();
    }

    pub fn set_max_tokens(&mut self, max: usize) {
        self.max_tokens = max;
    }

    pub fn set_tool_result_budget(&mut self, budget: usize) {
        self.tool_result_budget = budget;
    }

    pub fn add(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn messages(&self) -> Vec<Message> {
        let mut result = Vec::new();

        let mut system_content = self.system_prompt.clone();
        if !self.core_memory.is_empty() {
            system_content.push_str("\n\n== Memory ==\n");
            system_content.push_str(&self.core_memory);
            system_content.push_str("\n== End Memory ==\n");
        }
        result.push(Message::system(&system_content));

        result.extend(self.messages.iter().cloned());
        result
    }

    pub fn current_token_count(&self) -> usize {
        match count_tokens(&self.system_prompt) {
            Ok(count) => {
                let mut total = count;
                for msg in &self.messages {
                    total += message_token_count(msg);
                }
                total
            }
            Err(_) => {
                let mut total = self.system_prompt.len() / 4;
                for msg in &self.messages {
                    total += msg.token_count();
                }
                total
            }
        }
    }

    pub fn trim_to_fit(&mut self) {
        // Layer 1: snip compact - truncate large tool results
        self.snip_compact();

        // Layer 2: auto compact - drop oldest messages if over threshold
        let current = self.current_token_count();
        let threshold = (self.max_tokens as f64 * self.auto_compact_threshold) as usize;
        if current <= threshold {
            return;
        }

        let mut remove_count = 0;
        let mut removed_tokens = 0;
        let target = current - self.max_tokens;

        for msg in &self.messages {
            if removed_tokens >= target {
                break;
            }
            removed_tokens += message_token_count(msg);
            remove_count += 1;
        }

        if remove_count > 0 {
            self.messages.drain(..remove_count);
        }
    }

    fn snip_compact(&mut self) {
        for msg in &mut self.messages {
            if msg.role == crate::types::Role::Tool
                && let Some(ref content) = msg.content
                && content.len() > self.tool_result_budget
            {
                let truncated = format!(
                    "{}\n[... truncated from {} chars]",
                    &content[..self.tool_result_budget],
                    content.len()
                );
                msg.content = Some(truncated);
            }
        }
    }

    pub fn micro_compact(&mut self, keep_recent: usize) -> Option<String> {
        if self.messages.len() <= keep_recent {
            return None;
        }

        let split_point = self.messages.len() - keep_recent;
        let old_messages: Vec<&Message> = self.messages[..split_point].iter().collect();

        let mut summary_parts = Vec::new();
        for msg in &old_messages {
            if let Some(ref content) = msg.content {
                let preview = if content.len() > 200 {
                    let end = content.floor_char_boundary(200);
                    format!("{}...", &content[..end])
                } else {
                    content.clone()
                };
                summary_parts.push(format!("[{}]: {}", msg.role, preview));
            }
        }

        let summary = format!(
            "[Context summary of {} earlier messages]\n{}",
            old_messages.len(),
            summary_parts.join("\n")
        );

        self.messages.drain(..split_point);
        self.messages.insert(0, Message::system(&summary));

        Some(summary)
    }

    pub fn should_auto_compact(&self) -> bool {
        let current = self.current_token_count();
        let threshold = (self.max_tokens as f64 * self.auto_compact_threshold) as usize;
        current > threshold
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

fn count_tokens(text: &str) -> anyhow::Result<usize> {
    let bpe = cl100k_base()?;
    let tokens = bpe.encode_with_special_tokens(text);
    Ok(tokens.len())
}

fn message_token_count(msg: &Message) -> usize {
    let mut count = 4;

    if let Some(ref content) = msg.content {
        match count_tokens(content) {
            Ok(n) => count += n,
            Err(_) => count += content.len() / 4,
        }
    }

    if let Some(ref tool_calls) = msg.tool_calls {
        for tc in tool_calls {
            match count_tokens(&tc.function.name) {
                Ok(n) => count += n,
                Err(_) => count += tc.function.name.len() / 4,
            }
            match count_tokens(&tc.function.arguments) {
                Ok(n) => count += n,
                Err(_) => count += tc.function.arguments.len() / 4,
            }
            count += 10;
        }
    }

    if let Some(ref name) = msg.name {
        match count_tokens(name) {
            Ok(n) => count += n,
            Err(_) => count += name.len() / 4,
        }
    }

    count
}
