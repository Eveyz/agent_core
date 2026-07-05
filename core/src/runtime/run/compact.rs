//! Context compaction — chunked drop and LLM-based summarization.

use crate::types::Message;

use super::Run;

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
        const COMPACT_THRESHOLD: f64 = 0.80;
        const CHUNK_KEEP_RECENT: usize = 20;

        let current = self.context.current_token_count();
        let threshold =
            (self.client.model.max_context_tokens as f64 * COMPACT_THRESHOLD) as usize;

        if current < threshold {
            return;
        }

        // Tier 1: Chunked drop — zero-cost, cache-friendly bulk removal.
        let keep = (self.context.len() / 2).max(4).min(CHUNK_KEEP_RECENT);
        if self.context.chunked_drop(keep) > 0 {
            tracing::info!(
                compact = "chunked_drop",
                tokens_before = current,
                tokens_after = self.context.current_token_count(),
                "Chunked drop compact applied"
            );
            // Check if this brought us below the threshold.
            if self.context.current_token_count() < threshold {
                return;
            }
        }

        // Also run trim_to_fit for snip/dedup/chunk compression.
        let _result = self.context.trim_to_fit();

        if self.context.current_token_count() < threshold {
            return;
        }

        // Tier 2: LLM summarize — expensive, but handles pathological cases.
        let num_turns = self.context.len().max(4) * 2 / 5;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => return,
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                // LLM call failed — fallback to micro_compact
                self.context.micro_compact(self.context.len().max(4) / 3);
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
                self.context.micro_compact(self.context.len().max(4) / 3);
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
