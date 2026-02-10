//! GitHub connector — fetch file content from GitHub repositories.
//!
//! Supports two input formats:
//! - URL: `https://github.com/owner/repo/blob/branch/path/to/file`
//! - Spec: `owner/repo@branch:path/to/file`
//!
//! Uses the GitHub Contents API with optional token authentication.

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

// ═══════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════

/// Parsed GitHub file reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRef {
    pub owner: String,
    pub repo: String,
    pub git_ref: String,
    pub path: String,
}

impl std::fmt::Display for GitHubRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}@{}:{}",
            self.owner, self.repo, self.git_ref, self.path
        )
    }
}

// ═══════════════════════════════════════════════════════════
// Parser
// ═══════════════════════════════════════════════════════════

/// Parse a GitHub reference from either URL or spec format.
///
/// Accepted formats:
/// - `https://github.com/owner/repo/blob/ref/path/to/file`
/// - `https://github.com/owner/repo/raw/ref/path/to/file`
/// - `owner/repo@ref:path/to/file`
/// - `owner/repo:path/to/file` (defaults ref to `main`)
pub fn parse_github_ref(input: &str) -> Option<GitHubRef> {
    let input = input.trim();

    // Try URL format first
    if let Some(gh) = parse_github_url(input) {
        return Some(gh);
    }

    // Try spec format: owner/repo@ref:path or owner/repo:path
    parse_github_spec(input)
}

fn parse_github_url(url: &str) -> Option<GitHubRef> {
    // Match: https://github.com/owner/repo/blob/ref/path...
    // or:    https://github.com/owner/repo/raw/ref/path...
    let url = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;

    let parts: Vec<&str> = url.splitn(5, '/').collect();
    // parts: [owner, repo, "blob"|"raw", ref, path...]
    if parts.len() < 5 {
        return None;
    }

    let kind = parts[2];
    if kind != "blob" && kind != "raw" && kind != "tree" {
        return None;
    }

    // For "tree" links (directory), we still parse but the fetch might fail
    // gracefully downstream if it's a directory.

    Some(GitHubRef {
        owner: parts[0].to_string(),
        repo: parts[1].to_string(),
        git_ref: parts[3].to_string(),
        path: parts[4].to_string(),
    })
}

fn parse_github_spec(spec: &str) -> Option<GitHubRef> {
    // Format: owner/repo@ref:path or owner/repo:path
    let slash_pos = spec.find('/')?;
    let owner = &spec[..slash_pos];
    let rest = &spec[slash_pos + 1..];

    if owner.is_empty() {
        return None;
    }

    // Find @ or : to split repo from ref/path
    let (repo, git_ref, path) = if let Some(at_pos) = rest.find('@') {
        let repo = &rest[..at_pos];
        let after_at = &rest[at_pos + 1..];
        let colon_pos = after_at.find(':')?;
        let git_ref = &after_at[..colon_pos];
        let path = &after_at[colon_pos + 1..];
        (repo, git_ref, path)
    } else if let Some(colon_pos) = rest.find(':') {
        let repo = &rest[..colon_pos];
        let path = &rest[colon_pos + 1..];
        (repo, "main", path)
    } else {
        return None;
    };

    if repo.is_empty() || path.is_empty() {
        return None;
    }

    Some(GitHubRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        git_ref: git_ref.to_string(),
        path: path.to_string(),
    })
}

// ═══════════════════════════════════════════════════════════
// Fetch
// ═══════════════════════════════════════════════════════════

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024; // 10MB
const TIMEOUT_SECS: u64 = 30;

/// Fetch raw file content from GitHub.
///
/// Returns `(content_bytes, filename)`.
pub async fn fetch_github_blob(gh: &GitHubRef, token: Option<&str>) -> Result<(Vec<u8>, String)> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
        gh.owner, gh.repo, gh.path, gh.git_ref
    );

    info!("Fetching GitHub blob: {}", gh);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .user_agent("CodeScribe/1.0 (github.com/VetCoders/CodeScribe)")
        .build()
        .context("Failed to build HTTP client")?;

    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github.raw+json");

    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req.send().await.context("GitHub API request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("GitHub API error {status}: {body}");
    }

    let content_length = resp.content_length().unwrap_or(0) as usize;
    if content_length > MAX_RESPONSE_BYTES {
        bail!(
            "GitHub blob too large ({} bytes, max {})",
            content_length,
            MAX_RESPONSE_BYTES
        );
    }

    let bytes = resp
        .bytes()
        .await
        .context("Failed to read GitHub response")?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!(
            "GitHub blob too large ({} bytes, max {})",
            bytes.len(),
            MAX_RESPONSE_BYTES
        );
    }

    let filename = gh.path.rsplit('/').next().unwrap_or(&gh.path).to_string();

    debug!("Fetched GitHub blob: {} ({} bytes)", filename, bytes.len());
    Ok((bytes.to_vec(), filename))
}

/// Load GitHub token from Keychain (if available).
pub fn load_github_token() -> Option<String> {
    crate::config::keychain::load_key("GITHUB_TOKEN")
}

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_url_blob() {
        let gh = parse_github_ref("https://github.com/VetCoders/CodeScribe/blob/main/src/lib.rs");
        assert_eq!(
            gh,
            Some(GitHubRef {
                owner: "VetCoders".into(),
                repo: "CodeScribe".into(),
                git_ref: "main".into(),
                path: "src/lib.rs".into(),
            })
        );
    }

    #[test]
    fn test_parse_github_url_raw() {
        let gh = parse_github_ref(
            "https://github.com/rust-lang/rust/raw/master/compiler/rustc/src/main.rs",
        );
        assert_eq!(
            gh,
            Some(GitHubRef {
                owner: "rust-lang".into(),
                repo: "rust".into(),
                git_ref: "master".into(),
                path: "compiler/rustc/src/main.rs".into(),
            })
        );
    }

    #[test]
    fn test_parse_github_spec_full() {
        let gh = parse_github_ref("VetCoders/CodeScribe@fix/multiple-fixes:core/lib.rs");
        assert_eq!(
            gh,
            Some(GitHubRef {
                owner: "VetCoders".into(),
                repo: "CodeScribe".into(),
                git_ref: "fix/multiple-fixes".into(),
                path: "core/lib.rs".into(),
            })
        );
    }

    #[test]
    fn test_parse_github_spec_default_ref() {
        let gh = parse_github_ref("VetCoders/CodeScribe:core/lib.rs");
        assert_eq!(
            gh,
            Some(GitHubRef {
                owner: "VetCoders".into(),
                repo: "CodeScribe".into(),
                git_ref: "main".into(),
                path: "core/lib.rs".into(),
            })
        );
    }

    #[test]
    fn test_parse_github_invalid() {
        assert_eq!(parse_github_ref("not a github ref"), None);
        assert_eq!(parse_github_ref(""), None);
        assert_eq!(parse_github_ref("just/repo"), None);
        assert_eq!(parse_github_ref("https://gitlab.com/a/b/blob/main/x"), None);
    }

    #[test]
    fn test_parse_github_url_deep_path() {
        let gh = parse_github_ref("https://github.com/org/repo/blob/v2.0/src/deep/nested/file.rs");
        let gh = gh.unwrap();
        assert_eq!(gh.owner, "org");
        assert_eq!(gh.repo, "repo");
        assert_eq!(gh.git_ref, "v2.0");
        assert_eq!(gh.path, "src/deep/nested/file.rs");
    }

    #[test]
    fn test_github_ref_display() {
        let gh = GitHubRef {
            owner: "owner".into(),
            repo: "repo".into(),
            git_ref: "main".into(),
            path: "src/lib.rs".into(),
        };
        assert_eq!(gh.to_string(), "owner/repo@main:src/lib.rs");
    }
}
