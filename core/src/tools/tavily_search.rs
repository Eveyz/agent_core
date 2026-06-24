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
        Self {
            api_key,
            client: reqwest::Client::new(),
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

        let response: Value = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send HTTP request to Tavily")?
            .json()
            .await
            .context("Failed to parse Tavily JSON response")?;

        // ── Parse and format ──────────────────────────────────────────
        let mut output = String::new();

        if let Some(answer) = response["answer"].as_str() {
            output.push_str(&format!("### Direct Answer:\n{}\n\n", answer));
        }

        output.push_str("### Sources & Snippets:\n");

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
                    i + 1,
                    title,
                    url,
                    content
                ));
            }
        } else {
            output.push_str("Failed to extract results from API.");
        }

        Ok(output)
    }
}
