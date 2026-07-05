impl Run {
    /// Force an LLM compaction of the oldest turns regardless of current token
    /// count. Used by the recovery path when the model returns a context-too-long
    /// error. Falls back to `micro_compact` if the LLM call or JSON parse fails.
    async fn force_compact(&mut self, target_ratio: f64) {
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
