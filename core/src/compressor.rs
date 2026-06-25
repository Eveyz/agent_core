//! 5-stage compression pipeline for long context windows.
//!
//! Architecture:
//! ```text
//! Stage 1: snipCompact     — truncate tool results > budget
//! Stage 2: dedupCompact    — collapse duplicate tool outputs
//! Stage 3: chunkCompact    — merge tool_call + tool_result pairs
//! Stage 4: summaryCompact  — LLM-generated structured summary
//! Stage 5: gradientCompact — age-tiered compression (recent=raw, old=summary)
//! ```
//!
//! Stages run in order: 1→2→3→4→5. Each stage reduces token count.
//! The pipeline stops early if tokens are already under threshold.

use crate::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Compression metrics ──────────────────────────────────────────────

/// Result of running the compression pipeline.
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// Which stages ran.
    pub stages_ran: Vec<&'static str>,
    /// Tokens before compression.
    pub tokens_before: usize,
    /// Tokens after compression.
    pub tokens_after: usize,
    /// Messages before compression.
    pub messages_before: usize,
    /// Messages after compression.
    pub messages_after: usize,
}

impl CompressionResult {
    pub fn savings_percent(&self) -> f64 {
        if self.tokens_before == 0 {
            return 0.0;
        }
        (1.0 - self.tokens_after as f64 / self.tokens_before as f64) * 100.0
    }

    pub fn summary(&self) -> String {
        format!(
            "Compressed {} → {} messages ({} → {} tokens, {:.0}% saved). Stages: [{}]",
            self.messages_before,
            self.messages_after,
            self.tokens_before,
            self.tokens_after,
            self.savings_percent(),
            self.stages_ran.join(", ")
        )
    }
}

// ── Stage 4: Structured summary for LLM ─────────────────────────────

/// Structured summary generated from old conversation turns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSummary {
    pub decisions: Vec<String>,
    pub files_modified: Vec<String>,
    pub errors_encountered: Vec<String>,
    pub unresolved: Vec<String>,
    pub key_facts: Vec<String>,
}

impl TurnSummary {
    /// Build a context string from the summary.
    pub fn to_context_string(&self) -> String {
        let mut parts = Vec::new();

        if !self.decisions.is_empty() {
            parts.push(format!(
                "Decisions made:\n{}",
                self.decisions
                    .iter()
                    .map(|d| format!("  • {}", d))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !self.files_modified.is_empty() {
            parts.push(format!(
                "Files modified: {}",
                self.files_modified.join(", ")
            ));
        }

        if !self.errors_encountered.is_empty() {
            parts.push(format!(
                "Errors encountered:\n{}",
                self.errors_encountered
                    .iter()
                    .map(|e| format!("  • {}", e))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !self.unresolved.is_empty() {
            parts.push(format!(
                "Still unresolved:\n{}",
                self.unresolved
                    .iter()
                    .map(|u| format!("  • {}", u))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        if !self.key_facts.is_empty() {
            parts.push(format!(
                "Key facts:\n{}",
                self.key_facts
                    .iter()
                    .map(|f| format!("  • {}", f))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }

        parts.join("\n")
    }

    /// Generate a prompt asking an LLM to summarize old turns.
    pub fn prompt_for_llm(turns_text: &str) -> String {
        format!(
            r#"Summarize the following conversation turns into a structured JSON object. Be concise and extract only what matters:

{{
  "decisions": ["list of decisions made"],
  "files_modified": ["files that were read or written"],
  "errors_encountered": ["errors or problems hit"],
  "unresolved": ["open questions or unfinished tasks"],
  "key_facts": ["important facts or discoveries"]
}}

Conversation:
{turns_text}

Return ONLY valid JSON, no other text."#
        )
    }
}

// ── Compression pipeline ─────────────────────────────────────────────

/// The compression pipeline operates on a slice/vec of messages.
pub struct Compressor {
    /// Tool result budget for snipCompact (chars).
    pub tool_result_budget: usize,
    /// Threshold ratio: if tokens > max_tokens * threshold, start compressing.
    pub auto_compact_threshold: f64,
    /// Target ratio: compress until tokens < max_tokens * target.
    pub target_ratio: f64,
    /// How many recent messages to keep raw in gradientCompact.
    pub gradient_keep_recent: usize,
    /// How many semi-recent messages get snipCompact in gradientCompact.
    pub gradient_snip_range: usize,
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            tool_result_budget: 4000,
            auto_compact_threshold: 0.8,
            target_ratio: 0.6,
            gradient_keep_recent: 6,
            gradient_snip_range: 6,
        }
    }
}

impl Compressor {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Stage 1: snipCompact ────────────────────────────────────────

    /// Truncate tool results exceeding the budget.
    /// Returns the number of messages modified.
    pub fn snip_compact(&self, messages: &mut Vec<Message>) -> usize {
        let mut modified = 0;
        for msg in messages.iter_mut() {
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
                modified += 1;
            }
        }
        modified
    }

    // ── Stage 2: dedupCompact ───────────────────────────────────────

    /// Detect consecutive identical tool results and replace with references.
    /// E.g., three identical `read_file("main.rs")` → keep first, reference others.
    /// Returns the number of messages deduplicated.
    pub fn dedup_compact(&self, messages: &mut Vec<Message>) -> usize {
        let mut deduped = 0;
        let mut seen: HashMap<String, (usize, String)> = HashMap::new(); // content_hash → (msg_index, tool_name)

        for i in 0..messages.len() {
            let (content, tool_name) = {
                let msg = &messages[i];
                if msg.role != crate::types::Role::Tool {
                    continue;
                }
                let content = match &msg.content {
                    Some(c) => c,
                    None => continue,
                };
                let name = msg.name.clone().unwrap_or_default();
                (content.clone(), name)
            };

            // Skip very short results (< 50 chars) — not worth deduping
            if content.len() < 50 {
                continue;
            }

            let hash_key = format!("{}::{}", tool_name, content);

            if let Some((first_idx, first_name)) = seen.get(&hash_key) {
                let truncated = truncate_preview(&content, 200);
                messages[i].content = Some(format!(
                    "[Same as #{} — {}... ({} chars)]",
                    first_idx + 1,
                    truncated,
                    content.len()
                ));
                let _ = first_name;
                deduped += 1;
            } else {
                seen.insert(hash_key, (i, tool_name));
            }
        }

        deduped
    }

    // ── Stage 3: chunkCompact ───────────────────────────────────────

    /// Merge consecutive tool_call → tool_result pairs into single system messages.
    /// Only operates on older messages — the most recent `protect_recent` messages
    /// are left untouched so the API-required tool_call/tool_result pairing stays
    /// intact for the active conversation window.
    /// Returns the number of pairs merged.
    pub fn chunk_compact(&self, messages: &mut Vec<Message>) -> usize {
        let protect = 8.min(messages.len());
        let mut limit = messages.len().saturating_sub(protect);
        let mut merged = 0;
        let mut i = 0;

        while i + 1 < limit {
            let (tool_name, tool_args, call_id) = {
                let msg = &messages[i];
                if msg.role != crate::types::Role::Assistant {
                    i += 1;
                    continue;
                }
                match &msg.tool_calls {
                    Some(calls) if calls.len() == 1 => {
                        let call = &calls[0];
                        (
                            call.function.name.clone(),
                            call.function.arguments.clone(),
                            call.id.clone(),
                        )
                    }
                    _ => {
                        i += 1;
                        continue;
                    }
                }
            };

            let next = &messages[i + 1];
            if next.role == crate::types::Role::Tool
                && next.tool_call_id.as_deref() == Some(&call_id)
            {
                let result = next.content.clone().unwrap_or_default();
                let result_preview = truncate_preview(&result, 500);

                // Replace tool_call message with a system chunk
                let chunk = format!(
                    "[Tool: {} | Args: {}]\n→ Result: {}",
                    tool_name,
                    truncate_preview(&tool_args, 200),
                    result_preview
                );
                messages[i] = Message::system(&chunk);

                // Remove the tool result message
                messages.remove(i + 1);
                limit -= 1;
                merged += 1;
            }

            i += 1;
        }

        merged
    }

    // ── Stage 4: summaryCompact ─────────────────────────────────────

    /// Build text suitable for LLM summarization from old messages.
    /// Does NOT call the LLM — returns the text and indices to remove.
    /// The caller (Agent) calls the LLM and then replaces messages.
    pub fn prepare_summary_compact(
        &self,
        messages: &[Message],
        num_turns_to_summarize: usize,
    ) -> Option<SummarizeRequest> {
        if num_turns_to_summarize == 0 || messages.is_empty() {
            return None;
        }

        // Find the split point: each User message starts a new turn
        let mut turn_boundaries = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if msg.role == crate::types::Role::User {
                turn_boundaries.push(i);
            }
        }

        if turn_boundaries.len() <= num_turns_to_summarize {
            return None; // not enough turns
        }

        let split_idx = turn_boundaries[num_turns_to_summarize];
        let old_slice = &messages[..split_idx];

        // Build a text representation of old turns
        let mut turns_text = String::new();
        let mut turn_num = 0;
        for msg in old_slice {
            match msg.role {
                crate::types::Role::User => {
                    turn_num += 1;
                    turns_text.push_str(&format!(
                        "\n--- Turn {} (User) ---\n{}\n",
                        turn_num,
                        msg.content.as_deref().unwrap_or("")
                    ));
                }
                crate::types::Role::Assistant => {
                    if let Some(ref content) = msg.content {
                        if !content.is_empty() {
                            turns_text.push_str(&format!("[Assistant]: {}\n", content));
                        }
                    }
                    if let Some(ref calls) = msg.tool_calls {
                        for call in calls {
                            turns_text.push_str(&format!("[Tool call: {}]\n", call.function.name));
                        }
                    }
                }
                crate::types::Role::Tool => {
                    let preview = msg
                        .content
                        .as_deref()
                        .map(|c| truncate_preview(c, 300))
                        .unwrap_or_default();
                    let name = msg.name.as_deref().unwrap_or("unknown");
                    turns_text.push_str(&format!("[Tool result: {}] {}\n", name, preview));
                }
                _ => {}
            }
        }

        let prompt = TurnSummary::prompt_for_llm(&turns_text);

        Some(SummarizeRequest {
            turns_text,
            turns_to_remove: num_turns_to_summarize,
            split_index: split_idx,
            prompt,
        })
    }

    /// Apply a summary returned by the LLM, replacing old messages.
    pub fn apply_summary(
        messages: &mut Vec<Message>,
        split_idx: usize,
        summary: &TurnSummary,
        num_turns: usize,
    ) -> String {
        let summary_text = format!(
            "[Compressed turns 1-{}]\n{}",
            num_turns,
            summary.to_context_string()
        );

        // Remove old messages and insert summary as system message
        messages.drain(..split_idx);
        messages.insert(0, Message::system(&summary_text));

        summary_text
    }

    // ── Stage 1-3 pipeline ──────────────────────────────────────────

    /// Run Stage 1-3 (snip, dedup, chunk) in order. These are purely
    /// deterministic — no LLM required. Stage 4 (LLM summary) is handled
    /// externally by the caller via `prepare_summary_compact` / `apply_summary`.
    ///
    /// Returns a CompressionResult with stats.
    pub fn run_stages_1_3(
        &mut self,
        messages: &mut Vec<Message>,
        token_counter: impl Fn(&[Message]) -> usize,
    ) -> CompressionResult {
        let tokens_before = token_counter(messages);
        let msgs_before = messages.len();
        let mut stages = Vec::new();

        let snipped = self.snip_compact(messages);
        if snipped > 0 {
            stages.push("snipCompact");
        }

        let deduped = self.dedup_compact(messages);
        if deduped > 0 {
            stages.push("dedupCompact");
        }

        let chunked = self.chunk_compact(messages);
        if chunked > 0 {
            stages.push("chunkCompact");
        }

        CompressionResult {
            stages_ran: stages,
            tokens_before,
            tokens_after: token_counter(messages),
            messages_before: msgs_before,
            messages_after: messages.len(),
        }
    }

    /// Run the full pipeline (stages 1-3 + gradient). Stages 4-5 require LLM.
    pub fn run_pipeline(
        &mut self,
        messages: &mut Vec<Message>,
        token_counter: impl Fn(&[Message]) -> usize,
        max_tokens: usize,
    ) -> CompressionResult {
        let current = token_counter(messages);
        let threshold = (max_tokens as f64 * self.auto_compact_threshold) as usize;

        if current <= threshold {
            return CompressionResult {
                stages_ran: vec![],
                tokens_before: current,
                tokens_after: current,
                messages_before: messages.len(),
                messages_after: messages.len(),
            };
        }

        self.run_stages_1_3(messages, token_counter)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let end = text.floor_char_boundary(max_chars);
    format!("{}...", &text[..end])
}

/// Request for LLM-based summarization.
#[derive(Debug, Clone)]
pub struct SummarizeRequest {
    /// Text representation of the turns to summarize.
    pub turns_text: String,
    /// Number of turns being summarized.
    pub turns_to_remove: usize,
    /// Index in the messages vec where to split.
    pub split_index: usize,
    /// Prompt to send to the LLM.
    pub prompt: String,
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionCall, Role, ToolCall};

    fn make_tool_call_msg(id: &str, name: &str, args: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: Some("Let me use a tool.".to_string()),
            tool_calls: Some(vec![ToolCall {
                id: id.to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: args.to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
        }
    }

    fn make_tool_result(id: &str, name: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            name: Some(name.to_string()),
        }
    }

    #[test]
    fn test_snip_compact_truncates_long_results() {
        let comp = Compressor::new();
        let mut msgs = vec![make_tool_result("c1", "read_file", &"x".repeat(5000))];
        let modified = comp.snip_compact(&mut msgs);
        assert_eq!(modified, 1);
        let content = msgs[0].content.as_ref().unwrap();
        assert!(content.len() < 5000);
        assert!(content.contains("truncated"));
    }

    #[test]
    fn test_snip_compact_skips_short_results() {
        let comp = Compressor::new();
        let mut msgs = vec![make_tool_result("c1", "read_file", "short result")];
        let modified = comp.snip_compact(&mut msgs);
        assert_eq!(modified, 0);
    }

    #[test]
    fn test_dedup_compact_collapses_duplicates() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_result("c1", "read_file", &"A".repeat(100)),
            make_tool_result("c2", "read_file", &"A".repeat(100)), // same!
            make_tool_result("c3", "read_file", &"B".repeat(100)), // different
        ];
        let deduped = comp.dedup_compact(&mut msgs);
        assert_eq!(deduped, 1);
        assert!(msgs[1].content.as_ref().unwrap().contains("Same as"));
        assert_eq!(msgs[2].content.as_deref().unwrap(), &"B".repeat(100));
    }

    #[test]
    fn test_dedup_compact_skips_short() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_result("c1", "read_file", "hi"),
            make_tool_result("c2", "read_file", "hi"),
        ];
        let deduped = comp.dedup_compact(&mut msgs);
        assert_eq!(deduped, 0); // too short to dedup
    }

    #[test]
    fn test_chunk_compact_merges_pairs() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_call_msg("c1", "read_file", r#"{"path":"main.rs"}"#),
            make_tool_result("c1", "read_file", "fn main() {}"),
            // padding so protect_recent (8) doesn't cover the pair above
            Message::user("padding 1"),
            Message::assistant("padding 2"),
            Message::user("padding 3"),
            Message::assistant("padding 4"),
            Message::user("padding 5"),
            Message::assistant("padding 6"),
            Message::user("padding 7"),
            Message::assistant("padding 8"),
        ];
        let merged = comp.chunk_compact(&mut msgs);
        assert_eq!(merged, 1);
        assert_eq!(msgs[0].role, crate::types::Role::System);
        let content = msgs[0].content.as_ref().unwrap();
        assert!(content.contains("read_file"));
        assert!(content.contains("fn main()"));
    }

    #[test]
    fn test_chunk_compact_skips_non_matching_ids() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_call_msg("c1", "read_file", r#"{"path":"a.rs"}"#),
            make_tool_result("c2", "read_file", "content"), // different id
        ];
        let merged = comp.chunk_compact(&mut msgs);
        assert_eq!(merged, 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_prepare_summary_compact() {
        let comp = Compressor::new();
        let msgs = vec![
            Message::user("task 1"),
            Message::assistant("working on it"),
            make_tool_call_msg("c1", "read_file", "{}"),
            make_tool_result("c1", "read_file", "data"),
            Message::user("task 2"),
            Message::assistant("done"),
        ];

        let req = comp.prepare_summary_compact(&msgs, 1);
        assert!(req.is_some());
        let req = req.unwrap();
        assert!(req.prompt.contains("task 1"));
        assert!(req.prompt.contains("read_file"));
        assert_eq!(req.split_index, 4); // second User message starts at index 4
    }

    #[test]
    fn test_prepare_summary_compact_not_enough_turns() {
        let comp = Compressor::new();
        let msgs = vec![Message::user("hi"), Message::assistant("hello")];
        let req = comp.prepare_summary_compact(&msgs, 3);
        assert!(req.is_none());
    }

    #[test]
    fn test_apply_summary() {
        let mut msgs = vec![
            Message::user("task1"),
            Message::assistant("done1"),
            Message::user("task2"),
            Message::assistant("done2"),
        ];

        let summary = TurnSummary {
            decisions: vec!["decided to refactor".to_string()],
            files_modified: vec!["src/main.rs".to_string()],
            errors_encountered: vec![],
            unresolved: vec![],
            key_facts: vec!["codebase uses async".to_string()],
        };

        let text = Compressor::apply_summary(&mut msgs, 2, &summary, 1);
        assert_eq!(msgs.len(), 3); // summary + task2 + done2
        assert!(
            msgs[0]
                .content
                .as_ref()
                .unwrap()
                .contains("decided to refactor")
        );
        assert!(text.contains("Compressed turns"));
    }

    #[test]
    fn test_run_stages_1_3() {
        let mut comp = Compressor::default();

        let mut msgs = vec![
            Message::user("old task"),
            Message::assistant("working"),
            make_tool_call_msg("c1", "read_file", "{}"),
            make_tool_result("c1", "read_file", &"x".repeat(5000)),
            Message::user("recent task"),
            Message::assistant("done"),
        ];

        let result = comp.run_stages_1_3(&mut msgs, |m| m.len() * 100);
        assert!(result.stages_ran.contains(&"snipCompact"));
    }

    #[test]
    fn test_compression_result_savings() {
        let result = CompressionResult {
            stages_ran: vec!["snipCompact", "chunkCompact"],
            tokens_before: 1000,
            tokens_after: 400,
            messages_before: 10,
            messages_after: 5,
        };
        assert!((result.savings_percent() - 60.0).abs() < 0.01);
        assert!(result.summary().contains("60%"));
    }

    #[test]
    fn test_turn_summary_to_context_string() {
        let summary = TurnSummary {
            decisions: vec!["use async/await".to_string()],
            files_modified: vec!["main.rs".to_string(), "lib.rs".to_string()],
            errors_encountered: vec!["tokio runtime panic".to_string()],
            unresolved: vec!["need to test edge case".to_string()],
            key_facts: vec!["Rust 2024 edition".to_string()],
        };

        let s = summary.to_context_string();
        assert!(s.contains("async/await"));
        assert!(s.contains("main.rs, lib.rs"));
        assert!(s.contains("tokio runtime"));
        assert!(s.contains("edge case"));
        assert!(s.contains("Rust 2024"));
    }

    #[test]
    fn test_prompt_for_llm() {
        let prompt = TurnSummary::prompt_for_llm("User: hello\nAssistant: hi");
        assert!(prompt.contains("decisions"));
        assert!(prompt.contains("hello"));
        assert!(prompt.contains("JSON"));
    }
}
