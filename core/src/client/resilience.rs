use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    Closed, // Normal operation
    Open,   // Failing, blocking requests
    HalfOpen, // Testing if the service is back
}

pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub reset_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            reset_timeout: Duration::from_secs(60),
        }
    }
}

pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<CircuitBreakerStateData>,
}

struct CircuitBreakerStateData {
    state: CircuitState,
    failure_count: u32,
    last_failure_time: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            state: Mutex::new(CircuitBreakerStateData {
                state: CircuitState::Closed,
                failure_count: 0,
                last_failure_time: None,
            }),
        })
    }

    pub fn acquire_permit(&self) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();

        match state.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                if let Some(last_fail) = state.last_failure_time {
                    if last_fail.elapsed() >= self.config.reset_timeout {
                        state.state = CircuitState::HalfOpen;
                        return Ok(());
                    }
                }
                Err("Circuit breaker is OPEN. Requests are blocked.")
            }
            CircuitState::HalfOpen => {
                // Only allow one request through in HalfOpen state.
                state.state = CircuitState::Open;
                Ok(())
            }
        }
    }

    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap();
        state.failure_count = 0;
        state.state = CircuitState::Closed;
    }

    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap();
        state.failure_count += 1;
        state.last_failure_time = Some(Instant::now());

        if state.failure_count >= self.config.failure_threshold {
            state.state = CircuitState::Open;
        }
    }
}

pub fn calculate_backoff(attempt: u32, base_delay: Duration, max_delay: Duration) -> Duration {
    let factor = 2_u32.pow(attempt.min(6));
    let current_delay = base_delay.saturating_mul(factor);
    let current_delay = current_delay.min(max_delay);
    
    let millis = current_delay.as_millis() as u64;
    let jitter_millis = rand::thread_rng().gen_range(0..=millis.max(1));
    Duration::from_millis(jitter_millis)
}
