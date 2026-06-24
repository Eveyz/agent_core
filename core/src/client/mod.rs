pub mod resilience;
pub mod streaming;

use crate::client::resilience::{CircuitBreaker, CircuitBreakerConfig, calculate_backoff};
use crate::config::{ModelConfig, RuntimeOverrides};
use crate::types::{Message, StreamEvent, ToolDefinition};
use anyhow::{Result, bail};
use reqwest::Response;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

pub struct OpenAIClient {
    http: reqwest::Client,
    pub(crate) model: ModelConfig,
    pub(crate) overrides: RuntimeOverrides,
    circuit_breaker: Arc<CircuitBreaker>,
    fallback_client: Option<Box<OpenAIClient>>,
}

impl OpenAIClient {
    pub fn new(model: ModelConfig) -> Self {
        Self::with_fallback(model, None)
    }

    pub fn with_fallback(mut model: ModelConfig, fallback: Option<OpenAIClient>) -> Self {
        // Resolve `${VAR}` env-var references at the point of use. This keeps the
        // on-disk Config holding the `${VAR}` reference (so save()/get_config
        // never leak the plaintext secret) while the running client still gets
        // the real key for bearer_auth.
        model.api_key = crate::config::resolve_env_value(&model.api_key);
        let http = Self::build_http_client(model.request_timeout_secs);
        Self {
            http,
            model,
            overrides: RuntimeOverrides {
                temperature: None,
                max_tokens: None,
            },
            circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
            fallback_client: fallback.map(Box::new),
        }
    }

    /// Reconfigure model without destroying the HTTP connection pool.
    pub fn switch_model(&mut self, mut model: ModelConfig) {
        // Resolve `${VAR}` references here too (mirrors with_fallback) so a
        // model switched at runtime from an on-disk Config still gets the real key.
        model.api_key = crate::config::resolve_env_value(&model.api_key);
        let new_timeout = Duration::from_secs(model.request_timeout_secs);
        let old_timeout = Duration::from_secs(self.model.request_timeout_secs);
        // Rebuild client only if timeout changed (affects connection pool settings)
        if new_timeout != old_timeout {
            self.http = Self::build_http_client(model.request_timeout_secs);
        }
        self.model = model;
    }

    fn build_http_client(timeout_secs: u64) -> reqwest::Client {
        reqwest::Client::builder()
            .read_timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .build()
            .expect("failed to build http client")
    }

    pub fn set_temperature(&mut self, temp: f64) {
        self.overrides.temperature = Some(temp);
    }

    pub fn set_max_tokens(&mut self, max: u32) {
        self.overrides.max_tokens = Some(max);
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> Value {
        let mut body = serde_json::json!({
            "model": self.model.model_id,
            "messages": messages,
            "stream": stream,
        });

        if let Some(temp) = self.overrides.temperature.or(self.model.temperature) {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(max_tokens) = self.overrides.max_tokens.or(self.model.max_tokens) {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
            body["tool_choice"] = serde_json::json!("auto");
        }

        body
    }

    pub async fn chat_completion(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<(String, Vec<crate::types::ToolCall>)> {
        let body = self.build_request_body(messages, tools, false);
        let resp = self.send_with_retry(&body).await?;
        let data: Value = resp.json().await?;

        let choice = data["choices"][0]
            .as_object()
            .bail_if_none("no choices in response")?;

        let message = &choice["message"];
        let content = message["content"].as_str().unwrap_or("").to_string();

        let mut tool_calls: Vec<crate::types::ToolCall> = if message["tool_calls"].is_array() {
            serde_json::from_value(message["tool_calls"].clone())?
        } else {
            vec![]
        };

        let mut seen = std::collections::HashSet::new();
        for (i, tc) in tool_calls.iter_mut().enumerate() {
            if tc.id.is_empty() {
                tc.id = format!("call_{}", uuid::Uuid::new_v4());
            }
            while !seen.insert(tc.id.clone()) {
                tc.id = format!("{}_{}", tc.id, i);
            }
        }

        Ok((content, tool_calls))
    }

    pub async fn chat_completion_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>>> {
        let body = self.build_request_body(messages, tools, true);
        let resp = self.send_with_retry(&body).await?;
        Ok(streaming::SseParser::parse_stream(resp))
    }

    async fn send_with_retry(&self, body: &Value) -> Result<Response> {
        let mut current_client = self;

        loop {
            // Circuit breaker check
            if let Err(msg) = current_client.circuit_breaker.acquire_permit() {
                if let Some(ref fallback) = current_client.fallback_client {
                    tracing::warn!(
                        "Circuit breaker open for model {}: {}, falling back to {}",
                        current_client.model.model_id,
                        msg,
                        fallback.model.model_id
                    );
                    current_client = fallback.as_ref();
                    continue;
                }
                bail!("Circuit breaker open: {}", msg);
            }

            let url = format!("{}/chat/completions", current_client.model.base_url);
            let base_delay = Duration::from_millis(500);
            let max_delay = Duration::from_secs(10);
            let max_retries = 3;

            let mut final_error = None;

            for attempt in 0..max_retries {
                let resp = current_client
                    .http
                    .post(&url)
                    .bearer_auth(&current_client.model.api_key)
                    .json(body)
                    .send()
                    .await;

                match resp {
                    Ok(r) if r.status().is_success() => {
                        current_client.circuit_breaker.record_success();
                        return Ok(r);
                    }
                    Ok(r) if r.status().as_u16() == 429 || r.status().is_server_error() => {
                        current_client.circuit_breaker.record_failure();
                        if attempt == max_retries - 1 {
                            final_error = Some(anyhow::anyhow!("API error {}", r.status()));
                            break;
                        }

                        let retry_after = r
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok())
                            .map(Duration::from_secs);

                        let delay = retry_after
                            .unwrap_or_else(|| calculate_backoff(attempt, base_delay, max_delay));
                        tracing::warn!(
                            "Model {} failed with {}, retrying in {:?}",
                            current_client.model.model_id,
                            r.status(),
                            delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                    Ok(r) => {
                        current_client.circuit_breaker.record_failure();
                        let status = r.status();
                        let body_text = r.text().await.unwrap_or_default();
                        final_error = Some(anyhow::anyhow!("API error {status}: {body_text}"));
                        break;
                    }
                    Err(e) => {
                        current_client.circuit_breaker.record_failure();
                        if attempt == max_retries - 1 {
                            final_error = Some(e.into());
                            break;
                        }

                        let delay = calculate_backoff(attempt, base_delay, max_delay);
                        tracing::warn!(
                            "Model {} network error: {}, retrying in {:?}",
                            current_client.model.model_id,
                            e,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }

            // If we broke out of the loop with an error, try fallback
            if let Some(ref fallback) = current_client.fallback_client {
                tracing::warn!(
                    "Model {} exhausted retries or failed, falling back to {}",
                    current_client.model.model_id,
                    fallback.model.model_id
                );
                current_client = fallback.as_ref();
                continue;
            }

            if let Some(e) = final_error {
                return Err(e);
            }
            bail!("Max retries exceeded");
        }
    }
}

trait OptionExt<T> {
    fn bail_if_none(self, msg: &str) -> Result<T>;
}

impl<T> OptionExt<T> for Option<T> {
    fn bail_if_none(self, msg: &str) -> Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("{msg}"))
    }
}
