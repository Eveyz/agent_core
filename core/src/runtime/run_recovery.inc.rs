impl Run {
    async fn try_recover(&mut self, error: &str) -> RecoveryOutcome {
        let action = self.recovery.determine_strategy(&self.recovery_ctx);
        match action {
            RecoveryAction::CompactContext { target_ratio } => {
                self.emit(RunEvent::Error {
                    message: format!(
                        "context too long; compacting to {:.0}% before retry",
                        target_ratio * 100.0
                    ),
                });
                self.force_compact(target_ratio).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::EscalateTokens { new_max_tokens } => {
                self.emit(RunEvent::Error {
                    message: format!("escalating max_tokens to {new_max_tokens}"),
                });
                self.client.set_max_tokens(new_max_tokens);
                RecoveryOutcome::Retry
            }
            RecoveryAction::Retry { delay_ms } => {
                self.emit(RunEvent::Error {
                    message: format!("retrying model call after {delay_ms}ms"),
                });
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::SwitchModel { model } => {
                self.emit(RunEvent::Error {
                    message: format!("switching to fallback model: {model}"),
                });
                // Model switching at runtime is complex — for now, just give up
                RecoveryOutcome::GiveUp
            }
            RecoveryAction::Fail => RecoveryOutcome::GiveUp,
        }
    }
}
