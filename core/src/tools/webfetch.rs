use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:127.0) Gecko/20100101 Firefox/127.0",
];

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch and extract readable content from a web page. \
Returns clean Markdown text with navigation, ads, and scripts removed. \
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

        let timeout_secs = args["timeout"].as_f64().unwrap_or(30.0).clamp(1.0, 120.0);
        let url_to_fetch = upgrade_to_https(url);

        // Pick a random User-Agent
        let ua = USER_AGENTS[rand::random::<usize>() % USER_AGENTS.len()];

        let client = reqwest::Client::builder()
            .user_agent(ua)
            .timeout(std::time::Duration::from_secs_f64(timeout_secs))
            .gzip(true)
            .brotli(true)
            .build()
            .context("failed to create HTTP client")?;

        let response = match client.get(&url_to_fetch).send().await {
            Ok(r) => r,
            Err(e) => return Ok(format_http_error(&url_to_fetch, &e)),
        };

        let status_code = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        if !response.status().is_success() {
            return Ok(format_http_status(
                url,
                &url_to_fetch,
                status_code,
                &response,
            ));
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

        if is_image_content_type(&content_type) {
            return Ok(format!(
                "Fetched URL: {url_to_fetch}\nContent-Type: {content_type}\nStatus: {status_code}\n\n\
                 The URL points to an image file. I cannot process or display images. \
                 Try fetching a web page (HTML) instead, or find the information in text form."
            ));
        }

        // ── Content extraction pipeline ──────────────────────────────
        // 1. Readability: extract main article content from noisy HTML
        // 2. htmd: convert clean HTML to Markdown (token-efficient)
        let extracted = extract_readable(&body, &url_to_fetch);

        let max_len = 80_000;
        let truncated = if extracted.len() > max_len {
            let safe_end = extracted.floor_char_boundary(max_len);
            let head = &extracted[..safe_end];
            format!(
                "{head}...\n\n[Content truncated at {max_len} chars. Total: {} chars. Consider using a more specific URL.]",
                extracted.chars().count()
            )
        } else {
            extracted
        };

        Ok(format!(
            "Fetched: {url_to_fetch}\nStatus: {status_code}\n\n{truncated}",
        ))
    }
}

/// Run the extraction pipeline:
///   raw HTML → Readability (article body) → htmd (Markdown)
fn extract_readable(html: &str, url: &str) -> String {
    // Step 1: Readability — extract the main article content
    let article_html = match run_readability(html, url) {
        Some(h) if !h.trim().is_empty() => h,
        _ => {
            // Fallback: if Readability yields nothing, use the full HTML
            // but strip obvious noise
            html.to_string()
        }
    };

    // Step 2: Convert clean HTML to Markdown
    match htmd::convert(&article_html) {
        Ok(md) => {
            let cleaned = md.trim().to_string();
            if cleaned.len() > 200 {
                cleaned
            } else {
                // Markdown is suspiciously short — likely a JS-only SPA.
                // Return a helpful message instead of empty content.
                format!(
                    "[Extracted content is very short ({len} chars). \
                     This page may require JavaScript to render its content. \
                     Consider finding the information through an alternative source.]\n\n{cleaned}",
                    len = cleaned.len()
                )
            }
        }
        Err(e) => {
            // htmd failed — return the raw HTML stripped of tags as last resort
            format!(
                "[Warning: Markdown conversion failed ({e}). Falling back to plain text.]\n\n{}",
                strip_html_tags(&article_html)
            )
        }
    }
}

/// Run Mozilla Readability to extract the article body.
fn run_readability(html: &str, url: &str) -> Option<String> {
    let url_parsed = url::Url::parse(url).ok()?;
    let mut cursor = std::io::Cursor::new(html.as_bytes());
    let product = readability::extractor::extract(&mut cursor, &url_parsed).ok()?;

    let title = product.title.trim();
    let content = product.content.trim();

    if content.is_empty() {
        return None;
    }

    // Prepend title if available
    let mut output = String::new();
    if !title.is_empty() {
        output.push_str(&format!("<h1>{}</h1>\n", title));
    }
    output.push_str(content);
    Some(output)
}

/// Last-resort HTML tag stripper.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script_style = false;
    let mut tag = String::new();

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag.clear();
        } else if ch == '>' {
            if in_tag {
                in_tag = false;
                let t = tag.to_lowercase();
                if t == "script" || t == "style" {
                    in_script_style = true;
                } else if t == "/script" || t == "/style" {
                    in_script_style = false;
                } else if t == "br"
                    || t == "hr"
                    || t == "p"
                    || t.starts_with("/h")
                    || t == "/div"
                    || t == "/li"
                    || t == "/tr"
                {
                    out.push('\n');
                }
                tag.clear();
            } else {
                out.push(ch);
            }
        } else if in_tag {
            tag.push(ch);
        } else if !in_script_style {
            out.push(ch);
        }
    }

    // Collapse multiple blank lines
    let mut cleaned = String::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out.lines() {
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

fn upgrade_to_https(url: &str) -> String {
    if let Some(stripped) = url.strip_prefix("http://") {
        format!("https://{stripped}")
    } else {
        url.to_string()
    }
}

fn is_image_content_type(ct: &str) -> bool {
    ct.to_lowercase().starts_with("image/")
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
        let suggestion = if err_str.contains("dns")
            || err_str.contains("resolve")
            || err_str.contains("Name")
        {
            "\nSuggestion: The domain name could not be resolved. Verify the URL is correct."
        } else if err_str.contains("certificate")
            || err_str.contains("SSL")
            || err_str.contains("TLS")
            || err_str.contains("tls")
        {
            "\nSuggestion: SSL/TLS certificate error. The site's certificate may be invalid or self-signed. Try using http:// if appropriate."
        } else {
            "\nSuggestion: This is a network-level error. Verify the URL, check your internet connection, and try again."
        };

        format!("{base}\nReason: {err_str}{suggestion}")
    }
}

fn format_http_status(
    original_url: &str,
    final_url: &str,
    status_code: u16,
    response: &reqwest::Response,
) -> String {
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
                (
                    "The server returned a client or server error.",
                    "Try a different URL or source for this information.",
                )
            } else {
                (
                    "The server returned an unexpected non-success status.",
                    "Try again or use an alternative source.",
                )
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
