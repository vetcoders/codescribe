//! Secret-path escalation for the tool permission gateway (review S-P0-01).
//!
//! A read-level `Allow` — the migration default for read-only tools, or a
//! blanket per-tool/per-server operator rule — must never silently cover
//! credential-bearing files. With `~/.codescribe` inside the workspace roots,
//! a plain `read_file` could lift `.env` or the `env` blocks of `mcp.json`
//! without the operator ever seeing an approval card. The gateway therefore
//! re-classifies any Allow whose input references a secret-bearing path down
//! to Ask; the card shows the exact path, and no durable rule can lift the
//! escalation back to silent-allow.
//!
//! Every string in the tool input is scanned, not just schema-known path
//! fields: MCP tools carry arbitrary schemas, and argv-style arrays hide paths
//! inside plain string lists. The cost of a false positive is one approval
//! card, never a deny — fail-closed is the right side of that trade.

use serde_json::Value;

/// First secret-bearing path referenced anywhere in `input`, if any.
pub fn find_secret_path(input: &Value) -> Option<String> {
    match input {
        Value::String(text) => classify(text).then(|| text.clone()),
        Value::Array(items) => items.iter().find_map(find_secret_path),
        Value::Object(map) => map.values().find_map(find_secret_path),
        _ => None,
    }
}

/// Whether a single string references a secret-bearing filesystem location.
///
/// Only path-shaped strings participate (contain `/` or start with `~`) so bare
/// words like a search query for "env" never escalate.
fn classify(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || !(trimmed.contains('/') || trimmed.starts_with('~')) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    // Credential directories anywhere in the path — including the bare
    // directory itself ("~/.ssh", "/Users/op/.aws", with or without a
    // trailing slash): listing the directory already leaks the key inventory.
    const SECRET_DIRS: &[&str] = &["/.ssh", "/.aws", "/.gnupg", "/.kube"];
    let no_trailing = lower.trim_end_matches('/');
    if SECRET_DIRS
        .iter()
        .any(|dir| no_trailing.ends_with(dir) || lower.contains(&[dir, "/"].concat()))
    {
        return true;
    }
    let basename = lower.rsplit('/').next().unwrap_or(&lower);
    is_secret_basename(basename)
}

/// Whether a filename (no directory part) names credential-bearing content.
///
/// Covers dotted env files, the MCP config and its backups, the grant store,
/// conventional credential dotfiles, key material by extension, and the usual
/// SSH key stems.
fn is_secret_basename(name: &str) -> bool {
    // Dotted env files: .env, .env.local, .env.debug.example, …
    if name.starts_with(".env") {
        return true;
    }
    // MCP config carries per-server `env` secret blocks; backups keep them.
    if name == "mcp.json" || name.starts_with("mcp.json.") {
        return true;
    }
    // Grant store is integrity-sensitive: editing it IS privilege escalation.
    if name == "tool_grants.json" {
        return true;
    }
    if name == ".netrc" || name == ".npmrc" || name == ".pgpass" {
        return true;
    }
    // Key material by extension / conventional stem.
    const KEY_SUFFIXES: &[&str] = &[".pem", ".p12", ".pfx", ".key", ".keychain-db", ".keystore"];
    if KEY_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) {
        return true;
    }
    const KEY_STEMS: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];
    KEY_STEMS.iter().any(|stem| {
        name == *stem || (name.starts_with(stem) && name.as_bytes().get(stem.len()) == Some(&b'.'))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn env_and_mcp_config_paths_are_secret() {
        for path in [
            "/Users/op/.codescribe/.env",
            "~/.codescribe/.env.debug",
            "/Users/op/.codescribe/mcp.json",
            "/Users/op/.codescribe/mcp.json.backup-20260720",
            "/Users/op/.codescribe/tool_grants.json",
            "~/.ssh/id_rsa",
            "~/.ssh/known_hosts",
            "~/.ssh",
            "~/.ssh/",
            "/Users/op/.aws",
            "/Users/op/.gnupg/",
            "/Users/op/.aws/credentials",
            "/certs/server.pem",
            "/Users/op/Library/Keychains/login.keychain-db",
        ] {
            assert!(
                find_secret_path(&json!({ "path": path })).is_some(),
                "expected secret: {path}"
            );
        }
    }

    #[test]
    fn ordinary_workspace_paths_and_bare_words_pass() {
        for path in [
            "/Users/op/project/src/main.rs",
            "/Users/op/.codescribe/notes/todo.md",
            "~/.codescribe/transcriptions/2026-08-04/raw.txt",
            "/Users/op/project/environment.md",
            "keychain_env_docs.md",
            "/Users/op/project/notes.ssh.md",
            "/Users/op/kessh/readme.md",
        ] {
            assert!(
                find_secret_path(&json!({ "path": path })).is_none(),
                "expected clean: {path}"
            );
        }
        // Bare words never escalate, even secret-adjacent ones.
        assert!(find_secret_path(&json!({ "query": "env" })).is_none());
        assert!(find_secret_path(&json!({ "query": "mcp.json" })).is_none());
    }

    #[test]
    fn nested_arrays_and_objects_are_scanned() {
        let input = json!({
            "args": ["cat", "~/.codescribe/.env"],
            "options": { "cwd": "/tmp" }
        });
        assert_eq!(
            find_secret_path(&input).as_deref(),
            Some("~/.codescribe/.env")
        );
    }
}
