//! Recall gate — decides when to hint or auto-inject past conversation memory.
//!
//! Keeps dynamic context injection small in Standard mode (hint-only) while
//! allowing Deep mode to proactively inject top recall hits.

use super::recall::RecallRecord;

/// How aggressively the runtime should surface recall memory this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallIntent {
    /// No recall action needed.
    None,
    /// Inject a short hint telling the model to call `conversation_search`.
    Hint,
    /// Pre-search and inject top recall results into active memory (Deep mode).
    AutoInject,
}

/// Classify whether the latest user message likely needs historical context.
pub fn route_recall_intent(message: Option<&str>) -> RecallIntent {
    let Some(msg) = message else {
        return RecallIntent::None;
    };

    let trimmed = msg.trim();
    if trimmed.is_empty() {
        return RecallIntent::None;
    }

    let word_count = trimmed.split_whitespace().count();
    let char_count = trimmed.chars().filter(|c| !c.is_whitespace()).count();
    // CJK messages often have no whitespace — use char count as fallback.
    if word_count < 3 && char_count < 6 {
        return RecallIntent::None;
    }

    let lower = trimmed.to_lowercase();

    // Strong triggers → auto-inject in Deep mode, hint in Standard.
    const STRONG: &[&str] = &[
        "之前",
        "上次",
        "记得",
        "说过",
        "讨论过",
        "我们聊",
        "continue from",
        "last time",
        "we discussed",
        "do you remember",
        "as we said",
        "as mentioned",
        "earlier you",
        "you told me",
        "my preference",
        "my preferences",
    ];

    if STRONG.iter().any(|t| lower.contains(t)) {
        return RecallIntent::AutoInject;
    }

    // Medium triggers → hint only (Standard) or auto-inject (Deep decided by caller).
    const MEDIUM: &[&str] = &[
        "以前",
        "还记得",
        "回忆",
        "历史",
        "过往",
        "remember",
        "recall",
        "previously",
        "before we",
        "from before",
        "last session",
        "last conversation",
    ];

    if MEDIUM.iter().any(|t| lower.contains(t)) {
        return RecallIntent::Hint;
    }

    // Questions with enough substance may need past context.
    if trimmed.contains('?') || trimmed.contains('？') {
        if word_count >= 8 || char_count >= 12 {
            return RecallIntent::Hint;
        }
    }

    RecallIntent::None
}

/// Map router intent to runtime action for a given memory mode.
pub fn intent_for_mode(intent: RecallIntent, mode: crate::config::MemoryMode) -> RecallIntent {
    match (intent, mode) {
        (RecallIntent::None, _) => RecallIntent::None,
        (RecallIntent::Hint, _) => RecallIntent::Hint,
        (RecallIntent::AutoInject, crate::config::MemoryMode::Deep) => RecallIntent::AutoInject,
        (RecallIntent::AutoInject, _) => RecallIntent::Hint,
    }
}

/// Short hint injected into active memory (cache-friendly, ~20 tokens).
pub const RECALL_HINT: &str = "[Memory hint: This question may need past context. Call conversation_search before answering if you are unsure.]";

/// Format recall hits for injection into the active memory segment.
pub fn format_recall_results(records: &[RecallRecord], max_chars: usize) -> String {
    if records.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    let mut used = 0usize;

    for r in records {
        let snippet = truncate_chars(&r.content, 240);
        let line = format!("  • [{}] {}: {}", r.created_at, r.role, snippet);
        if used + line.len() > max_chars {
            break;
        }
        used += line.len();
        lines.push(line);
    }

    format!("Relevant Past Conversations:\n{}", lines.join("\n"))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .take(max)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(max)
        .min(s.len());
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryMode;

    #[test]
    fn short_messages_skip_recall() {
        assert_eq!(route_recall_intent(Some("hi")), RecallIntent::None);
        assert_eq!(route_recall_intent(Some("ok thanks")), RecallIntent::None);
    }

    #[test]
    fn strong_triggers_auto_inject() {
        assert_eq!(
            route_recall_intent(Some("我们上次讨论的方案是什么？")),
            RecallIntent::AutoInject
        );
        assert_eq!(
            route_recall_intent(Some("Do you remember what we discussed last time?")),
            RecallIntent::AutoInject
        );
    }

    #[test]
    fn medium_triggers_hint() {
        assert_eq!(
            route_recall_intent(Some("Can you recall our previous conversation about auth?")),
            RecallIntent::Hint
        );
    }

    #[test]
    fn deep_mode_allows_auto_inject() {
        assert_eq!(
            intent_for_mode(RecallIntent::AutoInject, MemoryMode::Deep),
            RecallIntent::AutoInject
        );
        assert_eq!(
            intent_for_mode(RecallIntent::AutoInject, MemoryMode::Standard),
            RecallIntent::Hint
        );
    }
}
