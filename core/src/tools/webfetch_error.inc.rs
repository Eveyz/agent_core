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
