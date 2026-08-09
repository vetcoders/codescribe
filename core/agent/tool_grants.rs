//! Persisted "always allow" grants for external (MCP) tools.
//!
//! One JSON file (`~/.codescribe/tool_grants.json`) keyed by
//! `"<server>:<upstream_tool>"`. A grant means the operator pressed
//! "Zawsze zezwalaj" on the approval card, so future calls of that exact
//! server+tool pair skip the approval gate. Grants never apply to
//! `ToolRisk::Destructive` — the registry denies those before consulting
//! grants.
//!
//! Same durability rules as the MCP config store: writes are atomic
//! (temp file + fsync + rename) and a present-but-invalid file makes
//! mutations error out instead of silently overwriting operator data.
//! Reads are forgiving: a missing or unparsable file yields an empty
//! grant set — the worst outcome is asking the operator again.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// On-disk grants schema version; bump when GrantsFile shape changes.
const VERSION: u32 = 1;

/// Canonical grants file location: `$HOME/.codescribe/tool_grants.json`.
/// Errors when `HOME` is unset rather than guessing a fallback directory.
pub fn default_tool_grants_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    Ok(PathBuf::from(home)
        .join(".codescribe")
        .join("tool_grants.json"))
}

/// Canonical grant key for an MCP tool. One format, used by the registry
/// check and the broker write — never assemble the string anywhere else.
pub fn grant_key(server: &str, upstream_tool: &str) -> String {
    format!("{}:{}", server.to_ascii_lowercase(), upstream_tool)
}

/// One grant's payload. Only the timestamp is stored — the identity lives in the
/// map key — so the Settings list can show when consent was given.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantEntry {
    granted_at: String,
}

/// On-disk shape of `tool_grants.json`.
///
/// `always_allow` is a [`BTreeMap`] so the serialized file has a stable key order
/// and does not churn between writes. `version` exists to let a future format
/// change be detected rather than silently misread.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantsFile {
    version: u32,
    #[serde(default)]
    always_allow: BTreeMap<String, GrantEntry>,
}

impl Default for GrantsFile {
    /// Empty grants file at current VERSION with stable empty map.
    fn default() -> Self {
        Self {
            version: VERSION,
            always_allow: BTreeMap::new(),
        }
    }
}

/// Load the granted key set. Missing file → empty set. An unparsable file
/// also yields an empty set (fail-safe direction: re-ask, never crash the
/// agent loop over a config read).
pub fn load_granted() -> HashSet<String> {
    default_tool_grants_path()
        .ok()
        .map(|path| load_granted_at(&path))
        .unwrap_or_default()
}

/// [`load_granted`] against an explicit path, so tests can drive a tempdir.
fn load_granted_at(path: &Path) -> HashSet<String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    serde_json::from_str::<GrantsFile>(&raw)
        .map(|file| file.always_allow.into_keys().collect())
        .unwrap_or_default()
}

/// Record an "always allow" grant. Errors out (without writing) when an
/// existing file cannot be parsed — mutations refuse to destroy data.
pub fn grant(server: &str, upstream_tool: &str) -> Result<()> {
    grant_at(&default_tool_grants_path()?, server, upstream_tool)
}

/// [`grant`] against an explicit path. Re-granting an existing key refreshes its
/// timestamp rather than erroring.
fn grant_at(path: &Path, server: &str, upstream_tool: &str) -> Result<()> {
    let mut file = read_for_mutation(path)?;
    file.always_allow.insert(
        grant_key(server, upstream_tool),
        GrantEntry {
            granted_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    write_atomic(path, &file)
}

/// Remove a grant (Settings surface). Unknown key is a no-op success.
pub fn revoke(key: &str) -> Result<()> {
    revoke_at(&default_tool_grants_path()?, key)
}

/// [`revoke`] against an explicit path.
fn revoke_at(path: &Path, key: &str) -> Result<()> {
    let mut file = read_for_mutation(path)?;
    file.always_allow.remove(key);
    write_atomic(path, &file)
}

/// List grants for the Settings surface, sorted by key.
pub fn list() -> Result<Vec<(String, String)>> {
    let path = default_tool_grants_path()?;
    let file = read_for_mutation(&path)?;
    Ok(file
        .always_allow
        .into_iter()
        .map(|(key, entry)| (key, entry.granted_at))
        .collect())
}

/// Read the grants file for a read-modify-write cycle.
///
/// The strict counterpart to [`load_granted_at`]: a missing file is an empty
/// default, but an **unparsable** one is an error. Tolerating a parse failure
/// here would mean overwriting an operator's grants with a fresh empty map.
fn read_for_mutation(path: &Path) -> Result<GrantsFile> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The only production caller passes `default_tool_grants_path()` ($HOME/.codescribe/tool_grants.json); tests pass a tempdir. Grant identity (server, tool) becomes a JSON KEY, never a path component — see `grant_key`.
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).with_context(|| {
            format!(
                "Refusing to modify unparsable tool grants file: {}",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(GrantsFile::default()),
        Err(error) => bail!("Failed to read {}: {error}", path.display()),
    }
}

/// Persist grants atomically: write a sibling `.tmp`, `fsync` it, then rename
/// into place. A crash mid-write leaves the previous file intact — a partially
/// written grants file would silently drop consent the operator already gave.
fn write_atomic(path: &Path, file: &GrantsFile) -> Result<()> {
    let parent = path
        .parent()
        .context("Tool grants path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;
    let serialized = serde_json::to_string_pretty(file)?;
    let tmp = path.with_extension("json.tmp");
    {
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- `tmp` is the same fixed path with a ".tmp" extension; see the note on `read_for_mutation`.
        let mut handle = std::fs::File::create(&tmp)
            .with_context(|| format!("Failed to create {}", tmp.display()))?;
        handle.write_all(serialized.as_bytes())?;
        handle.sync_all()?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("Failed to move grants into place at {}", path.display()))?;
    Ok(())
}

/// Grant persistence: round-trip, corrupt-file block, key case rules.
#[cfg(test)]
mod tests {
    use super::*;

    /// Grant writes survive load; revoke drops only the named key.
    #[test]
    fn grant_roundtrips_and_revoke_removes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool_grants.json");

        grant_at(&path, "Desktop-Commander", "write_file").expect("grant");
        grant_at(&path, "desktop-commander", "edit_block").expect("grant");
        let granted = load_granted_at(&path);
        assert!(granted.contains(&grant_key("desktop-commander", "write_file")));
        assert!(granted.contains(&grant_key("DESKTOP-COMMANDER", "edit_block")));
        assert_eq!(granted.len(), 2);

        revoke_at(&path, &grant_key("desktop-commander", "write_file")).expect("revoke");
        let granted = load_granted_at(&path);
        assert!(!granted.contains(&grant_key("desktop-commander", "write_file")));
        assert_eq!(granted.len(), 1);
    }

    /// Missing → empty grants; junk JSON blocks grant writes fail-closed.
    #[test]
    fn missing_file_is_empty_and_unparsable_file_blocks_mutation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tool_grants.json");
        assert!(load_granted_at(&path).is_empty());

        std::fs::write(&path, "{ not json").expect("write junk");
        assert!(load_granted_at(&path).is_empty());
        assert!(grant_at(&path, "srv", "tool").is_err());
    }

    /// Server segment lowercased; upstream tool name keeps case.
    #[test]
    fn grant_key_is_case_insensitive_on_server_only() {
        assert_eq!(
            grant_key("Desktop-Commander", "write_file"),
            "desktop-commander:write_file"
        );
        assert_ne!(grant_key("srv", "Tool"), grant_key("srv", "tool"));
    }
}
