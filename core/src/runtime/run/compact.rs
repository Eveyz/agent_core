//! Context compaction — chunked drop and LLM-based summarization (PLAN-0016).

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
    /// 0. **Ledger merge**: fold deterministic file touches into RollingSummary
    ///    before dropping turns (PLAN-0016).
    /// 1. **Chunked drop (preferred)**: When token usage exceeds 80% of the
    ///    context window, batch-drop the oldest 50% of conversation turns.
    /// 2. **LLM summarize (fallback)**: Incremental delta merge into
    ///    RollingSummary; `micro_compact()` as last resort.
    ///
    /// Compaction mutates only the in-memory model window (`context.messages`).
    /// The full transcript used for UI / SQLite persistence is never touched.
    pub(super) async fn maybe_compact(&mut self) {
        let full_len_before = self.full_transcript.len();
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

        // PLAN-0016: persist file ledger into RollingSummary before dropping turns.
        if !self.file_ledger.is_empty() {
            self.context
                .upsert_ledger_into_rolling_summary(&self.file_ledger);
        }

        // Tier 1: Chunked drop — zero-cost, cache-friendly bulk removal.
        if self.context.chunked_drop(keep) > 0 {
            // Dropped turns are gone; strip thinking from what remains so old
            // CoT does not linger across the compaction boundary.
            self.context.strip_thinking_after_compact();
            // chunked_drop may have removed the leading RollingSummary — restore
            // file ledger into a fresh summary so touched paths survive the drop.
            if !self.file_ledger.is_empty() {
                self.context
                    .upsert_ledger_into_rolling_summary(&self.file_ledger);
            }
            let tokens_after = self.context.current_token_count();
            tracing::info!(
                compact = "chunked_drop",
                tokens_before = current,
                tokens_after,
                "Chunked drop compact applied"
            );
            self.emit_context_compacted(
                "chunked_drop",
                current,
                tokens_after,
            );
            // Check if this brought us below the threshold.
            let threshold = (max_tokens as f64 * 0.80) as usize;
            if self.context.current_token_count() < threshold {
                debug_assert_eq!(
                    self.full_transcript.len(),
                    full_len_before,
                    "compaction must not mutate full_transcript"
                );
                self.refresh_usage_snapshot_only();
                return;
            }
        }

        // Also run trim_to_fit for snip/dedup/chunk compression.
        let _result = self.context.trim_to_fit();

        let threshold = (max_tokens as f64 * 0.80) as usize;
        if self.context.current_token_count() < threshold {
            debug_assert_eq!(
                self.full_transcript.len(),
                full_len_before,
                "compaction must not mutate full_transcript"
            );
            self.refresh_usage_snapshot_only();
            return;
        }

        // Tier 2: LLM summarize — expensive, but handles pathological cases.
        let num_turns = context_len.max(4) * 2 / 5;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => {
                debug_assert_eq!(self.full_transcript.len(), full_len_before);
                return;
            }
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                self.fallback_micro_compact(context_len, current, full_len_before);
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
                self.fallback_micro_compact(context_len, current, full_len_before);
                return;
            }
        };

        self.context
            .apply_summary_with_ledger(request.split_index, &summary, &self.file_ledger);
        self.emit_context_compacted(
            "llm_summary",
            current,
            self.context.current_token_count(),
        );
        debug_assert_eq!(
            self.full_transcript.len(),
            full_len_before,
            "compaction must not mutate full_transcript"
        );
        self.refresh_usage_snapshot_only();
    }

    /// Force an LLM compaction of the oldest turns regardless of current token
    /// count. Used by the recovery path when the model returns a context-too-long
    /// error. Falls back to `micro_compact` if the LLM call or JSON parse fails.
    /// Does not touch `full_transcript`.
    pub(super) async fn force_compact(&mut self, target_ratio: f64) {
        let full_len_before = self.full_transcript.len();
        let tokens_before = self.context.current_token_count();

        if !self.file_ledger.is_empty() {
            self.context
                .upsert_ledger_into_rolling_summary(&self.file_ledger);
        }

        let remove_fraction = (1.0 - target_ratio).clamp(0.1, 0.6);
        let num_turns = (self.context.len().max(4) as f64 * remove_fraction) as usize;
        let request = match self.context.prepare_summary(num_turns) {
            Some(r) => r,
            None => {
                self.fallback_micro_compact(self.context.len(), tokens_before, full_len_before);
                return;
            }
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                self.fallback_micro_compact(self.context.len(), tokens_before, full_len_before);
                return;
            }
        };

        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => s,
            Err(_) => {
                self.fallback_micro_compact(self.context.len(), tokens_before, full_len_before);
                return;
            }
        };

        self.context
            .apply_summary_with_ledger(request.split_index, &summary, &self.file_ledger);
        self.emit_context_compacted(
            "llm_summary",
            tokens_before,
            self.context.current_token_count(),
        );
        debug_assert_eq!(self.full_transcript.len(), full_len_before);
        self.refresh_usage_snapshot_only();
    }

    fn fallback_micro_compact(
        &mut self,
        context_len: usize,
        tokens_before: usize,
        full_len_before: usize,
    ) {
        self.context.micro_compact(context_len.max(4) / 3);
        if !self.file_ledger.is_empty() {
            self.context
                .upsert_ledger_into_rolling_summary(&self.file_ledger);
        }
        self.emit_context_compacted(
            "micro_compact",
            tokens_before,
            self.context.current_token_count(),
        );
        debug_assert_eq!(self.full_transcript.len(), full_len_before);
        self.refresh_usage_snapshot_only();
    }

    fn emit_context_compacted(&mut self, strategy: &str, tokens_before: usize, tokens_after: usize) {
        self.emit(crate::runtime::event::RunEvent::ContextCompacted {
            summary: format!(
                "{strategy}: {tokens_before} → {tokens_after} tokens (model window only; full transcript unchanged)"
            ),
        });
    }

    /// Update the Context Usage ring without rewriting the full-transcript snapshot.
    fn refresh_usage_snapshot_only(&self) {
        *self.usage_snapshot.write() = self.context.usage_snapshot();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compressor::{SummaryFiles, TurnSummary, ROLLING_SUMMARY_PREFIX};
    use crate::context::ContextEngine;
    use crate::types::Role;

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

    /// Dual-track invariant: chunked_drop + apply_summary shrink the model
    /// window but leave a parallel full transcript untouched.
    #[test]
    fn dual_track_compact_leaves_full_transcript_intact() {
        let mut full: Vec<Message> = Vec::new();
        let mut engine = ContextEngine::new("identity", 200_000);

        let push = |full: &mut Vec<Message>, engine: &mut ContextEngine, msg: Message| {
            full.push(msg.clone());
            engine.add(msg);
        };

        for i in 0..8 {
            push(
                &mut full,
                &mut engine,
                Message::user(&format!("user turn {i}")),
            );
            push(
                &mut full,
                &mut engine,
                Message::assistant(&format!("assistant reply {i}")),
            );
        }

        let full_len_before = full.len();
        let first_user = full[0].content.clone();
        let model_len_before = engine.len();
        assert_eq!(full_len_before, model_len_before);

        let dropped = engine.chunked_drop(6);
        assert!(dropped > 0);
        assert!(engine.len() < model_len_before);
        assert_eq!(full.len(), full_len_before, "chunked_drop must not touch full");
        assert_eq!(full[0].content, first_user);

        let summary = TurnSummary {
            decisions: vec!["keep going".into()],
            files: SummaryFiles::default(),
            facts: vec!["fact".into()],
            ..Default::default()
        };
        let split = engine.len().saturating_sub(2).max(1);
        // Summarize whatever is still in the model window front.
        if engine.len() > 2 {
            let num_turns = 1;
            engine.apply_summary(1.min(split), &summary, num_turns);
            assert!(
                engine.raw_messages().iter().any(|m| {
                    m.role == Role::Assistant
                        && m.content
                            .as_deref()
                            .is_some_and(|c| c.contains(ROLLING_SUMMARY_PREFIX))
                }),
                "summary lives only in the model window"
            );
        }

        assert_eq!(
            full.len(),
            full_len_before,
            "full transcript must survive summary compact"
        );
        assert_eq!(full[0].content, first_user);
        assert!(
            !full.iter().any(|m| {
                m.content
                    .as_deref()
                    .is_some_and(|c| c.contains(ROLLING_SUMMARY_PREFIX))
            }),
            "summary must never appear in the full transcript"
        );
    }
}
