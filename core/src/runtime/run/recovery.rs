//! Error recovery — retry, compact, escalate, or switch models.

use crate::error_recovery::{RecoveryAction, RecoveryContext};
use crate::runtime::event::RunEvent;

use super::{RecoveryOutcome, Run};

impl Run {
    pub(super) async fn try_recover(&mut self, _error: &str) -> RecoveryOutcome {
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
            RecoveryAction::Retry { delay_ms, reason } => {
                self.emit(RunEvent::Error {
                    message: format!(
                        "Failed to connect to remote model ({}), retrying in {}s (attempt {}/{})",
                        reason.as_str(),
                        delay_ms / 1000,
                        self.recovery_ctx.attempt,
                        self.recovery.max_retries(),
                    ),
                });
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                RecoveryOutcome::Retry
            }
            RecoveryAction::SwitchModel { model } => {
                self.emit(RunEvent::Error {
                    message: format!("switching to fallback model: {model}"),
                });
                // Look up the new model config from the Brain's shared Config
                // and rebuild the client directly (no &mut self on Arc<Brain> needed).
                match self.brain.config.get_model(&model) {
                    Some(model_config) => {
                        let new_client = crate::client::OpenAIClient::new(model_config.clone());
                        let max_tokens = new_client.model.max_context_tokens;
                        self.client = new_client;
                        self.recovery_ctx =
                            RecoveryContext::new(&model, max_tokens);
                        tracing::info!(
                            model = %model,
                            "switched model mid-run, continuing"
                        );
                        RecoveryOutcome::Retry
                    }
                    None => {
                        tracing::error!(
                            model = %model,
                            "fallback model not found in config"
                        );
                        RecoveryOutcome::GiveUp
                    }
                }
            }
            RecoveryAction::Fail => RecoveryOutcome::GiveUp,
        }
    }
}
