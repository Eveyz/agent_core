//! Context compaction — chunked drop and LLM-based summarization (PLAN-0016).

use crate::types::Message;

use super::Run;

/// Never reserve more than this fraction of the context window for output.
/// Models advertising huge max_output (e.g. 384K on a 1M window) would
/// otherwise collapse the usable input budget to nearly nothing.
const MAX_OUTPUT_RESERVE_FRACTION: f64 = 0.30;

/// Context tokens available for the prompt after reserving output headroom.
fn usable_input_budget(max_context_tokens: usize, max_output_tokens: Option<usize>) -> usize {
    let reserved = match max_output_tokens.filter(|&n| n > 0) {
        Some(out) => {
            let cap = ((max_context_tokens as f64) * MAX_OUTPUT_RESERVE_FRACTION).ceil() as usize;
            out.min(cap).min(max_context_tokens.saturating_sub(1))
        }
        None => 0,
    };
    max_context_tokens.saturating_sub(reserved).max(1)
}

/// Pure decision helper for `maybe_compact` so the threshold math can be
/// unit-tested in isolation (without constructing a full `Run`).
///
/// Returns the soft target token budget, or `None` when the context is under
/// the compaction threshold. Threshold and target are computed against the
/// usable input budget (`max_context - clamped max_output`), not the raw
/// context window.
fn compact_decision(
    max_context_tokens: usize,
    max_output_tokens: Option<usize>,
    current_tokens: usize,
    threshold_ratio: f64,
    target_ratio: f64,
) -> Option<usize> {
    let ratio = threshold_ratio.clamp(0.0, 1.0);
    let usable = usable_input_budget(max_context_tokens, max_output_tokens);
    let threshold = (usable as f64 * ratio) as usize;
    if current_tokens < threshold {
        return None;
    }
    Some((usable as f64 * target_ratio.clamp(0.0, ratio)) as usize)
}

fn compaction_threshold(
    max_context_tokens: usize,
    max_output_tokens: Option<usize>,
    threshold_ratio: f64,
) -> usize {
    let usable = usable_input_budget(max_context_tokens, max_output_tokens);
    (usable as f64 * threshold_ratio.clamp(0.0, 1.0)) as usize
}

fn compaction_stage_strategy(chunked: bool, trimmed: bool) -> Option<&'static str> {
    match (chunked, trimmed) {
        (true, true) => Some("chunked_drop+trim_to_fit"),
        (true, false) => Some("chunked_drop"),
        (false, true) => Some("trim_to_fit"),
        (false, false) => None,
    }
}

impl Run {
    /// Compact strategy (Stage: Compact).
    ///
    /// Two-tier approach optimized for DeepSeek prefix caching:
    ///
    /// 0. **Ledger merge**: fold deterministic file touches into RollingSummary
    ///    before dropping turns (PLAN-0016).
    /// 1. **Chunked drop (preferred)**: When token usage exceeds the configured
    ///    threshold, retain the largest whole-turn suffix within the soft target.
    /// 2. **LLM summarize (fallback)**: Incremental delta merge into
    ///    RollingSummary; `micro_compact()` as last resort.
    ///
    /// Compaction mutates only the in-memory model window (`context.messages`).
    /// The full transcript used for UI / SQLite persistence is never touched.
    pub(super) async fn maybe_compact(&mut self) {
        let full_len_before = self.full_transcript.len();
        let current = self.context.current_token_count();
        let max_tokens = self.client.model.max_context_tokens;
        let max_output = self.client.model.max_tokens.map(|n| n as usize);
        let context_len = self.context.len();
        let target_tokens = match compact_decision(
            max_tokens,
            max_output,
            current,
            self.context.compressor.auto_compact_threshold,
            self.context.compressor.target_ratio,
        ) {
            Some(target) => target,
            None => return, // under threshold — nothing to do
        };
        let threshold = compaction_threshold(
            max_tokens,
            max_output,
            self.context.compressor.auto_compact_threshold,
        );

        // PLAN-0016: persist file ledger into RollingSummary before dropping turns.
        if !self.file_ledger.is_empty() {
            self.context
                .upsert_ledger_into_rolling_summary(&self.file_ledger);
        }

        // Tier 1: Chunked drop — zero-cost, cache-friendly bulk removal.
        let chunked = self
            .context
            .chunked_drop_to_target(target_tokens, self.context.compressor.gradient_keep_recent)
            > 0;
        if chunked {
            // Dropped turns are gone; strip thinking from what remains so old
            // CoT does not linger across the compaction boundary.
            self.context.strip_thinking_after_compact();
            // chunked_drop may have removed the leading RollingSummary — restore
            // file ledger into a fresh summary so touched paths survive the drop.
            if !self.file_ledger.is_empty() {
                self.context
                    .upsert_ledger_into_rolling_summary(&self.file_ledger);
            }
            // Check if this brought us below the threshold.
            if self.context.current_token_count() < threshold {
                let tokens_after = self.context.current_token_count();
                tracing::info!(
                    compact = "chunked_drop",
                    tokens_before = current,
                    tokens_after,
                    "Chunked drop compact applied"
                );
                self.emit_context_compacted("chunked_drop", current, tokens_after)
                    .await;
                debug_assert_eq!(
                    self.full_transcript.len(),
                    full_len_before,
                    "compaction must not mutate full_transcript"
                );
                return;
            }
        }

        // Also run trim_to_fit for snip/dedup/chunk compression.
        let tokens_before_trim = self.context.current_token_count();
        let _result = self.context.trim_to_fit();
        let tokens_after_trim = self.context.current_token_count();
        let trimmed = tokens_after_trim < tokens_before_trim;

        if let Some(strategy) = compaction_stage_strategy(chunked, trimmed) {
            tracing::info!(
                compact = strategy,
                tokens_before = current,
                tokens_after = tokens_after_trim,
                "Deterministic context compact applied"
            );
            self.emit_context_compacted(strategy, current, tokens_after_trim)
                .await;
        }

        if tokens_after_trim < threshold {
            debug_assert_eq!(
                self.full_transcript.len(),
                full_len_before,
                "compaction must not mutate full_transcript"
            );
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
                self.fallback_micro_compact(context_len, current, full_len_before)
                    .await;
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
                self.fallback_micro_compact(context_len, current, full_len_before)
                    .await;
                return;
            }
        };

        self.context
            .apply_summary_with_ledger(request.split_index, &summary, &self.file_ledger);
        self.emit_context_compacted("llm_summary", current, self.context.current_token_count())
            .await;
        debug_assert_eq!(
            self.full_transcript.len(),
            full_len_before,
            "compaction must not mutate full_transcript"
        );
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
                self.fallback_micro_compact(self.context.len(), tokens_before, full_len_before)
                    .await;
                return;
            }
        };

        let messages = vec![Message::system(&request.prompt)];
        let (result_text, _) = match self.client.chat_completion(&messages, &[]).await {
            Ok(r) => r,
            Err(_) => {
                self.fallback_micro_compact(self.context.len(), tokens_before, full_len_before)
                    .await;
                return;
            }
        };

        let summary: crate::compressor::TurnSummary = match serde_json::from_str(&result_text) {
            Ok(s) => s,
            Err(_) => {
                self.fallback_micro_compact(self.context.len(), tokens_before, full_len_before)
                    .await;
                return;
            }
        };

        self.context
            .apply_summary_with_ledger(request.split_index, &summary, &self.file_ledger);
        self.emit_context_compacted(
            "llm_summary",
            tokens_before,
            self.context.current_token_count(),
        )
        .await;
        debug_assert_eq!(self.full_transcript.len(), full_len_before);
    }

    async fn fallback_micro_compact(
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
        )
        .await;
        debug_assert_eq!(self.full_transcript.len(), full_len_before);
        self.refresh_usage_snapshot_only();
    }

    async fn emit_context_compacted(
        &mut self,
        strategy: &str,
        tokens_before: usize,
        tokens_after: usize,
    ) {
        // Publish the new snapshot before the event. The frontend refreshes
        // usage as soon as it receives ContextCompacted.
        self.refresh_usage_snapshot_only();
        self.persist_model_window_checkpoint().await;
        self.emit(crate::runtime::event::RunEvent::ContextCompacted {
            summary: format!(
                "{strategy}: {tokens_before} → {tokens_after} tokens (model window only; full transcript unchanged)"
            ),
            strategy: Some(strategy.to_string()),
            tokens_before: Some(tokens_before),
            tokens_after: Some(tokens_after),
        });
    }

    async fn persist_model_window_checkpoint(&self) {
        let (Some(session_manager), Some(session_id)) =
            (self.session_manager.clone(), self.session_id.clone())
        else {
            return;
        };
        let model_id = self.client.model.model_id.clone();
        let prompt_id = self.prompt_id.clone();
        let full_transcript = self.full_transcript.clone();
        let model_window = self.context.raw_messages().to_vec();
        // Invalidate queued snapshot writers before taking the shared lock.
        // A writer already holding the lock completes first; this commit then
        // replaces it. A queued writer observes the newer generation and skips.
        self.session_snapshot_gen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let snapshot_lock = self.session_snapshot_lock.clone();
        match tokio::task::spawn_blocking(move || {
            let _guard = snapshot_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            session_manager.save_live_model_window_checkpoint(
                &session_id,
                prompt_id.as_deref(),
                &model_id,
                &full_transcript,
                &model_window,
            )
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "failed to persist compacted model window");
            }
            Err(error) => {
                tracing::warn!(%error, "model-window checkpoint task failed");
            }
        }
    }

    /// Update the Context Usage ring without rewriting the full-transcript snapshot.
    fn refresh_usage_snapshot_only(&self) {
        *self.usage_snapshot.write() = self.context.usage_snapshot();
        *self.model_window_snapshot.write() = self.context.raw_messages().to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compressor::{ROLLING_SUMMARY_PREFIX, SummaryFiles, TurnSummary};
    use crate::context::ContextEngine;
    use crate::types::Role;

    #[test]
    fn test_compact_decision_returns_none_under_threshold() {
        // 500 tokens vs 10000 max — well below the 80% threshold.
        assert_eq!(compact_decision(10_000, None, 500, 0.8, 0.2), None);
    }

    #[test]
    fn test_compact_decision_returns_none_at_exactly_threshold_minus_one() {
        // 7999 tokens vs 8000 threshold — returns None at strictly-below.
        assert_eq!(compact_decision(10_000, None, 7999, 0.8, 0.2), None);
    }

    #[test]
    fn test_compact_decision_triggers_at_threshold() {
        // exactly 80% → triggers chunked_drop
        assert_eq!(compact_decision(10_000, None, 8_000, 0.8, 0.2), Some(2_000));
    }

    #[test]
    fn test_compact_decision_uses_configured_soft_target() {
        assert_eq!(
            compact_decision(256_000, None, 322_723, 0.8, 0.2),
            Some(51_200)
        );
    }

    #[test]
    fn compact_decision_reserves_clamped_max_output() {
        // 256K context + 128K output → reserve min(128K, 30% of 256K=76.8K) = 76800
        // usable = 256000 - 76800 = 179200; threshold = 80% = 143360; target = 20% = 35840
        assert_eq!(
            compact_decision(256_000, Some(128_000), 143_360, 0.8, 0.2),
            Some(35_840)
        );
        assert_eq!(
            compact_decision(256_000, Some(128_000), 143_359, 0.8, 0.2),
            None
        );
    }

    #[test]
    fn usable_input_budget_clamps_huge_max_output() {
        // 1M context + 384K output → reserve only 30% (300K), not full 384K
        assert_eq!(usable_input_budget(1_000_000, Some(384_000)), 700_000);
        assert_eq!(usable_input_budget(64_000, Some(8_192)), 64_000 - 8_192);
        assert_eq!(usable_input_budget(10_000, None), 10_000);
    }

    #[test]
    fn compaction_stage_reports_trim_only_with_final_count() {
        assert_eq!(compaction_stage_strategy(false, true), Some("trim_to_fit"));
    }

    #[test]
    fn compaction_stage_reports_chunked_then_trim_as_one_final_stage() {
        assert_eq!(
            compaction_stage_strategy(true, true),
            Some("chunked_drop+trim_to_fit")
        );
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
        assert_eq!(
            full.len(),
            full_len_before,
            "chunked_drop must not touch full"
        );
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
