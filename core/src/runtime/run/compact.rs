//! Context compaction — chunked drop and LLM-based summarization.

use crate::types::Message;

use super::Run;

/// Pure decision helper for `maybe_compact` so the threshold math can be
/// unit-tested in isolation (without constructing a full `Run`).
///
/// Returns `Some(keep)` = number of recent turns to retain via chunked_drop,
/// or `None` when the context is under the compaction threshold.
fn compact_decision(
    max_tokens: usize,
    current_tokens: usize,
    context_len: usize,
    keep_recent: usize,
) -> Option<usize> {
    const COMPACT_THRESHOLD: f64 = 0.80;

    let threshold = (max_tokens as f64 * COMPACT_THRESHOLD) as usize;
    if current_tokens < threshold {
        return None;
    }
    let keep = (context_len / 2).max(4).min(keep_recent);
    Some(keep)
}

impl Run {
    /// Compact strategy (Stage: Compact).
    ///
    /// Two-tier approach optimized for DeepSeek prefix caching:
    ///
    /// 1. **Chunked drop (preferred)**: When token usage exceeds 80% of the
    ///    context window, batch-drop the oldest 50% of conversation turns.
    ///    This causes a single-turn cache miss but leaves a stable prefix
    ///    for the next 10+ turns — maximizing long-term cache hits with
    ///    zero LLM overhead.
    ///
    /// 2. **LLM summarize (fallback)**: If chunked_drop wasn't sufficient
    ///    (e.g., a single tool result is dominating), fall back to the
    ///    LLM-based summary compact with `micro_compact()` as last resort.
    pub(super) async fn maybe_compact(&mut self) {
        let current = self.context.current_token_count();
        let max_tokens = self.client.model.max_context_tokens;
        let context_len = self.context.len();
        let keep = match compact_decision(
            max_tokens,
            current,
            context_len,
            self.context.compressor.gradient_keep_recent,
        ) {
            Some(k) => k,
            None => return, // under threshold — nothing to do
        };

        // Tier 1: Chunked drop — zero-cost, cache-friendly bulk removal.
        if self.context.chunked_drop(keep) > 0 {
            // Dropped turns are gone; strip thinking from what remains so old
            // CoT does not linger across the compaction boundary.
            self.context.strip_thinking_after_compact();
            tracing::info!(
                compact = "chunked_drop",
                tokens_before = current,
                tokens_after = self.context.current_token_count(),
                "Chunked drop compact applied"
            );
            // Check if this brought us below the threshold.
            let threshold = (max_tokens as f64 * 0.80) as usize;
            if self.context.current_token_count() < threshold {
                return;
            }
        }

        // Also run trim_to_fit for snip/dedup/chunk compression.
        let _result = self.context.trim_to_fit();

        let threshold = (max_tokens as f64 * 0.80) as usize;
        if self.context.current_token_count() < threshold {
            return;
        }

        // Tier 2: LLM summarize — expensive, but handles pathological cases.
        let num_turns = context_len.max(4) * 2 / 5;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => return,
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                // LLM call failed — fallback to micro_compact
                self.context.micro_compact(context_len.max(4) / 3);
                return;
            }
        };

        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => {
                tracing::info!(
                    compact = "llm_summary",
                    turns_summarized = num_turns,
                    "LLM compact applied"
                );
                s
            }
            Err(_) => {
                // JSON parse failed — fallback to micro_compact
                self.context.micro_compact(context_len.max(4) / 3);
                return;
            }
        };

        self.context
            .apply_summary(request.split_index, &summary, num_turns);
    }

    /// Force an LLM compaction of the oldest turns regardless of current token
    /// count. Used by the recovery path when the model returns a context-too-long
    /// error. Falls back to `micro_compact` if the LLM call or JSON parse fails.
    pub(super) async fn force_compact(&mut self, target_ratio: f64) {
        let remove_fraction = (1.0 - target_ratio).clamp(0.1, 0.6);
        let num_turns = (self.context.len().max(4) as f64 * remove_fraction) as usize;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => {
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => s,
            Err(_) => {
                self.context.micro_compact(self.context.len().max(4) / 3);
                return;
            }
        };

        self.context
            .apply_summary(request.split_index, &summary, num_turns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_decision_returns_none_under_threshold() {
        // 500 tokens vs 10000 max — well below the 80% threshold.
        assert_eq!(compact_decision(10_000, 500, 10, 6), None);
    }

    #[test]
    fn test_compact_decision_returns_none_at_exactly_threshold_minus_one() {
        // 7999 tokens vs 8000 threshold — returns None at strictly-below.
        assert_eq!(compact_decision(10_000, 7999, 30, 6), None);
    }

    #[test]
    fn test_compact_decision_triggers_at_threshold() {
        // exactly 80% → triggers chunked_drop
        assert!(compact_decision(10_000, 8_000, 30, 6).is_some());
    }

    #[test]
    fn test_compact_decision_keep_is_half_of_context_len() {
        // 20 turns → keep 10 (cap of 20 not hit)
        let keep = compact_decision(10_000, 9_000, 20, 20).unwrap();
        assert_eq!(keep, 10);
    }

    #[test]
    fn test_compact_decision_keep_min_is_4() {
        // 2 turns → keep 4 (the min floor)
        let keep = compact_decision(10_000, 9_000, 2, 6).unwrap();
        assert_eq!(keep, 4);
    }

    #[test]
    fn test_compact_decision_keep_respects_cap() {
        // 100 turns, cap=20 → keep 20 (the cap)
        let keep = compact_decision(10_000, 9_000, 100, 20).unwrap();
        assert_eq!(keep, 20);
    }

    #[test]
    fn test_compact_decision_keep_respects_small_cap() {
        // 100 turns, cap=6 → keep 6 (tight cap from gradient_keep_recent)
        let keep = compact_decision(10_000, 9_000, 100, 6).unwrap();
        assert_eq!(keep, 6);
    }

    #[test]
    fn test_compact_decision_keep_at_floor_when_odd_len() {
        // 5 turns → keep = max(5/2, 4) = max(2, 4) = 4
        let keep = compact_decision(10_000, 9_000, 5, 6).unwrap();
        assert_eq!(keep, 4);
    }
}
