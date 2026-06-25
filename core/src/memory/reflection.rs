//! Background reflection daemon — extracts durable facts from conversation
//! using a low-cost LLM and writes them to archival memory.
//!
//! Only active in Deep mode when `reflection_model` is configured.
//! Runs as a detached `tokio::spawn` task, surviving across Runs.
//! Communicates via a non-blocking mpsc channel.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::client::OpenAIClient;
use crate::memory::MemoryManager;
use crate::types::Message;

/// A single conversation message sent to the daemon.
struct ConversationSlice {
    role: String,
    content: String,
}

/// Handle returned to the caller — just a sender.
/// When dropped, the background task exits gracefully.
pub struct ReflectionDaemon {
    sender: mpsc::Sender<ConversationSlice>,
}

impl ReflectionDaemon {
    /// Spawn the background reflection task.
    /// Returns a handle for feeding conversation slices.
    pub fn spawn(
        client: OpenAIClient,
        memory: Arc<Mutex<MemoryManager>>,
        trigger_count: usize,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<ConversationSlice>(200);

        tokio::spawn(async move {
            let mut buffer: Vec<ConversationSlice> = Vec::new();

            while let Some(slice) = receiver.recv().await {
                if should_skip(&slice.role, &slice.content) {
                    continue;
                }

                buffer.push(slice);

                if buffer.len() >= trigger_count {
                    let slices = std::mem::take(&mut buffer);
                    if let Err(e) = run_reflection(&client, &memory, &slices).await {
                        tracing::warn!("reflection failed: {e}");
                    }
                }
            }

            // Channel closed — drain remaining buffer
            if !buffer.is_empty() {
                if let Err(e) = run_reflection(&client, &memory, &buffer).await {
                    tracing::warn!("final reflection failed: {e}");
                }
            }
        });

        Self { sender }
    }

    /// Non-blocking send. Silently drops if channel is full.
    pub fn try_send(&self, role: &str, content: &str) {
        let _ = self.sender.try_send(ConversationSlice {
            role: role.to_string(),
            content: content.to_string(),
        });
    }
}

/// Pre-filter: skip messages that have no durable value.
fn should_skip(role: &str, content: &str) -> bool {
    // Too short
    let word_count = content.split_whitespace().count();
    if word_count < 5 {
        return true;
    }

    // Tool output: pure stdout / exit codes
    if role == "tool" {
        if content.starts_with("stdout:")
            || content.starts_with("exit code")
            || content.starts_with("Output:")
        {
            return true;
        }
    }

    // Pure error stack traces
    if (content.contains("panic:") || content.contains("Traceback"))
        && content.lines().count() > 10
    {
        return true;
    }

    false
}

/// Call the LLM to extract facts, then write to archival memory.
async fn run_reflection(
    client: &OpenAIClient,
    memory: &Arc<Mutex<MemoryManager>>,
    slices: &[ConversationSlice],
) -> Result<()> {
    let conversation_text = slices
        .iter()
        .map(|s| format!("[{}]: {}", s.role, s.content))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = build_extraction_prompt(&conversation_text);
    let messages = vec![Message::system(&prompt)];

    let (response, _) = client
        .chat_completion(&messages, &[])
        .await
        .context("reflection LLM call failed")?;

    let facts = parse_facts(&response);

    if facts.is_empty() {
        tracing::debug!("reflection: no new facts extracted");
        return Ok(());
    }

    tracing::info!("reflection: extracted {} facts", facts.len());

    let mem = memory.lock();
    for fact in &facts {
        let metadata = serde_json::json!({
            "source": "reflection",
            "extracted_from": format!("{} messages", slices.len()),
        })
        .to_string();

        if let Err(e) = mem.archival().insert(fact, Some(&metadata)) {
            tracing::warn!("reflection: failed to store fact: {e}");
        }
    }

    Ok(())
}

fn build_extraction_prompt(conversation: &str) -> String {
    format!(
        r#"You are a memory extractor. Read the conversation below and extract ONLY new, durable facts.

Rules:
- Extract: user preferences, decisions, conventions, architecture decisions, tool choices.
- Do NOT extract: trivia, greetings, transient context, questions, or facts already obvious from the project setup.
- If no new facts, respond with exactly: {{"facts":[]}}
- If facts exist, respond as JSON: {{"facts":["fact 1","fact 2",...]}}
- Each fact should be a single concise sentence.

Conversation:
{conversation}"#
    )
}

/// Parse the LLM response into a list of facts.
fn parse_facts(response: &str) -> Vec<String> {
    // Try to parse as JSON
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(response) {
        if let Some(facts) = parsed["facts"].as_array() {
            return facts
                .iter()
                .filter_map(|f| f.as_str().map(|s| s.to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    // Fallback: if the response looks like it contains no facts marker
    let trimmed = response.trim();
    if trimmed == "{}" || trimmed.is_empty() {
        return Vec::new();
    }

    // Last resort: treat the whole response as a single fact if it's short
    if trimmed.len() < 200 && !trimmed.starts_with('{') {
        return vec![trimmed.to_string()];
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip_short() {
        assert!(should_skip("user", "ok"));
        assert!(should_skip("user", "hi there"));
        assert!(!should_skip("user", "I prefer using Rust for all backend work"));
    }

    #[test]
    fn test_should_skip_tool_output() {
        assert!(should_skip("tool", "stdout: hello world output here"));
        assert!(should_skip("tool", "exit code: 0"));
        assert!(!should_skip("tool", "The build succeeded with warnings about unused imports in main.rs"));
    }

    #[test]
    fn test_should_skip_error_stack() {
        let stack = "panic: thread crashed\n".repeat(15);
        assert!(should_skip("assistant", &stack));
        assert!(!should_skip("assistant", "There was a panic: but this is a short message"));
    }

    #[test]
    fn test_parse_facts_json() {
        let response = r#"{"facts":["User prefers Rust","Project uses PostgreSQL"]}"#;
        let facts = parse_facts(response);
        assert_eq!(facts.len(), 2);
        assert!(facts[0].contains("Rust"));
    }

    #[test]
    fn test_parse_facts_empty() {
        assert!(parse_facts("{}").is_empty());
        assert!(parse_facts(r#"{"facts":[]}"#).is_empty());
        assert!(parse_facts("").is_empty());
    }

    #[test]
    fn test_parse_facts_fallback() {
        let facts = parse_facts("The user decided to use Vue 3 for the frontend.");
        assert_eq!(facts.len(), 1);
        assert!(facts[0].contains("Vue 3"));
    }

    #[test]
    fn test_build_extraction_prompt_contains_rules() {
        let prompt = build_extraction_prompt("test conversation");
        assert!(prompt.contains("memory extractor"));
        assert!(prompt.contains("Do NOT extract"));
        assert!(prompt.contains("test conversation"));
    }
}
