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

    pub fn max_retries(&self) -> u32 {
        self.max_retries
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
                    reason: RetryReason::RateLimit,
                };
            }

            // Network-level errors — stream drops, timeouts, connection resets.
            // These benefit from a longer base delay (1s) since the underlying
            // infrastructure may need more time to stabilize.
            let is_network_error = ["timeout", "connection", "reset",
                "broken pipe", "connection refused", "eof",
                "unexpected eof", "dns"].iter()
                .any(|kw| error.contains(kw));

            let is_server_error = ["500", "502", "503", "504", "server error",
                "empty response", "no useful events", "sse stream", "stream error", "stream"].iter()
                .any(|kw| error.contains(kw));

            if is_network_error || is_server_error {
                if let Some(ref fallback) = self.fallback_model
                    && ctx.attempt >= self.max_retries
                {
                    return RecoveryAction::SwitchModel {
                        model: fallback.clone(),
                    };
                }
                let reason = if is_network_error {
                    RetryReason::NetworkError
                } else {
                    RetryReason::ServerError
                };
                return RecoveryAction::Retry {
                    delay_ms: 1000 * 2u64.pow(ctx.attempt),
                    reason,
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
                    reason: RetryReason::Generic,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryReason {
    RateLimit,
    NetworkError,
    ServerError,
    Generic,
}

impl RetryReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RateLimit => "rate limit",
            Self::NetworkError => "network error",
            Self::ServerError => "server error",
            Self::Generic => "generic",
        }
    }
}

#[derive(Debug, Clone)]
pub enum RecoveryAction {
    Retry {
        delay_ms: u64,
        reason: RetryReason,
    },
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
            RecoveryAction::Retry { delay_ms, .. } => assert!(delay_ms > 0),
            _ => panic!("expected Retry"),
        }
    }

    #[test]
    fn test_escalate_tokens_on_truncation() {
        let engine = RecoveryEngine::new();
        let mut ctx = RecoveryContext::new("test", 4096);
        ctx.record_error("response was truncated due to length");

        match engine.determine_strategy(&ctx) {
            RecoveryAction::EscalateTokens { new_max_tokens } => assert!(new_max_tokens > 4096),
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

    #[test]
    fn test_context_too_long_keyword_variants_compact() {
        // Both "too long" and "context length" should map to CompactContext.
        let engine = RecoveryEngine::new();
        for kw in ["context too long", "maximum context length", "context length exceeded"] {
            let mut ctx = RecoveryContext::new("model", 4096);
            ctx.record_error(kw);
            match engine.determine_strategy(&ctx) {
                RecoveryAction::CompactContext { .. } => {}
                other => panic!("for '{kw}' expected CompactContext, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_switch_model_on_consecutive_rate_limits() {
        let engine = RecoveryEngine::new().with_fallback_model("fallback-model");
        let mut ctx = RecoveryContext::new("model", 4096);
        for _ in 0..3 {
            ctx.record_error("rate limit exceeded");
        }

        match engine.determine_strategy(&ctx) {
            RecoveryAction::SwitchModel { model } => assert_eq!(model, "fallback-model"),
            _ => panic!("expected SwitchModel"),
        }
    }

    #[test]
    fn test_switch_to_fallback_on_network_error_after_retries() {
        // Mirrors the contract the Run's SwitchModel recovery path relies on
        // (recovery.rs): after max_retries, a network error routes to the fallback.
        let engine = RecoveryEngine::new().with_fallback_model("fallback-model");
        let mut ctx = RecoveryContext::new("model", 4096);
        for _ in 0..3 {
            ctx.record_error("connection reset");
        }
        match engine.determine_strategy(&ctx) {
            RecoveryAction::SwitchModel { model } => assert_eq!(model, "fallback-model"),
            other => panic!("expected SwitchModel after retries, got {other:?}"),
        }
    }

    #[test]
    fn test_retry_backoff_grows_exponentially() {
        let engine = RecoveryEngine::new();
        let mut delays = Vec::new();
        for i in 0..3 {
            let mut ctx = RecoveryContext::new("model", 4096);
            ctx.attempt = i;
            ctx.last_error = Some("transient".into());
            match engine.determine_strategy(&ctx) {
                RecoveryAction::Retry { delay_ms, .. } => delays.push(delay_ms),
                other => panic!("attempt {i}: expected Retry, got {other:?}"),
            }
        }
        assert_eq!(delays[0], 500);
        assert_eq!(delays[1], 1000);
        assert_eq!(delays[2], 2000);
    }

    #[test]
    fn test_escalate_tokens_scales_by_factor() {
        // token_escalation_factor default = 1.5
        let engine = RecoveryEngine::new();
        let mut ctx = RecoveryContext::new("model", 4096);
        ctx.record_error("input truncat");
        match engine.determine_strategy(&ctx) {
            RecoveryAction::EscalateTokens { new_max_tokens } => {
                // (4096 * 1.5) = 6144
                assert_eq!(new_max_tokens, 6144);
            }
            other => panic!("expected EscalateTokens, got {other:?}"),
        }
    }
}
