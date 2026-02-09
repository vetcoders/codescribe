//! URL/internet connector — fetch web page content as text attachment.
//!
//! Strips HTML to plain text with a simple state-machine parser.
//! No heavy dependencies (no headless browser, no full HTML parser).

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const MAX_RESPONSE_BYTES: usize = 1024 * 1024; // 1MB
const TIMEOUT_SECS: u64 = 15;
const MAX_REDIRECTS: usize = 3;

// ═══════════════════════════════════════════════════════════
// Public API
// ═══════════════════════════════════════════════════════════

/// Fetch a URL and return its content as plain text.
///
/// Returns `(text_content, page_title)`.
///
/// HTML is stripped to plain text. Non-HTML responses are returned as-is
/// (if they're valid UTF-8).
pub async fn fetch_url_as_text(url: &str) -> Result<(String, String)> {
    info!("Fetching URL: {}", url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .user_agent("CodeScribe/1.0 (speech-to-text assistant)")
        .build()
        .context("Failed to build HTTP client")?;

    let resp = client.get(url).send().await.context("URL fetch failed")?;

    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP error {status} fetching {url}");
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let content_length = resp.content_length().unwrap_or(0) as usize;
    if content_length > MAX_RESPONSE_BYTES {
        bail!(
            "Response too large ({} bytes, max {})",
            content_length,
            MAX_RESPONSE_BYTES
        );
    }

    let bytes = resp.bytes().await.context("Failed to read response body")?;

    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!(
            "Response too large ({} bytes, max {})",
            bytes.len(),
            MAX_RESPONSE_BYTES
        );
    }

    let body = String::from_utf8_lossy(&bytes).to_string();

    let is_html = content_type.contains("text/html")
        || body.trim_start().starts_with("<!DOCTYPE")
        || body.trim_start().starts_with("<!doctype")
        || body.trim_start().starts_with("<html");

    if is_html {
        let title = extract_title(&body);
        let text = strip_html(&body);
        debug!(
            "Fetched URL as HTML: title={:?}, {} chars text",
            title,
            text.len()
        );
        Ok((text, title))
    } else {
        let title = url_to_title(url);
        debug!("Fetched URL as text: {} chars", body.len());
        Ok((body, title))
    }
}

/// Check if a string looks like a URL.
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("http://") || s.starts_with("https://")
}

// ═══════════════════════════════════════════════════════════
// HTML stripping
// ═══════════════════════════════════════════════════════════

/// Extract `<title>` content from HTML.
fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    if let Some(start) = lower.find("<title") {
        let after_tag = &html[start..];
        if let Some(close_bracket) = after_tag.find('>') {
            let after_open = &after_tag[close_bracket + 1..];
            if let Some(end) = after_open.to_ascii_lowercase().find("</title") {
                let title = &after_open[..end];
                let title = title.trim();
                if !title.is_empty() {
                    return decode_html_entities(title);
                }
            }
        }
    }
    String::new()
}

/// Strip HTML tags and return plain text.
///
/// Simple state-machine approach: skip everything inside `<...>`,
/// collapse whitespace, decode basic HTML entities.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_space = true;

    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0; // byte offset — all slicing is byte-safe

    while i < len {
        let b = bytes[i];

        if in_tag {
            if b == b'>' {
                in_tag = false;
            }
            i += 1;
            continue;
        }

        if b == b'<' {
            // Check for script/style start/end (byte-safe: '<' is ASCII)
            let remaining = &lower[i..];

            if remaining.starts_with("<script") {
                in_script = true;
            } else if remaining.starts_with("</script") {
                in_script = false;
            } else if remaining.starts_with("<style") {
                in_style = true;
            } else if remaining.starts_with("</style") {
                in_style = false;
            }

            // Block-level elements → newline
            if remaining.starts_with("<br")
                || remaining.starts_with("<p")
                || remaining.starts_with("</p")
                || remaining.starts_with("<div")
                || remaining.starts_with("</div")
                || remaining.starts_with("<h1")
                || remaining.starts_with("<h2")
                || remaining.starts_with("<h3")
                || remaining.starts_with("<h4")
                || remaining.starts_with("<h5")
                || remaining.starts_with("<h6")
                || remaining.starts_with("</h1")
                || remaining.starts_with("</h2")
                || remaining.starts_with("</h3")
                || remaining.starts_with("</h4")
                || remaining.starts_with("</h5")
                || remaining.starts_with("</h6")
                || remaining.starts_with("<li")
                || remaining.starts_with("<tr")
            {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
                last_was_space = true;
            }

            in_tag = true;
            i += 1;
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        // Handle HTML entities (byte-safe: '&' is ASCII)
        if b == b'&'
            && let Some((decoded, advance)) = try_decode_entity(&html[i..])
        {
            for c in decoded.chars() {
                if c.is_whitespace() {
                    if !last_was_space {
                        out.push(' ');
                        last_was_space = true;
                    }
                } else {
                    out.push(c);
                    last_was_space = false;
                }
            }
            i += advance; // advance is already byte count from try_decode_entity
            continue;
        }

        // Regular character — decode UTF-8 from byte position
        let c = html[i..].chars().next().unwrap();
        let char_len = c.len_utf8();
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }

        i += char_len;
    }

    // Collapse multiple newlines
    let mut result = String::with_capacity(out.len());
    let mut newline_count = 0u32;
    for c in out.chars() {
        if c == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                result.push('\n');
            }
        } else {
            newline_count = 0;
            result.push(c);
        }
    }

    result.trim().to_string()
}

/// Try to decode an HTML entity starting at `&`.
/// Returns `(decoded_string, bytes_consumed)` or None.
fn try_decode_entity(s: &str) -> Option<(String, usize)> {
    let end = s.find(';')?;
    if end > 10 {
        return None; // entities are short
    }
    let entity = &s[1..end];
    let decoded = match entity {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        "ndash" => "–",
        "mdash" => "—",
        "lsquo" => "'",
        "rsquo" => "'",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "hellip" => "…",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        _ => {
            // Numeric entity: &#123; or &#x1F4A9;
            if let Some(hex) = entity.strip_prefix("#x") {
                let code = u32::from_str_radix(hex, 16).ok()?;
                let c = char::from_u32(code)?;
                return Some((c.to_string(), end + 1));
            } else if let Some(dec) = entity.strip_prefix('#') {
                let code: u32 = dec.parse().ok()?;
                let c = char::from_u32(code)?;
                return Some((c.to_string(), end + 1));
            }
            return None;
        }
    };
    Some((decoded.to_string(), end + 1))
}

/// Decode common HTML entities in a string.
fn decode_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'&'
            && let Some((decoded, advance)) = try_decode_entity(&s[i..])
        {
            out.push_str(&decoded);
            i += advance;
            continue;
        }
        out.push(s[i..].chars().next().unwrap());
        i += s[i..].chars().next().unwrap().len_utf8();
    }

    out
}

/// Derive a display title from a URL.
fn url_to_title(url: &str) -> String {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_html_basic() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn test_strip_html_script_style() {
        let html = "<p>Before</p><script>alert('x')</script><style>.x{}</style><p>After</p>";
        let text = strip_html(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("alert"));
        assert!(!text.contains(".x"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D</p>";
        let text = strip_html(html);
        assert!(text.contains("A & B < C > D"));
    }

    #[test]
    fn test_strip_html_whitespace_collapse() {
        let html = "<p>  Hello   World  </p>";
        let text = strip_html(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_extract_title() {
        let html = "<html><head><title>My Page</title></head><body></body></html>";
        assert_eq!(extract_title(html), "My Page");
    }

    #[test]
    fn test_extract_title_with_entities() {
        let html = "<title>A &amp; B</title>";
        assert_eq!(extract_title(html), "A & B");
    }

    #[test]
    fn test_extract_title_missing() {
        let html = "<html><body>No title here</body></html>";
        assert_eq!(extract_title(html), "");
    }

    #[test]
    fn test_looks_like_url() {
        assert!(looks_like_url("https://example.com"));
        assert!(looks_like_url("http://example.com/page"));
        assert!(looks_like_url("  https://example.com  "));
        assert!(!looks_like_url("not a url"));
        assert!(!looks_like_url("ftp://example.com"));
    }

    #[test]
    fn test_url_to_title() {
        assert_eq!(url_to_title("https://example.com/page"), "example.com");
        assert_eq!(url_to_title("http://docs.rs/crate"), "docs.rs");
    }

    #[test]
    fn test_numeric_entity() {
        let html = "<p>&#65;&#x42;</p>";
        let text = strip_html(html);
        assert!(text.contains("AB"));
    }

    #[test]
    fn test_strip_html_polish_diacritics() {
        let html = "<p>Zażółć gęślą jaźń</p>";
        let text = strip_html(html);
        assert_eq!(text, "Zażółć gęślą jaźń");
    }

    #[test]
    fn test_strip_html_mixed_multibyte_and_entities() {
        let html = "<div>Résumé &amp; café</div><p>日本語テスト</p>";
        let text = strip_html(html);
        assert!(text.contains("Résumé & café"));
        assert!(text.contains("日本語テスト"));
    }

    #[test]
    fn test_extract_title_multibyte() {
        let html = "<html><head><title>Strona główna — informacje</title></head></html>";
        assert_eq!(extract_title(html), "Strona główna — informacje");
    }
}
