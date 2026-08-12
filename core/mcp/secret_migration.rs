//! Migrate plaintext MCP env credentials into Keychain account references.
//!
//! Live `~/.codescribe/mcp.json` historically stored provider API keys in
//! `env`. Migration:
//! 1. timestamped backup with restrictive permissions
//! 2. move secret-looking values into Keychain
//! 3. rewrite config with `env_refs` account names (no plaintext secrets)
//! 4. require operator rotation of already-exposed credentials (reported, not automated)
//!
//! Never prints secret values.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::config::keychain;

const SERVERS_KEY: &str = "mcpServers";

/// Keys that are safe to keep as plaintext in mcp.json (paths, non-secrets).
fn is_non_secret_env_key(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "HOME"
            | "LANG"
            | "LC_ALL"
            | "TERM"
            | "TMPDIR"
            | "USER"
            | "LOGNAME"
            | "SHELL"
            | "IJ_MCP_SERVER_PROJECT_PATH"
            | "IJ_MCP_SERVER_PORT"
    )
}

fn looks_like_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("KEY")
        || upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.contains("PASSWD")
        || upper.contains("CREDENTIAL")
        || upper.contains("AUTH")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMigrationReport {
    pub backup_path: Option<PathBuf>,
    pub migrated: Vec<MigratedSecret>,
    pub rotation_required: Vec<String>,
    pub skipped: Vec<String>,
    pub wrote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedSecret {
    pub server: String,
    pub env_key: String,
    /// Keychain account name (never the secret value).
    pub account: String,
}

/// Dry-run or apply migration for plaintext env secrets in an MCP config file.
pub fn migrate_plaintext_env_secrets(path: &Path, dry_run: bool) -> Result<SecretMigrationReport> {
    if !path.exists() {
        return Ok(SecretMigrationReport {
            backup_path: None,
            migrated: Vec::new(),
            rotation_required: Vec::new(),
            skipped: vec!["mcp.json missing".into()],
            wrote: false,
        });
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("read MCP config {}", path.display()))?;
    let mut root: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse MCP config {}", path.display()))?;

    let servers = root
        .get_mut(SERVERS_KEY)
        .and_then(Value::as_object_mut)
        .with_context(|| format!("MCP config missing {SERVERS_KEY} object"))?;

    let mut migrated = Vec::new();
    let mut rotation_required = Vec::new();
    let mut skipped = Vec::new();

    for (server_name, entry) in servers.iter_mut() {
        let Some(obj) = entry.as_object_mut() else {
            skipped.push(format!("{server_name}: not an object"));
            continue;
        };

        let env = match obj.get("env").cloned() {
            Some(Value::Object(map)) => map,
            Some(_) => {
                skipped.push(format!("{server_name}: env is not an object"));
                continue;
            }
            None => continue,
        };

        let mut env_refs: Map<String, Value> = obj
            .get("env_refs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let mut kept_env = Map::new();

        for (key, value) in env {
            let Some(secret) = value.as_str() else {
                kept_env.insert(key, value);
                continue;
            };

            if is_non_secret_env_key(&key) || !looks_like_secret_key(&key) {
                kept_env.insert(key, Value::String(secret.to_string()));
                continue;
            }

            if secret.is_empty() {
                skipped.push(format!("{server_name}.{key}: empty"));
                continue;
            }

            // Already a keychain reference marker — leave structure alone.
            if secret.starts_with("keychain:") {
                let account = secret.trim_start_matches("keychain:").to_string();
                env_refs.insert(key.clone(), Value::String(account));
                continue;
            }

            let account = keychain_account_name(server_name, &key);
            if !dry_run {
                keychain::save_key(&account, secret).with_context(|| {
                    format!("store Keychain account for {server_name}.{key} (value not logged)")
                })?;
            }

            env_refs.insert(key.clone(), Value::String(account.clone()));
            migrated.push(MigratedSecret {
                server: server_name.clone(),
                env_key: key,
                account: account.clone(),
            });
            rotation_required.push(format!(
                "{server_name}: rotate exposed credential for Keychain account {account}"
            ));
        }

        if kept_env.is_empty() {
            obj.remove("env");
        } else {
            obj.insert("env".to_string(), Value::Object(kept_env));
        }
        if env_refs.is_empty() {
            obj.remove("env_refs");
        } else {
            obj.insert("env_refs".to_string(), Value::Object(env_refs));
        }
    }

    if migrated.is_empty() {
        return Ok(SecretMigrationReport {
            backup_path: None,
            migrated,
            rotation_required,
            skipped,
            wrote: false,
        });
    }

    if dry_run {
        return Ok(SecretMigrationReport {
            backup_path: None,
            migrated,
            rotation_required,
            skipped,
            wrote: false,
        });
    }

    let backup = write_timestamped_backup(path, raw.as_bytes())?;
    write_atomic(path, &root)?;

    Ok(SecretMigrationReport {
        backup_path: Some(backup),
        migrated,
        rotation_required,
        skipped,
        wrote: true,
    })
}

/// Resolve `env` + `env_refs` into a concrete env map for process spawn.
/// Secret values are loaded from Keychain; missing refs fail closed.
pub fn resolve_server_env(
    env: &HashMap<String, String>,
    env_refs: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let mut out = env.clone();
    for (key, account) in env_refs {
        let secret = keychain::runtime_key(account).with_context(|| {
            format!("Missing Keychain secret for MCP env_ref {key} → account {account}")
        })?;
        out.insert(key.clone(), secret);
    }
    Ok(out)
}

pub fn keychain_account_name(server: &str, env_key: &str) -> String {
    let server = sanitize_account_part(server);
    let env_key = sanitize_account_part(env_key);
    format!("MCP_ENV_{server}_{env_key}")
}

fn sanitize_account_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn write_timestamped_backup(path: &Path, body: &[u8]) -> Result<PathBuf> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_extension(format!("json.bak-{secs}"));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&backup)
            .with_context(|| format!("create MCP backup {}", backup.display()))?;
        file.write_all(body)
            .with_context(|| format!("write MCP backup {}", backup.display()))?;
        file.sync_all()?;
    }
    Ok(backup)
}

fn write_atomic(path: &Path, root: &Value) -> Result<()> {
    let pretty = serde_json::to_string_pretty(root).context("serialize MCP config")?;
    let tmp = path.with_extension("tmp");
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("create temp MCP config {}", tmp.display()))?;
        file.write_all(pretty.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Human-readable report that never includes secret values.
pub fn format_report(report: &SecretMigrationReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "MCP secret migration: wrote={} migrated={}",
        report.wrote,
        report.migrated.len()
    ));
    if let Some(backup) = &report.backup_path {
        lines.push(format!("backup: {}", backup.display()));
    }
    for item in &report.migrated {
        lines.push(format!(
            "  {} env.{} → Keychain account {}",
            item.server, item.env_key, item.account
        ));
    }
    for item in &report.rotation_required {
        lines.push(format!("ROTATE: {item}"));
    }
    for item in &report.skipped {
        lines.push(format!("skip: {item}"));
    }
    if report.wrote {
        lines.push(
            "Operator action required: rotate every credential listed above — they were previously stored as plaintext."
                .into(),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn dry_run_detects_plaintext_secrets_without_writing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "brave-search": {
                  "command": "npx",
                  "args": ["-y", "@modelcontextprotocol/server-brave-search"],
                  "env": {
                    "BRAVE_API_KEY": "BSA_test_not_a_real_key_abcdefgh",
                    "PATH": "/usr/bin"
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let report = migrate_plaintext_env_secrets(&path, true).expect("dry-run");
        assert!(!report.wrote);
        assert_eq!(report.migrated.len(), 1);
        assert_eq!(report.migrated[0].env_key, "BRAVE_API_KEY");
        assert!(report.migrated[0].account.contains("BRAVE"));
        assert!(!report.rotation_required.is_empty());

        // File unchanged.
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("BSA_test_not_a_real_key"));
    }

    #[test]
    fn apply_rewrites_to_env_refs_and_backups() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "porkbun": {
                  "command": "npx",
                  "env": {
                    "PORKBUN_API_KEY": "pk_test_value_long_enough",
                    "PATH": "/usr/bin"
                  }
                }
              }
            }"#,
        )
        .unwrap();

        let report = migrate_plaintext_env_secrets(&path, false).expect("apply");
        assert!(report.wrote);
        assert!(report.backup_path.is_some());

        let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let server = &after["mcpServers"]["porkbun"];
        assert!(server["env_refs"]["PORKBUN_API_KEY"].is_string());
        assert!(
            server
                .get("env")
                .and_then(|e| e.get("PORKBUN_API_KEY"))
                .is_none()
        );
        assert_eq!(server["env"]["PATH"], "/usr/bin");

        // Backup retains original (operator rotation still required).
        let backup = fs::read_to_string(report.backup_path.as_ref().unwrap()).unwrap();
        assert!(backup.contains("pk_test_value_long_enough"));

        // Report text never embeds the secret.
        let text = format_report(&report);
        assert!(!text.contains("pk_test_value_long_enough"));
        assert!(text.contains("ROTATE:"));
    }

    #[test]
    fn non_secret_keys_stay_in_env() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "loctree": {
                  "command": "loctree-mcp",
                  "env": { "PATH": "/opt/bin", "IJ_MCP_SERVER_PORT": "64342" }
                }
              }
            }"#,
        )
        .unwrap();
        let report = migrate_plaintext_env_secrets(&path, false).unwrap();
        assert!(!report.wrote);
        assert!(report.migrated.is_empty());
    }

    #[test]
    fn resolve_server_env_merges_refs() {
        // In test env, save_key sets process env for the account name.
        let account = "MCP_ENV_TEST_RESOLVE_KEY";
        keychain::save_key(account, "resolved-secret-value").unwrap();
        let env = HashMap::from([("PATH".into(), "/bin".into())]);
        let refs = HashMap::from([("API_KEY".into(), account.to_string())]);
        let resolved = resolve_server_env(&env, &refs).expect("resolve");
        assert_eq!(resolved.get("PATH").map(String::as_str), Some("/bin"));
        assert_eq!(
            resolved.get("API_KEY").map(String::as_str),
            Some("resolved-secret-value")
        );
    }
}
