pub mod streaming;

use crate::config::{ModelConfig, RuntimeOverrides};
use crate::types::{Message, StreamEvent, ToolDefinition};
use anyhow::{Result, bail};
use reqwest::Response;
use serde_json::Value;
use std::time::Duration;

pub struct OpenAIClient {
    http: reqwest::Client,
    pub(crate) model: ModelConfig,
    pub(crate) overrides: RuntimeOverrides,
}

impl OpenAIClient {
    pub fn new(model: ModelConfig) -> Self {
        let timeout = Duration::from_secs(model.request_timeout_secs);
        // Disable automatic gzip/deflate decompression — some providers (e.g. volces)
        // return malformed Content-Encoding headers that cause "error decoding response body"
        // in reqwest's chunk decoder.
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(15))
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .build()
            .expect("failed to build http client");

        Self {
            http,
            model,
            overrides: RuntimeOverrides {
                temperature: None,
                max_tokens: None,
            },
        }
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

        let tool_calls: Vec<crate::types::ToolCall> = if message["tool_calls"].is_array() {
            serde_json::from_value(message["tool_calls"].clone())?
        } else {
            vec![]
        };

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
        let url = format!("{}/chat/completions", self.model.base_url);
        let mut backoff = Duration::from_millis(500);
        let max_retries = 3;

        for attempt in 0..max_retries {
            let resp = self
                .http
                .post(&url)
                .bearer_auth(&self.model.api_key)
                .json(body)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => return Ok(r),
                Ok(r) if r.status().as_u16() == 429 || r.status().is_server_error() => {
                    let retry_after = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(Duration::from_secs);

                    // retry silently — error will flow through Result if needed
                    tokio::time::sleep(retry_after.unwrap_or(backoff)).await;
                    backoff *= 2;
                }
                Ok(r) => {
                    let status = r.status();
                    let body_text = r.text().await.unwrap_or_default();
                    bail!("API error {status}: {body_text}");
                }
                Err(e) if attempt < max_retries - 1 => {
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(e) => return Err(e.into()),
            }
        }

        bail!("max retries ({max_retries}) exceeded")
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
