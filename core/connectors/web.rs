//! URL/internet connector — fetch web page content as text attachment.
//!
//! Strips HTML to plain text with a simple state-machine parser.
//! No heavy dependencies (no headless browser, no full HTML parser).

use anyhow::{Context, Result, bail};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use tracing::{debug, info};

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const MAX_RESPONSE_BYTES: usize = 1024 * 1024; // 1MB
const MAX_REDIRECTS: usize = 3;

// ═══════════════════════════════════════════════════════════
// SSRF protection
// ═══════════════════════════════════════════════════════════

/// Check if a URL points to a private/internal host.
fn is_private_host(url: &reqwest::Url) -> bool {
    let Some(host_raw) = url.host_str() else {
        return true; // no host → block
    };
    let host = host_raw.trim_matches(['[', ']']);

    // Exact matches
    if matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        return true;
    }

    // Suffix-based blocks
    if host.ends_with(".local") || host.ends_with(".internal") {
        return true;
    }

    // Parse as IPv4 and check RFC 1918 / link-local ranges.
    if let Some(is_private) = check_ipv4_private(host) {
        return is_private;
    }

    // Parse as IPv6 and check loopback/link-local/ULA ranges.
    if let Some(is_private) = check_ipv6_private(host) {
        return is_private;
    }

    false
}

/// Check if a resolved IP is private/internal.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    match octets[0] {
        10 => true,                            // 10.0.0.0/8
        172 => (16..=31).contains(&octets[1]), // 172.16.0.0/12
        192 if octets[1] == 168 => true,       // 192.168.0.0/16
        169 if octets[1] == 254 => true,       // 169.254.0.0/16 link-local
        127 => true,                           // 127.0.0.0/8 loopback
        0 => true,                             // 0.0.0.0/8
        _ => false,
    }
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || ip.is_unicast_link_local() || ip.is_unique_local()
}

/// Resolve a non-literal host and block if any resolved IP is private/internal.
fn resolves_to_private_host(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim_matches(['[', ']']);

    // Literal IPs are handled by `is_private_host`; DNS rebinding protection
    // targets domain names that resolve to private/internal addresses.
    if host.parse::<IpAddr>().is_ok() {
        return false;
    }

    let port = url
        .port_or_known_default()
        .unwrap_or(if url.scheme() == "http" { 80 } else { 443 });

    let addrs = (host, port).to_socket_addrs();
    let Ok(iter) = addrs else {
        // Fail closed on DNS errors.
        return true;
    };

    let mut resolved_any = false;
    for addr in iter {
        resolved_any = true;
        if is_private_ip(addr.ip()) {
            return true;
        }
    }

    // Fail closed if resolver returns no addresses.
    !resolved_any
}

/// Check IPv4 address against private/reserved ranges.
/// Returns `Some(true)` if private, `Some(false)` if public IPv4,
/// `None` if not an IPv4 address.
fn check_ipv4_private(host: &str) -> Option<bool> {
    let ip = host.parse::<Ipv4Addr>().ok()?;
    Some(is_private_ipv4(ip))
}

/// Check IPv6 address against private/reserved ranges.
/// Returns `Some(true)` if private, `Some(false)` if public IPv6,
/// `None` if not an IPv6 address.
fn check_ipv6_private(host: &str) -> Option<bool> {
    let ip = host.parse::<std::net::Ipv6Addr>().ok()?;
    Some(is_private_ipv6(ip))
}

/// Build an SSRF-safe HTTP client with redirect policy that revalidates
/// each hop against `is_private_host`.
fn ssrf_safe_client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= MAX_REDIRECTS {
                        attempt.error(anyhow::anyhow!("Too many redirects"))
                    } else if is_private_host(attempt.url())
                        || resolves_to_private_host(attempt.url())
                    {
                        attempt.error(anyhow::anyhow!(
                            "Redirect to private/internal address blocked"
                        ))
                    } else {
                        attempt.follow()
                    }
                }))
                .user_agent("CodeScribe/1.0 (speech-to-text assistant)")
                .build()
                .expect("Failed to build SSRF-safe HTTP client")
        })
        .clone()
}

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
    // SSRF protection: only allow http/https schemes.
    let url_trimmed = url.trim();
    let parsed = reqwest::Url::parse(url_trimmed).context("Invalid URL")?;

    let scheme = parsed.scheme();
    if scheme != "https" && scheme != "http" {
        bail!("Unsupported URL scheme (only http/https allowed): {url_trimmed}");
    }

    // Block private/internal IP ranges and metadata services.
    if is_private_host(&parsed) || resolves_to_private_host(&parsed) {
        bail!("URL blocked: private/internal addresses not allowed");
    }

    info!("Fetching URL: {}", url);

    // Use a client with custom redirect policy that revalidates each hop.
    let client = ssrf_safe_client();

    let mut resp = client
        .get(parsed)
        .send()
        .await
        .context("URL fetch failed")?;

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

    // Early reject if Content-Length header exceeds limit.
    let content_length = resp.content_length().unwrap_or(0) as usize;
    if content_length > MAX_RESPONSE_BYTES {
        bail!(
            "Response too large ({} bytes, max {})",
            content_length,
            MAX_RESPONSE_BYTES
        );
    }

    // Stream chunks with running size check — prevents decompression bombs
    // and chunked-transfer attacks that omit Content-Length.
    let mut buf = Vec::with_capacity(content_length.min(MAX_RESPONSE_BYTES));
    while let Some(chunk) = resp
        .chunk()
        .await
        .context("Failed to read response chunk")?
    {
        if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
            bail!(
                "Response too large (>{} bytes, max {})",
                buf.len() + chunk.len(),
                MAX_RESPONSE_BYTES
            );
        }
        buf.extend_from_slice(&chunk);
    }

    let body = String::from_utf8_lossy(&buf).to_string();

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

    // ── SSRF protection tests ──

    #[tokio::test]
    async fn test_ssrf_private_ip_blocked() {
        let result = fetch_url_as_text("https://127.0.0.1/secret").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[tokio::test]
    async fn test_ssrf_localhost_blocked() {
        let result = fetch_url_as_text("https://localhost/admin").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[tokio::test]
    async fn test_ssrf_internal_blocked() {
        let result = fetch_url_as_text("https://metadata.internal/latest").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[tokio::test]
    async fn test_ssrf_rfc1918_blocked() {
        let result = fetch_url_as_text("https://192.168.1.1/config").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked"));
    }

    #[tokio::test]
    async fn test_ftp_scheme_rejected() {
        let result = fetch_url_as_text("ftp://evil.com/payload").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unsupported URL scheme")
        );
    }

    #[test]
    fn test_ssrf_172_31_blocked() {
        // 172.31.x.x is private (RFC 1918: 172.16.0.0/12)
        let url = reqwest::Url::parse("https://172.31.255.1/secret").unwrap();
        assert!(is_private_host(&url));
    }

    #[test]
    fn test_ssrf_172_32_not_blocked() {
        // 172.32.x.x is PUBLIC — outside RFC 1918 range
        let url = reqwest::Url::parse("https://172.32.0.1/page").unwrap();
        assert!(!is_private_host(&url));
    }

    #[test]
    fn test_ssrf_10_blocked() {
        let url = reqwest::Url::parse("https://10.0.0.1/admin").unwrap();
        assert!(is_private_host(&url));
    }

    #[test]
    fn test_ssrf_public_ip_allowed() {
        let url = reqwest::Url::parse("https://8.8.8.8/dns").unwrap();
        assert!(!is_private_host(&url));
    }

    #[test]
    fn test_ssrf_link_local_blocked() {
        let url = reqwest::Url::parse("https://169.254.1.1/meta").unwrap();
        assert!(is_private_host(&url));
    }

    #[test]
    fn test_ssrf_ipv6_loopback_blocked() {
        let url = reqwest::Url::parse("https://[::1]/admin").unwrap();
        assert!(is_private_host(&url));
    }

    #[test]
    fn test_ssrf_ipv6_unique_local_blocked() {
        let url = reqwest::Url::parse("https://[fd00::1]/admin").unwrap();
        assert!(is_private_host(&url));
    }

    #[test]
    fn test_ssrf_ipv6_public_allowed() {
        let url = reqwest::Url::parse("https://[2606:4700:4700::1111]/dns").unwrap();
        assert!(!is_private_host(&url));
    }

    // ── HTML edge cases ──

    #[test]
    fn test_strip_html_nested_script() {
        let html = "<p>A</p><script>var x = '<p>not real</p>';</script><p>B</p>";
        let text = strip_html(html);
        assert!(text.contains('A'));
        assert!(text.contains('B'));
        assert!(!text.contains("not real"));
    }

    #[test]
    fn test_strip_html_empty() {
        assert_eq!(strip_html(""), "");
    }

    #[test]
    fn test_strip_html_no_tags() {
        assert_eq!(strip_html("plain text"), "plain text");
    }
}
