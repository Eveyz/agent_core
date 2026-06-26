//! Background reflection daemon — extracts durable facts from conversation
//! using a low-cost LLM and writes them to agverse.md (Core Memory)
//! and archival memory (backup).
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

/// Lazy-initialized reflection daemon.
///
/// The Tokio task is NOT spawned in `spawn()` — it is deferred to the first
/// `try_send()` call, which always happens from within a Run's async context.
/// This avoids panicking when `Brain::from_config` is called outside a Tokio
/// runtime (e.g., Tauri's synchronous `setup` closure).
pub struct ReflectionDaemon {
    sender: Mutex<Option<mpsc::Sender<ConversationSlice>>>,
    init: Mutex<Option<DaemonInit>>,
}

struct DaemonInit {
    client: OpenAIClient,
    memory: Arc<Mutex<MemoryManager>>,
    trigger_count: usize,
}

impl ReflectionDaemon {
    /// Create the daemon handle without spawning the task yet.
    pub fn spawn(
        client: OpenAIClient,
        memory: Arc<Mutex<MemoryManager>>,
        trigger_count: usize,
    ) -> Self {
        Self {
            sender: Mutex::new(None),
            init: Mutex::new(Some(DaemonInit {
                client,
                memory,
                trigger_count,
            })),
        }
    }

    /// Lazily spawn the background task on first use.
    fn ensure_spawned(&self) {
        let mut sender_guard = self.sender.lock();
        if sender_guard.is_some() {
            return;
        }

        let mut init_guard = self.init.lock();
        let Some(init) = init_guard.take() else {
            return;
        };

        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // No Tokio runtime — put init back for a later retry.
            *init_guard = Some(init);
            return;
        };

        let (tx, mut receiver) = mpsc::channel::<ConversationSlice>(200);
        let DaemonInit { client, memory, trigger_count } = init;

        handle.spawn(async move {
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

        *sender_guard = Some(tx);
    }

    /// Non-blocking send. Silently drops if channel is full or not yet spawned.
    pub fn try_send(&self, role: &str, content: &str) {
        self.ensure_spawned();
        let guard = self.sender.lock();
        if let Some(sender) = guard.as_ref() {
            let _ = sender.try_send(ConversationSlice {
                role: role.to_string(),
                content: content.to_string(),
            });
        }
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

/// A fact extracted by the reflection LLM, with a suggested section.
#[derive(Debug)]
struct ExtractedFact {
    section: String,
    text: String,
}

/// Call the LLM to extract facts, then write to agverse.md + archival memory.
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

    // Write to agverse.md (Core Memory)
    let agverse_path = crate::paths::get_global_agverse_md_path();
    match std::fs::read_to_string(&agverse_path) {
        Ok(content) => {
            let updated = append_facts_to_sections(&content, &facts);
            if updated != content {
                if let Err(e) = std::fs::write(&agverse_path, &updated) {
                    tracing::warn!("reflection: failed to update agverse.md: {e}");
                } else {
                    tracing::info!("reflection: updated agverse.md with {} facts", facts.len());
                }
            }
        }
        Err(e) => {
            tracing::warn!("reflection: could not read agverse.md for update: {e}");
        }
    }

    // Also write to archival memory as backup
    let mem = memory.lock();
    for fact in &facts {
        let metadata = serde_json::json!({
            "source": "reflection",
            "section": &fact.section,
            "extracted_from": format!("{} messages", slices.len()),
        })
        .to_string();

        if let Err(e) = mem.archival().insert(&fact.text, Some(&metadata)) {
            tracing::warn!("reflection: failed to store fact in archival: {e}");
        }
    }

    Ok(())
}

/// Append extracted facts to their matching sections in agverse.md.
/// If a section doesn't exist, the fact is appended to the end under a
/// "## Pending Notes" header for the agent to review and integrate.
fn append_facts_to_sections(content: &str, facts: &[ExtractedFact]) -> String {
    let mut result = content.to_string();

    for fact in facts {
        let section_header = format!("# {}", fact.section);
        if let Some(pos) = result.find(&section_header) {
            // Find the next section header or end of file
            let after_header = pos + section_header.len();
            let next_section = result[after_header..]
                .find("\n# ")
                .map(|p| after_header + p)
                .unwrap_or(result.len());

            // Insert before the next section
            result.insert_str(next_section, &format!("\n- {}", fact.text));
        } else {
            // Section not found — append to Pending Notes
            if !result.contains("# Pending Notes") {
                result.push_str("\n\n# Pending Notes\n");
            }
            result.push_str(&format!("- [{}] {}\n", fact.section, fact.text));
        }
    }

    result
}

fn build_extraction_prompt(conversation: &str) -> String {
    format!(
        r#"You are a memory extractor. Read the conversation below and extract ONLY new, durable facts.

Rules:
- Extract: user preferences, decisions, conventions, architecture decisions, tool choices.
- Do NOT extract: trivia, greetings, transient context, questions, or facts already obvious from the project setup.
- If no new facts, respond with exactly: {{"facts":[]}}
- If facts exist, respond as JSON: {{"facts":[{{"section":"User Preferences","text":"User prefers Rust for backend"}},...]}}
- Valid sections: "Project Overview", "Tech Stack & Commands", "Architecture Decisions", "Coding Conventions", "User Preferences", "Agent Instructions"
- Each fact text should be a single concise sentence.

Conversation:
{conversation}"#
    )
}

/// Parse the LLM response into a list of structured facts.
fn parse_facts(response: &str) -> Vec<ExtractedFact> {
    // Try to parse as JSON
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(response) {
        if let Some(facts) = parsed["facts"].as_array() {
            return facts
                .iter()
                .filter_map(|f| {
                    if let Some(text) = f["text"].as_str() {
                        let section = f["section"]
                            .as_str()
                            .unwrap_or("Pending Notes")
                            .to_string();
                        if !text.is_empty() {
                            return Some(ExtractedFact {
                                section,
                                text: text.to_string(),
                            });
                        }
                    }
                    // Backward compat: plain string facts
                    f.as_str().filter(|s| !s.is_empty()).map(|s| ExtractedFact {
                        section: "Pending Notes".to_string(),
                        text: s.to_string(),
                    })
                })
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
        return vec![ExtractedFact {
            section: "Pending Notes".to_string(),
            text: trimmed.to_string(),
        }];
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
        let response = r#"{"facts":[{"section":"User Preferences","text":"User prefers Rust"},{"section":"Architecture Decisions","text":"Project uses PostgreSQL"}]}"#;
        let facts = parse_facts(response);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].section, "User Preferences");
        assert!(facts[0].text.contains("Rust"));
        assert_eq!(facts[1].section, "Architecture Decisions");
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
        assert!(facts[0].text.contains("Vue 3"));
        assert_eq!(facts[0].section, "Pending Notes");
    }

    #[test]
    fn test_parse_facts_plain_string_array() {
        let response = r#"{"facts":["User prefers Rust","Project uses PostgreSQL"]}"#;
        let facts = parse_facts(response);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].section, "Pending Notes");
    }

    #[test]
    fn test_build_extraction_prompt_contains_rules() {
        let prompt = build_extraction_prompt("test conversation");
        assert!(prompt.contains("memory extractor"));
        assert!(prompt.contains("Do NOT extract"));
        assert!(prompt.contains("test conversation"));
    }

    #[test]
    fn test_append_facts_to_existing_section() {
        let content = "# Project Overview\n\nSome overview.\n\n# Architecture Decisions\n\n";
        let facts = vec![ExtractedFact {
            section: "Architecture Decisions".to_string(),
            text: "Use SQLite for local storage".to_string(),
        }];
        let result = append_facts_to_sections(content, &facts);
        assert!(result.contains("- Use SQLite for local storage"));
        assert!(result.contains("# Architecture Decisions"));
    }

    #[test]
    fn test_append_facts_to_missing_section() {
        let content = "# Project Overview\n\nSome overview.\n";
        let facts = vec![ExtractedFact {
            section: "User Preferences".to_string(),
            text: "User prefers dark mode".to_string(),
        }];
        let result = append_facts_to_sections(content, &facts);
        assert!(result.contains("# Pending Notes"));
        assert!(result.contains("[User Preferences] User prefers dark mode"));
    }
}
