use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::cookie::Jar;
use rand::Rng;
use serde_json::{Value, json};

use super::Tool;
use reqwest::header;

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
// Header construction — full browser emulation with Client Hints
// ─────────────────────────────────────────────────────────────────────

fn build_browser_headers(profile: &UaProfile, _url: &str) -> header::HeaderMap {
    let mut h = header::HeaderMap::new();

    // ── Standard browser headers ────────────────────────────────────
    h.insert(
        header::ACCEPT,
        header::HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,\
             image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
        ),
    );
    h.insert(
        header::ACCEPT_LANGUAGE,
        header::HeaderValue::from_static("en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7"),
    );
    h.insert(
    "Accept-Encoding",
    header::HeaderValue::from_static("gzip, deflate, br"),
    );
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("max-age=0"),
    );
    // Some sites still check Pragma for HTTP/1.0 intermediaries
    h.insert("Pragma", header::HeaderValue::from_static("no-cache"));
    h.insert("DNT", header::HeaderValue::from_static("1"));
    h.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("keep-alive"),
    );
    h.insert(
        "Upgrade-Insecure-Requests",
        header::HeaderValue::from_static("1"),
    );

    // ── Client Hints (matching the UA profile) ──────────────────────
    h.insert(
        "Sec-Ch-Ua",
        header::HeaderValue::from_str(profile.brands)
            .unwrap_or(header::HeaderValue::from_static("\"Chromium\";v=\"126\"")),
    );
    h.insert(
        "Sec-Ch-Ua-Mobile",
        header::HeaderValue::from_str(profile.mobile)
            .unwrap_or(header::HeaderValue::from_static("?0")),
    );
    h.insert(
        "Sec-Ch-Ua-Platform",
        header::HeaderValue::from_str(profile.platform)
            .unwrap_or(header::HeaderValue::from_static("\"macOS\"")),
    );

    // ── Sec-Fetch headers (document-level navigation) ──────────────
    h.insert("Sec-Fetch-Dest", header::HeaderValue::from_static("document"));
    h.insert("Sec-Fetch-Mode", header::HeaderValue::from_static("navigate"));
    // "none" for direct entry (no referrer); switches to "same-origin"
    // or "cross-site" if we had a referrer chain.
    h.insert("Sec-Fetch-Site", header::HeaderValue::from_static("none"));
    // Real Chrome also sends Sec-Fetch-User: ?1 on user-initiated navigations
    h.insert("Sec-Fetch-User", header::HeaderValue::from_static("?1"));

    // ── Referrer (set only when we have a prior URL) ────────────────
    // For direct fetches we don't send a referrer — "none" Sec-Fetch-Site
    // is consistent with this. If we later track referrer chains, set
    // the REFERER header to the previous URL and switch Sec-Fetch-Site
    // to "cross-site" or "same-origin".
    // (Left empty for now — direct-entry navigation.)

    h
}

// ─────────────────────────────────────────────────────────────────────
// Robots.txt check
// ─────────────────────────────────────────────────────────────────────

async fn check_robots(
    client: &reqwest::Client,
    parsed_url: &url::Url,
    url_to_fetch: &str,
) -> Result<()> {
    let origin = parsed_url.origin().ascii_serialization();
    if origin.is_empty() || origin == "null" {
        return Ok(());
    }
    let robots_url = format!("{}/robots.txt", origin);
    let Ok(robots_resp) = client
        .get(&robots_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    else {
        return Ok(());
    };
    if !robots_resp.status().is_success() {
        return Ok(());
    }
    let Ok(robots_body) = robots_resp.text().await else {
        return Ok(());
    };
    let mut matcher = robotstxt::DefaultMatcher::default();
    // Use "*" as the user-agent since we rotate UAs — this is the most
    // conservative (and polite) choice.
    if !matcher.one_agent_allowed_by_robots(&robots_body, "*", parsed_url.as_str()) {
        anyhow::bail!(
            "Access denied by robots.txt for URL: {url_to_fetch}\n\n\
             The site explicitly forbids automated access to this path."
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Meta tag extraction — title, description, Open Graph, Twitter cards
// ─────────────────────────────────────────────────────────────────────

fn extract_meta(html: &str) -> String {
    let mut title = String::new();
    let mut description = String::new();
    let mut og_title = String::new();
    let mut og_description = String::new();
    let mut og_image = String::new();
    let mut og_site = String::new();
    let mut twitter_card = String::new();

    // Quick scan with simple string matching (no full HTML parser needed)
    let lower = html.to_lowercase();
    let _meta_start = lower.find("<meta ");
    let title_start = lower.find("<title>");

    // <title> tag
    if let Some(ts) = title_start {
        let content_start = ts + 7; // len("<title>")
        if let Some(te) = lower[content_start..].find("</title>") {
            title = html[content_start..content_start + te].trim().to_string();
        }
    }

    // <meta ...> tags — scan up to first 16 KB for meta
    let scan_limit = html.len().min(16_384);
    let scan = &html[..scan_limit];
    let scan_lower = &lower[..scan_limit];

    let mut pos = 0;
    while let Some(m_idx) = scan_lower[pos..].find("<meta ") {
        let abs_idx = pos + m_idx;
        let tag_end = scan_lower[abs_idx..]
            .find('>')
            .map(|e| abs_idx + e + 1);
        pos = abs_idx + 6; // move past "<meta "

        let Some(end) = tag_end else { continue };
        let tag_slice = &scan[abs_idx..end];

        let name = extract_attr(tag_slice, "name");
        let property = extract_attr(tag_slice, "property");
        let content = extract_attr(tag_slice, "content");

        if content.is_empty() {
            continue;
        }

        // Standard meta
        match name.to_lowercase().as_str() {
            "description" if description.is_empty() => {
                description = content.clone();
            }
            "twitter:card" if twitter_card.is_empty() => {
                twitter_card = content.clone();
            }
            _ => {}
        }

        // Open Graph (property=)
        match property.to_lowercase().as_str() {
            "og:title" if og_title.is_empty() => og_title = content.clone(),
            "og:description" if og_description.is_empty() => og_description = content.clone(),
            "og:image" if og_image.is_empty() => og_image = content.clone(),
            "og:site_name" if og_site.is_empty() => og_site = content.clone(),
            _ => {}
        }
    }

    let mut output = String::new();
    let display_title = if !og_title.is_empty() {
        &og_title
    } else if !title.is_empty() {
        &title
    } else {
        ""
    };
    let display_desc = if !og_description.is_empty() {
        &og_description
    } else {
        &description
    };

    if !display_title.is_empty() {
        output.push_str(&format!("**Title:** {display_title}\n"));
    }
    if !display_desc.is_empty() {
        output.push_str(&format!("**Description:** {display_desc}\n"));
    }
    if !og_site.is_empty() {
        output.push_str(&format!("**Source:** {og_site}\n"));
    }

    output
}

/// Extract an HTML attribute value from a tag fragment.
/// Handles quoted (single/double) and unquoted values.
fn extract_attr(tag: &str, attr_name: &str) -> String {
    let lower = tag.to_lowercase();
    let pattern = format!("{attr_name}=");
    let Some(attr_start) = lower.find(&pattern) else {
        return String::new();
    };

    let after_eq = attr_start + pattern.len();
    let rest = &tag[after_eq..];

    // Quoted value
    if let Some(first_char) = rest.chars().next() {
        if first_char == '"' || first_char == '\'' {
            let quote = first_char;
            let rest_after_quote = &rest[1..];
            if let Some(end) = rest_after_quote.find(quote) {
                return unescape_html(&rest_after_quote[..end]);
            }
        }
    }

    // Unquoted value — take until whitespace or >
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    unescape_html(&rest[..end])
}

/// Basic HTML entity unescaping for meta values.
fn unescape_html(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
}

// ─────────────────────────────────────────────────────────────────────
// Content extraction pipeline
//   raw HTML → Readability (article body) → htmd (Markdown)
// ─────────────────────────────────────────────────────────────────────

fn extract_readable(html: &str, url: &str) -> String {
    let article_html = match run_readability(html, url) {
        Some(h) if !h.trim().is_empty() => h,
        _ => html.to_string(),
    };

    match htmd::convert(&article_html) {
        Ok(md) => {
            let cleaned = md.trim().to_string();
            if cleaned.len() > 200 {
                cleaned
            } else {
                format!(
                    "[Extracted content is very short ({len} chars). \
                     This page may require JavaScript to render its content. \
                     Consider finding the information through an alternative source.]\n\n{cleaned}",
                    len = cleaned.len()
                )
            }
        }
        Err(e) => {
            format!(
                "[Warning: Markdown conversion failed ({e}). Falling back to plain text.]\n\n{}",
                strip_html_tags(&article_html)
            )
        }
    }
}

fn run_readability(html: &str, url: &str) -> Option<String> {
    let url_parsed = url::Url::parse(url).ok()?;
    let mut cursor = std::io::Cursor::new(html.as_bytes());
    let product = readability::extractor::extract(&mut cursor, &url_parsed).ok()?;

    let title = product.title.trim();
    let content = product.content.trim();

    if content.is_empty() {
        return None;
    }

    let mut output = String::new();
    if !title.is_empty() {
        output.push_str(&format!("<h1>{}</h1>\n", title));
    }
    output.push_str(content);
    Some(output)
}

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

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────
// Error formatting — includes rate-limit info when applicable
// ─────────────────────────────────────────────────────────────────────

fn format_http_error(
    url: &str,
    rate_delay: Option<Duration>,
    error: &reqwest::Error,
) -> String {
    let mut base = String::new();
    if let Some(d) = rate_delay {
        base.push_str(&format!(
            "[Rate limit: waited {:.1}s before request]\n",
            d.as_secs_f64()
        ));
    }
    base.push_str(&format!("Failed to fetch {url}."));

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
    _original_url: &str,
    fetch_url: &str,
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

    let redirect_note = if fetch_url != final_url {
        format!("\nNote: The request was redirected to {final_url}.")
    } else {
        String::new()
    };

    format!(
        "Failed to fetch {fetch_url} (status {status_code} {reason}).\n\
         Explanation: {explanation}\n\
         Suggestion: {suggestion}{redirect_note}"
    )
}
