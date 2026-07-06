use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

pub struct TavilySearchTool {
    api_key: String,
    client: reqwest::Client,
}

impl TavilySearchTool {
    pub fn new(api_key: String) -> Self {
        use std::time::Duration;
        Self {
            api_key,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("failed to build tavily http client"),
        }
    }

    /// Try to create from the TAVILY_API_KEY environment variable.
    pub fn from_env() -> Option<Self> {
        std::env::var("TAVILY_API_KEY")
            .ok()
            .map(|key| Self::new(key))
    }
}

#[async_trait]
impl Tool for TavilySearchTool {
    fn name(&self) -> &str {
        "tavily_search"
    }

    fn description(&self) -> &str {
        "A search engine optimized for AI agents. \
Pass in a natural language question (NOT a URL), and it will return a synthesized answer \
along with clean content snippets from the top relevant sources. \
Use this whenever you need to find up-to-date information on the internet."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query in natural language (e.g., 'Portugal at the 2026 FIFA World Cup standings')"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"]
            .as_str()
            .context("missing required parameter 'query'")?;

        let request_body = json!({
            "api_key": self.api_key,
            "query": query,
            "search_depth": "advanced",
            "include_answer": true,
            "include_raw_content": false,
            "max_results": 3
        });

        const MAX_RETRIES: u32 = 3;
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            let resp = self
                .client
                .post("https://api.tavily.com/search")
                .json(&request_body)
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    let response: Value = r.json().await
                        .context("Failed to parse Tavily JSON response")?;
                    // ── Parse and format ─────────────────────────
                    let mut output = String::new();
                    if let Some(answer) = response["answer"].as_str() {
                        if !answer.is_empty() {
                            output.push_str(&format!("{}\n\n", answer));
                        }
                    }
                    if let Some(results) = response["results"].as_array() {
                        if results.is_empty() {
                            return Ok("No relevant information found for this query.".to_string());
                        }
                        for (i, res) in results.iter().enumerate() {
                            let title = res["title"].as_str().unwrap_or("Unknown Title");
                            let url = res["url"].as_str().unwrap_or("Unknown URL");
                            let content = res["content"].as_str().unwrap_or("No content");
                            output.push_str(&format!(
                                "{}. **{}**\nURL: {}\nContent: {}\n\n",
                                i + 1, title, url, content
                            ));
                        }
                    } else {
                        output.push_str("Failed to extract results from API.");
                    }
                    return Ok(output);
                }
                Ok(r) => {
                    let status = r.status();
                    last_err = Some(anyhow::anyhow!("Tavily API error: {status}"));
                    if attempt + 1 < MAX_RETRIES {
                        let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
                Err(e) => {
                    last_err = Some(anyhow::anyhow!("Tavily request failed: {e}"));
                    if attempt + 1 < MAX_RETRIES {
                        let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Tavily search failed after retries")))
    }
}
