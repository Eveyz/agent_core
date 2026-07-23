pub mod resilience;
pub mod streaming;
pub mod providers;

use crate::client::resilience::{CircuitBreaker, CircuitBreakerConfig, calculate_backoff};
use crate::config::{ApiMode, ModelConfig, RuntimeOverrides};
use crate::types::{Message, StreamEvent, ToolDefinition};
use anyhow::{Result, bail};
use reqwest::Response;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Cache hints handed to the client so it can (a) emit cache telemetry for
/// verification and (b) attach provider-specific cache markers
/// (e.g. Anthropic `cache_control`). Computed by `ContextEngine::cache_hint()`
/// and copied into this lightweight, dependency-free struct to avoid coupling
/// the client module to the context module.
#[derive(Clone, Copy)]
pub struct ClientCacheHint {
    pub stable_prefix_tokens: usize,
    pub can_reuse_cache: bool,
    pub strategy: &'static str,
    /// System + conversation history tokens that form a stable prefix
    /// (excludes the per-turn context injection that changes every turn).
    pub cacheable_prefix_tokens: usize,
    /// Milliseconds since the last turn (0 = first turn).
    pub last_turn_elapsed_ms: u64,
    /// True when the idle gap likely exceeds the provider's KV cache TTL.
    pub expected_cold_miss: bool,
}

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
            .timeout(Duration::from_secs(timeout_secs.saturating_add(30)))
            .read_timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(32)
            .tcp_keepalive(Duration::from_secs(30))
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .no_proxy()
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
        cache_hint: Option<ClientCacheHint>,
        required_tool: Option<&str>,
    ) -> Value {
        let api_mode = self.model.resolved_api_mode();
        let temperature = self.overrides.temperature.or(self.model.temperature);
        let max_tokens = self.overrides.max_tokens.or(self.model.max_tokens);

        let mut body = providers::build_provider_body(
            api_mode,
            &self.model.model_id,
            messages,
            tools,
            stream,
            temperature,
            max_tokens,
            self.model.thinking_enabled,
            self.model.reasoning_effort.as_deref(),
            self.model.supports_images(),
        );
        providers::apply_required_tool_choice(&mut body, api_mode, required_tool);

        // NVIDIA's API gateway wraps DeepSeek models behind a chat_template_kwargs
        // translation layer.  Sending top-level `thinking` / `reasoning_effort`
        // produces a 400 — they must live inside `chat_template_kwargs`.
        // Only applies to Chat Completions wire format.
        if api_mode == ApiMode::ChatCompletions {
            let is_nvidia = self.model.base_url.contains("nvidia.com");
            if is_nvidia {
                let mut ctk = serde_json::Map::new();
                if let Some(ref effort) = self.model.reasoning_effort {
                    ctk.insert("reasoning_effort".to_string(), serde_json::json!(effort));
                }
                if self.model.thinking_enabled {
                    ctk.insert("thinking".to_string(), serde_json::json!(true));
                }
                if !ctk.is_empty() {
                    body["chat_template_kwargs"] = serde_json::Value::Object(ctk);
                    body.as_object_mut().map(|o| {
                        o.remove("thinking");
                        o.remove("reasoning_effort");
                    });
                }
            } else {
                if let Some(effort) = &self.model.reasoning_effort {
                    body["reasoning_effort"] = serde_json::json!(effort);
                }
                if self.model.thinking_enabled {
                    body["thinking"] = serde_json::json!({
                        "type": "enabled"
                    });
                }
            }
        }

        // ── KV cache hint wiring ────────────────────────────────────
        if let Some(hint) = cache_hint {
            tracing::info!(
                target: "kv_cache",
                provider = %self.model.base_url,
                api_mode = ?api_mode,
                stable_prefix_tokens = hint.stable_prefix_tokens,
                cacheable_prefix_tokens = hint.cacheable_prefix_tokens,
                last_turn_elapsed_ms = hint.last_turn_elapsed_ms,
                expected_cold_miss = hint.expected_cold_miss,
                strategy = hint.strategy,
                can_reuse = hint.can_reuse_cache,
                "KV cache hint for this request"
            );

            if api_mode == ApiMode::AnthropicMessages
                && hint.can_reuse_cache
                && hint.strategy != "none"
            {
                // Anthropic: mark system with ephemeral cache_control when possible.
                if let Some(sys) = body.get_mut("system") {
                    if let Some(text) = sys.as_str() {
                        *sys = serde_json::json!([
                            {
                                "type": "text",
                                "text": text,
                                "cache_control": { "type": "ephemeral" }
                            }
                        ]);
                    }
                }
            } else if api_mode == ApiMode::ChatCompletions {
                let is_anthropic = self.model.base_url.contains("anthropic.com")
                    || self.model.base_url.contains("api.anthropic");
                if is_anthropic && hint.can_reuse_cache && hint.strategy != "none" {
                    if let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
                        for m in msgs.iter_mut() {
                            if m.get("role").and_then(|r| r.as_str()) == Some("system") {
                                if let Some(text) = m.get("content").and_then(|c| c.as_str()) {
                                    m["content"] = serde_json::json!([
                                        {
                                            "type": "text",
                                            "text": text,
                                            "cache_control": { "type": "ephemeral" }
                                        }
                                    ]);
                                }
                            }
                        }
                    }
                }
            }
        }

        body
    }

    pub async fn chat_completion(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<(String, Vec<crate::types::ToolCall>)> {
        self.chat_completion_with_hint(messages, tools, None).await
    }

    /// Like [`chat_completion`](Self::chat_completion) but carries the KV cache
    /// hint so the client can emit cache telemetry and attach provider-specific
    /// cache markers.
    pub async fn chat_completion_with_hint(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        cache_hint: Option<ClientCacheHint>,
    ) -> Result<(String, Vec<crate::types::ToolCall>)> {
        let body = self.build_request_body(messages, tools, false, cache_hint, None);
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
        self.chat_completion_stream_with_hint(messages, tools, None).await
    }

    /// Like [`chat_completion_stream`](Self::chat_completion_stream) but carries
    /// the KV cache hint (see [`chat_completion_with_hint`]).
    pub async fn chat_completion_stream_with_hint(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        cache_hint: Option<ClientCacheHint>,
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>>> {
        self.chat_completion_stream_with_hint_and_required_tool(
            messages,
            tools,
            cache_hint,
            None,
        )
        .await
    }

    /// Stream a completion while requiring the provider to call one named
    /// tool. Existing callers remain on automatic tool selection.
    pub async fn chat_completion_stream_with_hint_and_required_tool(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        cache_hint: Option<ClientCacheHint>,
        required_tool: Option<&str>,
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>>> {
        let http_t0 = std::time::Instant::now();
        let body = self.build_request_body(messages, tools, true, cache_hint, required_tool);
        let resp = self.send_with_retry(&body).await?;
        let status = resp.status();
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(none)");
        // info: one line per request — useful in prod for provider TTFB.
        tracing::info!(
            %status,
            content_type = %ct,
            http_ms = http_t0.elapsed().as_millis() as u64,
            model = %self.model.model_id,
            "LATENCY: LLM stream HTTP ready"
        );
        if !ct.starts_with("text/event-stream") {
            tracing::warn!(%status, %ct, "unexpected content-type for SSE stream");
        }
        Ok(streaming::SseParser::parse_stream(resp))
    }

    async fn send_with_retry(&self, body: &Value) -> Result<Response> {
        let mut current_client = self;

        loop {
            // Circuit breaker check — pause requests after repeated provider failures
            // so we don't hammer a dead endpoint. Fall back to the next model if set.
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
                bail!("{}", msg);
            }

            let url = providers::endpoint_for(
                current_client.model.resolved_api_mode(),
                &current_client.model.base_url,
            );
            let base_delay = Duration::from_millis(500);
            let max_delay = Duration::from_secs(60);
            // Rate-limit retries (429 only) — separate from general network retries.
            // Transient 5xx / connection errors used to get only 3 attempts, which was too
            // few for flaky providers (a 503 is usually a brief overload). Bumped to match
            // the rate-limit budget (10) so behaviour is consistent across transient failures.
            // `calculate_backoff` still caps each wait at 60s, so the worst case is bounded.
            let rate_limit_max_retries = 10usize;
            let network_max_retries = 10usize;

            let mut final_error = None;
            let mut rate_limit_attempts = 0usize;
            let mut network_attempts = 0usize;

            loop {
                if rate_limit_attempts >= rate_limit_max_retries || network_attempts >= network_max_retries {
                    break;
                }

                let mut req = current_client
                    .http
                    .post(&url)
                    .json(body);

                // Anthropic Messages uses x-api-key + version header, not Bearer.
                if current_client.model.resolved_api_mode() == ApiMode::AnthropicMessages {
                    req = req
                        .header("x-api-key", &current_client.model.api_key)
                        .header("anthropic-version", "2023-06-01");
                } else {
                    req = req.bearer_auth(&current_client.model.api_key);
                }

                let resp = req.send().await;

                match resp {
                    Ok(r) if r.status().is_success() => {
                        current_client.circuit_breaker.record_success();
                        return Ok(r);
                    }
                    Ok(r) if r.status().as_u16() == 429 => {
                        current_client.circuit_breaker.record_failure();
                        rate_limit_attempts += 1;
                        if rate_limit_attempts >= rate_limit_max_retries {
                            final_error = Some(anyhow::anyhow!(
                                "The AI model service is rate-limiting requests right now (HTTP 429). I retried several times but it is still busy — please wait a moment and try again."
                            ));
                            break;
                        }

                        let retry_after = r
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok())
                            .map(Duration::from_secs);

                        let delay = retry_after
                            .unwrap_or_else(|| calculate_backoff(rate_limit_attempts as u32, base_delay, max_delay));
                        tracing::warn!(
                            "Model {} 429 rate limited, attempt {}/{}, retrying in {:?}",
                            current_client.model.model_id,
                            rate_limit_attempts,
                            rate_limit_max_retries,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                    Ok(r) if r.status().is_server_error() => {
                        current_client.circuit_breaker.record_failure();
                        network_attempts += 1;
                        if network_attempts >= network_max_retries {
                            final_error = Some(anyhow::anyhow!(
                                "The AI model service is temporarily unavailable (it returned a {} error). I retried several times but it is still not responding — this is usually a brief overload on the provider's side, so please try again in a minute.",
                                r.status()
                            ));
                            break;
                        }
                        let delay = calculate_backoff(network_attempts as u32, base_delay, max_delay);
                        tracing::warn!(
                            "Model {} server error {}, attempt {}/{}, retrying in {:?}",
                            current_client.model.model_id,
                            r.status(),
                            network_attempts,
                            network_max_retries,
                            delay
                        );
                        tokio::time::sleep(delay).await;
                    }
                    Ok(r) => {
                        current_client.circuit_breaker.record_failure();
                        let status = r.status();
                        let body_text = r.text().await.unwrap_or_default();
                        // Trim the provider's raw error body so it stays readable in the UI.
                        let detail = if body_text.chars().count() > 300 {
                            format!("{}…", body_text.chars().take(300).collect::<String>())
                        } else {
                            body_text
                        };
                        final_error = Some(anyhow::anyhow!(
                            "The AI model service rejected the request (HTTP {}). This is usually a configuration problem — e.g. an invalid API key, model name, or request parameter. Details: {}",
                            status, detail
                        ));
                        break;
                    }
                    Err(e) => {
                        current_client.circuit_breaker.record_failure();
                        network_attempts += 1;
                        if network_attempts >= network_max_retries {
                            final_error = Some(anyhow::anyhow!(
                                "I couldn't reach the AI model service (a network error occurred: {}). I retried several times without success. Please check your internet connection and the provider's status, then try again.",
                                e
                            ));
                            break;
                        }
                        let delay = calculate_backoff(network_attempts as u32, base_delay, max_delay);
                        tracing::warn!(
                            "Model {} network error: {}, attempt {}/{}, retrying in {:?}",
                            current_client.model.model_id,
                            e,
                            network_attempts,
                            network_max_retries,
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
