use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryStrategy {
    RetryWithBackoff {
        max_retries: u32,
        base_delay_ms: u64,
    },
    TokenEscalation {
        increase_factor: f64,
        max_tokens_limit: u32,
    },
    FallbackModel {
        model_name: String,
    },
    ContextCompact {
        target_ratio: f64,
    },
    PathSwitch {
        suggestion: String,
    },
}

pub struct RecoveryContext {
    pub attempt: u32,
    pub last_error: Option<String>,
    pub token_count: usize,
    pub max_tokens: usize,
    pub model_name: String,
}

impl RecoveryContext {
    pub fn new(model_name: &str, max_tokens: usize) -> Self {
        Self {
            attempt: 0,
            last_error: None,
            token_count: 0,
            max_tokens,
            model_name: model_name.to_string(),
        }
    }

    pub fn record_error(&mut self, error: &str) {
        self.attempt += 1;
        self.last_error = Some(error.to_string());
    }

    pub fn record_success(&mut self) {
        self.attempt = 0;
        self.last_error = None;
    }
}

pub struct RecoveryEngine {
    _strategies: Vec<RecoveryStrategy>,
    fallback_model: Option<String>,
    max_retries: u32,
    token_escalation_factor: f64,
    compact_threshold: f64,
}

impl Default for RecoveryEngine {
    fn default() -> Self {
        Self {
            _strategies: vec![RecoveryStrategy::RetryWithBackoff {
                max_retries: 3,
                base_delay_ms: 500,
            }],
            fallback_model: None,
            max_retries: 3,
            token_escalation_factor: 1.5,
            compact_threshold: 0.8,
        }
    }
}

impl RecoveryEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fallback_model(mut self, model: &str) -> Self {
        self.fallback_model = Some(model.to_string());
        self
    }

    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    pub fn determine_strategy(&self, ctx: &RecoveryContext) -> RecoveryAction {
        if let Some(ref error) = ctx.last_error {
            if error.contains("too long") || error.contains("context length") {
                return RecoveryAction::CompactContext {
                    target_ratio: self.compact_threshold,
                };
            }

            if error.contains("rate limit") || error.contains("429") {
                if let Some(ref fallback) = self.fallback_model
                    && ctx.attempt >= self.max_retries
                {
                    return RecoveryAction::SwitchModel {
                        model: fallback.clone(),
                    };
                }
                return RecoveryAction::Retry {
                    delay_ms: 500 * 2u64.pow(ctx.attempt),
                };
            }

            // Network-level errors — stream drops, timeouts, connection resets.
            // These benefit from a longer base delay (1s) since the underlying
            // infrastructure may need more time to stabilize.
            if ["timeout", "stream error", "connection", "reset",
                "broken pipe", "connection refused", "eof",
                "unexpected eof", "dns"].iter()
                .any(|kw| error.contains(kw))
            {
                if let Some(ref fallback) = self.fallback_model
                    && ctx.attempt >= self.max_retries
                {
                    return RecoveryAction::SwitchModel {
                        model: fallback.clone(),
                    };
                }
                return RecoveryAction::Retry {
                    delay_ms: 1000 * 2u64.pow(ctx.attempt),
                };
            }

            if error.contains("length") || error.contains("truncat") {
                let new_max = (ctx.max_tokens as f64 * self.token_escalation_factor) as u32;
                return RecoveryAction::EscalateTokens {
                    new_max_tokens: new_max,
                };
            }

            if ctx.attempt < self.max_retries {
                return RecoveryAction::Retry {
                    delay_ms: 500 * 2u64.pow(ctx.attempt),
                };
            }

            if let Some(ref fallback) = self.fallback_model {
                return RecoveryAction::SwitchModel {
                    model: fallback.clone(),
                };
            }
        }

        RecoveryAction::Fail
    }
}

#[derive(Debug, Clone)]
pub enum RecoveryAction {
    Retry { delay_ms: u64 },
    EscalateTokens { new_max_tokens: u32 },
    SwitchModel { model: String },
    CompactContext { target_ratio: f64 },
    Fail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_on_rate_limit() {
        let engine = RecoveryEngine::new();
        let mut ctx = RecoveryContext::new("test", 4096);
        ctx.record_error("rate limit exceeded");

        match engine.determine_strategy(&ctx) {
            RecoveryAction::Retry { delay_ms } => assert!(delay_ms > 0),
            _ => panic!("expected Retry"),
        }
    }

    #[test]
    fn test_escalate_tokens_on_truncation() {
        let engine = RecoveryEngine::new();
        let mut ctx = RecoveryContext::new("test", 4096);
        ctx.record_error("response was truncated due to length");

        match engine.determine_strategy(&ctx) {
            RecoveryAction::EscalateTokens { new_max_tokens } => {
                assert!(new_max_tokens > 4096);
            }
            _ => panic!("expected EscalateTokens"),
        }
    }

    #[test]
    fn test_compact_on_context_too_long() {
        let engine = RecoveryEngine::new();
        let mut ctx = RecoveryContext::new("test", 4096);
        ctx.record_error("context length exceeded");

        match engine.determine_strategy(&ctx) {
            RecoveryAction::CompactContext { .. } => {}
            _ => panic!("expected CompactContext"),
        }
    }

    #[test]
    fn test_fallback_model_after_max_retries() {
        let engine = RecoveryEngine::new()
            .with_fallback_model("gpt-3.5")
            .with_max_retries(2);
        let mut ctx = RecoveryContext::new("test", 4096);
        ctx.record_error("rate limit");
        ctx.record_error("rate limit");
        ctx.record_error("rate limit");

        match engine.determine_strategy(&ctx) {
            RecoveryAction::SwitchModel { model } => assert_eq!(model, "gpt-3.5"),
            _ => panic!("expected SwitchModel"),
        }
    }

    #[test]
    fn test_fail_with_no_strategy() {
        let engine = RecoveryEngine::new();
        let ctx = RecoveryContext::new("test", 4096);

        match engine.determine_strategy(&ctx) {
            RecoveryAction::Fail => {}
            _ => panic!("expected Fail"),
        }
    }
}
