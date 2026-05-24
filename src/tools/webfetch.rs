use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL on the web. Takes a URL and returns the page content. \
         Use when you need to retrieve and analyze web content."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from (must be fully-formed and valid)"
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown", "html"],
                    "description": "The format to return the content in. Defaults to text.",
                    "default": "text"
                },
                "timeout": {
                    "type": "number",
                    "description": "Optional timeout in seconds (max 120). Defaults to 30.",
                    "default": 30
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let url = args["url"]
            .as_str()
            .context("missing required parameter 'url'")?;

        let format = args["format"].as_str().unwrap_or("text");
        let timeout_secs = args["timeout"].as_f64().unwrap_or(30.0).clamp(1.0, 120.0);

        let url_to_fetch = upgrade_to_https(url);

        let client = reqwest::Client::builder()
            .user_agent("agent_core/0.1.0")
            .timeout(std::time::Duration::from_secs_f64(timeout_secs))
            .build()
            .context("failed to create HTTP client")?;

        let response = match client.get(&url_to_fetch).send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(format_http_error(&url_to_fetch, &e));
            }
        };

        let status_code = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        if !response.status().is_success() {
            return Ok(format_http_status(url, &url_to_fetch, status_code, &response));
        }

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(format!(
                    "Failed to read response body from {url_to_fetch}: {e}\n\n\
                     The request succeeded (status {status_code}) but the response body could not be decoded. \
                     This may indicate binary content or an encoding issue. \
                     Try fetching a different URL or use a format that handles binary content."
                ));
            }
        };

        if is_garbled(&body) {
            return Ok(format!(
                "Fetched URL: {url_to_fetch}\nContent-Type: {content_type}\nStatus: {status_code}\n\n\
                 The response body appears to be binary or compressed data rather than readable text. \
                 This often happens when a site uses client-side rendering (JavaScript-only), \
                 returns gzip-compressed content without proper headers, or serves encrypted/obfuscated responses.\n\n\
                 Suggestions:\n\
                 - This site likely requires a browser to render its content and cannot be fetched directly.\n\
                 - Try an alternative source for this information (e.g., an API endpoint, RSS feed, or a simpler page).\n\
                 - Provide the information you need directly rather than asking me to fetch it from this site."
            ));
        }

        let processed = match format {
            "html" => body,
            _ => strip_html(&body),
        };

        let max_len = 100_000;
        let truncated = if processed.len() > max_len {
            let safe_end = processed.floor_char_boundary(max_len);
            let head = &processed[..safe_end];
            format!(
                "{head}...\n\n[Content truncated at {max_len} bytes. Total: {} chars / {} bytes. Consider using a more specific URL.]",
                processed.chars().count(),
                processed.len()
            )
        } else {
            processed
        };

        Ok(format!(
            "Fetched URL: {url_to_fetch}\nContent-Type: {content_type}\nStatus: {status_code}\n\n{truncated}",
        ))
    }
}

fn upgrade_to_https(url: &str) -> String {
    if let Some(stripped) = url.strip_prefix("http://") {
        format!("https://{stripped}")
    } else {
        url.to_string()
    }
}

fn format_http_error(url: &str, error: &reqwest::Error) -> String {
    let base = format!("Failed to fetch {url}.");

    if error.is_timeout() {
        format!(
            "{base}\nReason: Request timed out. The server took too long to respond.\n\
             Suggestion: Try again later, increase the timeout parameter, or try a different URL."
        )
    } else if error.is_connect() {
        format!(
            "{base}\nReason: Could not connect to the server. The site may be down, \
             unreachable, or blocking connections.\n\
             Suggestion: Verify the URL is correct. The site may not exist, \
             may be behind a firewall, or may block automated access. \
             Try searching for this information through other sources."
        )
    } else if error.is_redirect() {
        format!(
            "{base}\nReason: Too many redirects or a redirect loop.\n\
             Suggestion: The URL may have moved permanently. Check if a newer or direct URL is available."
        )
    } else if error.is_body() || error.is_decode() {
        format!(
            "{base}\nReason: Error reading or decoding the response body.\n\
             Suggestion: The response may be binary or in an unexpected format. \
             Try a different page or resource."
        )
    } else {
        let err_str = error.to_string();
        let suggestion = if err_str.contains("dns") || err_str.contains("resolve") || err_str.contains("Name") {
            "\nSuggestion: The domain name could not be resolved. Verify the URL is correct."
        } else if err_str.contains("certificate") || err_str.contains("SSL") || err_str.contains("TLS") || err_str.contains("tls") {
            "\nSuggestion: SSL/TLS certificate error. The site's certificate may be invalid or self-signed. Try using http:// if appropriate."
        } else {
            "\nSuggestion: This is a network-level error. Verify the URL, check your internet connection, and try again."
        };

        format!("{base}\nReason: {err_str}{suggestion}")
    }
}

fn format_http_status(original_url: &str, final_url: &str, status_code: u16, response: &reqwest::Response) -> String {
    let reason = response.status().canonical_reason().unwrap_or("unknown");

    let (explanation, suggestion) = match status_code {
        403 | 401 => (
            "The server refused access (authentication required or access forbidden).",
            "This site blocks automated access or requires login credentials. \
             Try a different source for this information, use a publicly accessible page, \
             or search for an alternative mirror of the content.",
        ),
        404 => (
            "The requested page was not found on the server.",
            "The URL is invalid, the page has been removed, or the link is outdated. \
             Check the URL spelling or look for the content elsewhere.",
        ),
        429 => (
            "The server is rate-limiting requests (too many requests in a short time).",
            "Wait before retrying, or try a different source. \
             The site is throttling automated access.",
        ),
        451 => (
            "The content is unavailable for legal reasons in your region.",
            "Try using a different source or an alternative page that serves the same information.",
        ),
        500..=599 => (
            "The server encountered an internal error.",
            "This is a server-side problem. Try again later or use a different source.",
        ),
        300..=399 => (
            "The server redirected, but the redirect target may not be reachable or requires different handling.",
            "Try the redirect target URL directly, or use a different source.",
        ),
        _ => {
            if status_code >= 400 {
                ("The server returned a client or server error.", "Try a different URL or source for this information.")
            } else {
                ("The server returned an unexpected non-success status.", "Try again or use an alternative source.")
            }
        }
    };

    let redirect_note = if original_url != final_url {
        format!("\nNote: The request was redirected to {final_url}.")
    } else {
        String::new()
    };

    format!(
        "Failed to fetch {final_url} (status {status_code} {reason}).\n\
         Explanation: {explanation}\n\
         Suggestion: {suggestion}{redirect_note}"
    )
}

fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_style_script = false;
    let mut tag_name = String::new();

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag_name.clear();
        } else if ch == '>' {
            if in_tag {
                in_tag = false;
                let tag_lower = tag_name.to_lowercase();
                if tag_lower == "script" || tag_lower == "style" {
                    in_style_script = true;
                } else if tag_lower == "/script" || tag_lower == "/style" {
                    in_style_script = false;
                } else if tag_lower == "br" || tag_lower == "hr" {
                    result.push('\n');
                } else if tag_lower == "p"
                    || tag_lower.starts_with("/h")
                    || tag_lower == "/div"
                    || tag_lower == "/li"
                    || tag_lower == "/tr"
                {
                    result.push('\n');
                }
                tag_name.clear();
            } else {
                result.push(ch);
            }
        } else if in_tag {
            tag_name.push(ch);
        } else if !in_style_script {
            result.push(ch);
        }
    }

    let mut cleaned = String::with_capacity(result.len());
    let mut prev_blank = false;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                cleaned.push('\n');
                prev_blank = true;
            }
        } else {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
            prev_blank = false;
        }
    }

    cleaned.trim().to_string()
}

fn is_garbled(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }

    let sample = if text.len() > 2000 { &text[..text.floor_char_boundary(2000)] } else { text };

    let total = sample.chars().count() as f64;
    if total < 10.0 {
        return false;
    }

    let printable = sample
        .chars()
        .filter(|&c| {
            c.is_ascii_alphanumeric()
                || c.is_ascii_punctuation()
                || c.is_ascii_whitespace()
                || c == '\n'
                || c == '\r'
                || c == '\t'
                || c as u32 > 127
        })
        .count() as f64;

    let good_chars = sample
        .chars()
        .filter(|&c| {
            c.is_ascii_alphanumeric()
                || c.is_ascii_whitespace()
                || c == '\n'
                || c == '\r'
                || c == '\t'
        })
        .count() as f64;

    let printable_ratio = printable / total;
    let good_ratio = good_chars / total;

    good_ratio < 0.3 && printable_ratio < 0.5
}
