use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::cookie::Jar;
use rand::Rng;
use serde_json::{Value, json};

use super::Tool;

mod error;
mod html;

use error::{format_http_error, format_http_status, is_image_content_type, upgrade_to_https};
use html::{build_browser_headers, check_robots, extract_meta, extract_readable};

// ─────────────────────────────────────────────────────────────────────
// User-Agent profiles — each UA comes with matching Client Hints
// so the full fingerprint is consistent. Weights control selection.
// ─────────────────────────────────────────────────────────────────────

struct UaProfile {
    ua: &'static str,
    brands: &'static str,   // sec-ch-ua
    platform: &'static str, // sec-ch-ua-platform
    mobile: &'static str,   // sec-ch-ua-mobile
    weight: f64,
}

/// Heavily weighted toward Chrome desktop (~70%) — the most common
/// real browser fingerprint on the web today.
const UA_PROFILES: &[UaProfile] = &[
    // ── Chrome 126 Desktop ────────────────────────────────────────
    UaProfile {
        ua: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        brands: "\"Not/A)Brand\";v=\"99\", \"Google Chrome\";v=\"126\", \"Chromium\";v=\"126\"",
        platform: "\"macOS\"",
        mobile: "?0",
        weight: 0.30,
    },
    UaProfile {
        ua: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        brands: "\"Not/A)Brand\";v=\"99\", \"Google Chrome\";v=\"126\", \"Chromium\";v=\"126\"",
        platform: "\"Windows\"",
        mobile: "?0",
        weight: 0.25,
    },
    UaProfile {
        ua: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        brands: "\"Not/A)Brand\";v=\"99\", \"Google Chrome\";v=\"126\", \"Chromium\";v=\"126\"",
        platform: "\"Linux\"",
        mobile: "?0",
        weight: 0.15,
    },
    // ── Firefox Desktop ───────────────────────────────────────────
    UaProfile {
        ua: "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:127.0) Gecko/20100101 Firefox/127.0",
        brands: "\"Firefox\";v=\"127\", \"Not)A;Brand\";v=\"99\"",
        platform: "\"Windows\"",
        mobile: "?0",
        weight: 0.10,
    },
    UaProfile {
        ua: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:127.0) Gecko/20100101 Firefox/127.0",
        brands: "\"Firefox\";v=\"127\", \"Not)A;Brand\";v=\"99\"",
        platform: "\"macOS\"",
        mobile: "?0",
        weight: 0.05,
    },
    // ── Safari Desktop ───────────────────────────────────────────
    UaProfile {
        ua: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
        brands: "\"Safari\";v=\"17.5\", \"Not)A;Brand\";v=\"99\"",
        platform: "\"macOS\"",
        mobile: "?0",
        weight: 0.07,
    },
    // ── Mobile (small weight — keep desktop-dominant) ─────────────
    UaProfile {
        ua: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
        brands: "\"Safari\";v=\"17.5\", \"Not)A;Brand\";v=\"99\"",
        platform: "\"iOS\"",
        mobile: "?1",
        weight: 0.04,
    },
    UaProfile {
        ua: "Mozilla/5.0 (Linux; Android 14; Pixel 8 Pro) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.6478.50 Mobile Safari/537.36",
        brands: "\"Not/A)Brand\";v=\"99\", \"Google Chrome\";v=\"126\", \"Chromium\";v=\"126\"",
        platform: "\"Android\"",
        mobile: "?1",
        weight: 0.04,
    },
];

/// Pick a weighted-random UA profile.
fn pick_ua_profile() -> &'static UaProfile {
    let total: f64 = UA_PROFILES.iter().map(|p| p.weight).sum();
    let mut rng = rand::thread_rng();
    let mut roll: f64 = rng.r#gen();
    roll *= total;
    let mut accum = 0.0;
    for p in UA_PROFILES {
        accum += p.weight;
        if roll <= accum {
            return p;
        }
    }
    // Fallback (should not reach due to floating-point, but just in case)
    &UA_PROFILES[0]
}

// ─────────────────────────────────────────────────────────────────────
// Per-domain rate limiter — enforces a random delay between requests
// to the same domain, mimicking human browsing cadence.
// ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct DomainRateLimiter {
    last_access: Arc<Mutex<HashMap<String, Instant>>>,
    min_delay: Duration,
    max_delay: Duration,
}

impl DomainRateLimiter {
    fn new(min_s: f64, max_s: f64) -> Self {
        Self {
            last_access: Arc::new(Mutex::new(HashMap::new())),
            min_delay: Duration::from_secs_f64(min_s),
            max_delay: Duration::from_secs_f64(max_s),
        }
    }

    /// Extracts the host from a URL string.
    fn extract_host(url_str: &str) -> Result<String> {
        let parsed = url::Url::parse(url_str)?;
        Ok(parsed.host_str().unwrap_or("unknown").to_string())
    }

    /// Waits if needed to satisfy the per-domain rate limit, then
    /// updates the last-access time. Returns the actual delay waited.
    async fn wait(&self, url_str: &str) -> Option<Duration> {
        let host = Self::extract_host(url_str).ok()?;

        let required_delay = {
            let map = self.last_access.lock().unwrap();
            if let Some(last) = map.get(&host) {
                let required = self.random_delay();
                let elapsed = last.elapsed();
                if elapsed < required {
                    Some(required - elapsed)
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(d) = required_delay {
            tokio::time::sleep(d).await;
        }

        // Record access time
        {
            let mut map = self.last_access.lock().unwrap();
            map.insert(host, Instant::now());
        }

        required_delay
    }

    /// Generate a random delay between min_delay and max_delay.
    fn random_delay(&self) -> Duration {
        let mut rng = rand::thread_rng();
        Duration::from_secs_f64(
            rng.gen_range(self.min_delay.as_secs_f64()..self.max_delay.as_secs_f64()),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────
// WebFetchTool — now with state: cookie jar + rate limiter + redirect
// tracking.
// ─────────────────────────────────────────────────────────────────────

pub struct WebFetchTool {
    cookie_store: Arc<Jar>,
    rate_limiter: DomainRateLimiter,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            cookie_store: Arc::new(Jar::default()),
            rate_limiter: DomainRateLimiter::new(2.0, 5.0),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

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

        // ── Rate limiting ───────────────────────────────────────────
        let rate_delay = self.rate_limiter.wait(url).await;

        // ── Pick a weighted random UA + Client Hints ────────────────
        let profile = pick_ua_profile();

        // ── Build browser-like headers ──────────────────────────────
        let headers = build_browser_headers(profile, &url_to_fetch);

        let client = reqwest::Client::builder()
            .user_agent(profile.ua)
            .default_headers(headers)
            .cookie_provider(self.cookie_store.clone())
            .timeout(std::time::Duration::from_secs_f64(timeout_secs))
            .gzip(true)
            .brotli(true)
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()
            .context("failed to create HTTP client")?;

        // ── Robots.txt check ────────────────────────────────────────
        if let Ok(parsed_url) = url::Url::parse(&url_to_fetch) {
            check_robots(&client, &parsed_url, &url_to_fetch).await?;
        }

        // ── Fetch with redirect chain tracking ──────────────────────
        let response = match client.get(&url_to_fetch).send().await {
            Ok(r) => r,
            Err(e) => return Ok(format_http_error(&url_to_fetch, rate_delay, &e)),
        };

        let status_code = response.status().as_u16();
        let final_url = response.url().to_string();
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
                &final_url,
                status_code,
                &response,
            ));
        }

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(format!(
                    "Failed to read response body from {final_url}: {e}\n\n\
                     The request succeeded (status {status_code}) but the response body \
                     could not be decoded. This may indicate binary content or an encoding \
                     issue. Try fetching a different URL or use a format that handles \
                     binary content."
                ));
            }
        };

        if is_image_content_type(&content_type) {
            return Ok(format!(
                "Fetched URL: {final_url}\nContent-Type: {content_type}\nStatus: {status_code}\n\n\
                 The URL points to an image file. I cannot process or display images. \
                 Try fetching a web page (HTML) instead, or find the information in text form."
            ));
        }

        // ── Content extraction pipeline ─────────────────────────────
        // 1. Extract meta tags (title, description, og:tags) from raw HTML
        let meta = extract_meta(&body);

        // 2. Readability: extract main article content from noisy HTML
        // 3. htmd: convert clean HTML to Markdown (token-efficient)
        let extracted = extract_readable(&body, &final_url);

        // ── Truncation ──────────────────────────────────────────────
        let max_len = 80_000;
        let truncated = if extracted.len() > max_len {
            let safe_end = extracted.floor_char_boundary(max_len);
            let head = &extracted[..safe_end];
            format!(
                "{head}...\n\n[Content truncated at {max_len} chars. \
                 Total: {} chars. Consider using a more specific URL.]",
                extracted.chars().count()
            )
        } else {
            extracted
        };

        let mut output = String::new();

        // ── Meta info header ────────────────────────────────────────
        if !meta.is_empty() {
            output.push_str(&meta);
            output.push_str("\n");
        }

        // ── Redirect info ───────────────────────────────────────────
        if url_to_fetch != final_url {
            output.push_str(&format!(
                "Redirected: {url_to_fetch} → {final_url}\n\n"
            ));
        }

        output.push_str(&format!(
            "Fetched: {final_url}\nStatus: {status_code}\n\n{truncated}",
        ));

        Ok(output)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Submodules html and error hold browser emulation, robots check, and error formatting.
// ─────────────────────────────────────────────────────────────────────
