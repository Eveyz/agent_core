use std::time::Duration;
use std::io::Cursor;
use anyhow::{Result, bail};
use reqwest::header;
use url::Url;

use super::UaProfile;

pub(crate) fn build_browser_headers(profile: &UaProfile, _url: &str) -> header::HeaderMap {
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

pub(crate) async fn check_robots(
    client: &reqwest::Client,
    parsed_url: &Url,
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
        bail!(
            "Access denied by robots.txt for URL: {url_to_fetch}\n\n\
             The site explicitly forbids automated access to this path."
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Meta tag extraction — title, description, Open Graph, Twitter cards
// ─────────────────────────────────────────────────────────────────────

pub(crate) fn extract_meta(html: &str) -> String {
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

pub(crate) fn extract_readable(html: &str, url: &str) -> String {
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
    let url_parsed = Url::parse(url).ok()?;
    let mut cursor = Cursor::new(html.as_bytes());
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
