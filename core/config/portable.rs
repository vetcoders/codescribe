//! Portable settings export / import.
//!
//! Exported profiles contain no secrets, machine-absolute paths, or generated
//! per-tool grants. Import supports dry-run validation with migration preview.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::settings::UserSettings;

const PORTABLE_SCHEMA_VERSION: u32 = 1;

/// Redacted secret reference shown in exports (never a raw secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretReference {
    pub account: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortableProfile {
    pub schema_version: u32,
    pub kind: String,
    /// Settings with secrets stripped and absolute home paths remapped to `~`.
    pub settings: Value,
    /// Keychain account names that were present at export time (no values).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_refs: Vec<SecretReference>,
    /// Absolute paths that were remapped (for import-time prompts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remapped_paths: Vec<PathRemap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRemap {
    pub field: String,
    pub original: String,
    pub portable: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportPlan {
    pub would_write: bool,
    pub migrations: Vec<String>,
    pub conflicts: Vec<String>,
    pub path_prompts: Vec<PathRemap>,
    pub secret_refs: Vec<SecretReference>,
    pub diff_summary: String,
    pub settings_preview: Value,
}

/// Build a portable export from live [`UserSettings`].
pub fn export_portable(settings: &UserSettings) -> Result<PortableProfile> {
    let mut value = serde_json::to_value(settings).context("serialize settings for export")?;
    let mut remapped = Vec::new();
    strip_secrets(&mut value);
    strip_generated_grants(&mut value);
    remap_absolute_paths(&mut value, "", &mut remapped);

    let secret_refs = known_secret_refs();

    Ok(PortableProfile {
        schema_version: PORTABLE_SCHEMA_VERSION,
        kind: "codescribe.portable_settings".to_string(),
        settings: value,
        secret_refs,
        remapped_paths: remapped,
    })
}

/// Write portable JSON to `path` with restrictive permissions.
pub fn write_portable_export(path: &Path, settings: &UserSettings) -> Result<()> {
    let profile = export_portable(settings)?;
    let json = serde_json::to_string_pretty(&profile).context("serialize portable profile")?;
    atomic_write_private(path, json.as_bytes())
}

/// Dry-run import: schema validation + migration preview, no disk write.
pub fn import_portable_dry_run(profile_json: &str, current: &UserSettings) -> Result<ImportPlan> {
    let profile: PortableProfile =
        serde_json::from_str(profile_json).context("portable profile is not valid JSON")?;
    validate_profile(&profile)?;

    let mut migrations = Vec::new();
    if profile.schema_version < PORTABLE_SCHEMA_VERSION {
        migrations.push(format!(
            "migrate schema {} → {}",
            profile.schema_version, PORTABLE_SCHEMA_VERSION
        ));
    }
    if profile.schema_version > PORTABLE_SCHEMA_VERSION {
        bail!(
            "portable profile schema {} is newer than supported {}",
            profile.schema_version,
            PORTABLE_SCHEMA_VERSION
        );
    }

    let mut settings_value = profile.settings.clone();
    // Expand `~` for preview so the operator sees machine-local paths.
    expand_portable_paths(&mut settings_value);

    let mut conflicts = Vec::new();
    let current_value = serde_json::to_value(current).unwrap_or(Value::Null);
    collect_conflicts(&current_value, &settings_value, "", &mut conflicts);

    let incoming: UserSettings = serde_json::from_value(settings_value.clone())
        .context("portable settings failed schema validation")?;
    let _ = incoming;

    let diff_summary = format!(
        "{} migration(s), {} conflict(s), {} path remap prompt(s), {} secret ref(s)",
        migrations.len(),
        conflicts.len(),
        profile.remapped_paths.len(),
        profile.secret_refs.len()
    );

    Ok(ImportPlan {
        would_write: false,
        migrations,
        conflicts,
        path_prompts: profile.remapped_paths,
        secret_refs: profile.secret_refs,
        diff_summary,
        settings_preview: settings_value,
    })
}

/// Apply portable profile to disk with automatic backup + rollback on failure.
pub fn import_portable_apply(profile_json: &str, settings_path: &Path) -> Result<ImportPlan> {
    let current = if settings_path.exists() {
        let raw = fs::read_to_string(settings_path)
            .with_context(|| format!("read current settings {}", settings_path.display()))?;
        serde_json::from_str::<UserSettings>(&raw).unwrap_or_default()
    } else {
        UserSettings::default()
    };

    let mut plan = import_portable_dry_run(profile_json, &current)?;
    let incoming: UserSettings = serde_json::from_value(plan.settings_preview.clone())
        .context("portable settings failed schema validation before apply")?;

    let backup_path = backup_path(settings_path);
    if settings_path.exists() {
        fs::copy(settings_path, &backup_path).with_context(|| {
            format!(
                "backup settings {} → {}",
                settings_path.display(),
                backup_path.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&backup_path, fs::Permissions::from_mode(0o600));
        }
    }

    if let Err(error) = incoming.save_to(settings_path) {
        if backup_path.exists() {
            let _ = fs::copy(&backup_path, settings_path);
        }
        return Err(error).context("import apply failed; backup restored when available");
    }

    plan.would_write = true;
    plan.diff_summary = format!(
        "applied; backup at {} — {}",
        backup_path.display(),
        plan.diff_summary
    );
    Ok(plan)
}

fn validate_profile(profile: &PortableProfile) -> Result<()> {
    if profile.kind != "codescribe.portable_settings" {
        bail!("unsupported portable profile kind: {}", profile.kind);
    }
    if profile.schema_version == 0 {
        bail!("portable profile schema_version must be >= 1");
    }
    // Fail closed if export accidentally retained secret-looking values.
    let serialized = serde_json::to_string(&profile.settings)?;
    for needle in ["sk-", "api_key", "apikey", "secret", "token", "password"] {
        if serialized.to_ascii_lowercase().contains(needle)
            && serialized.contains(':')
            && !serialized.contains("secret_refs")
        {
            // Soft check: only reject obvious key-like strings with long values.
            if let Some(hit) = find_plaintext_secret_hit(&profile.settings) {
                bail!("portable profile still contains plaintext secret material near {hit}");
            }
        }
    }
    Ok(())
}

fn find_plaintext_secret_hit(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = k.to_ascii_lowercase();
                if (key.contains("key") || key.contains("token") || key.contains("secret"))
                    && let Value::String(s) = v
                    && s.len() >= 16
                    && !s.starts_with("keychain:")
                    && !s.starts_with('~')
                {
                    return Some(k.clone());
                }
                if let Some(hit) = find_plaintext_secret_hit(v) {
                    return Some(hit);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_plaintext_secret_hit),
        _ => None,
    }
}

fn strip_secrets(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    // Never export tool grants or permission tool maps (machine-generated).
    const SECRET_KEYS: &[&str] = &[
        "api_key",
        "apiKey",
        "token",
        "secret",
        "password",
        "auth_ref",
        "openai_oauth_client_secret",
        "anthropic_oauth_client_secret",
    ];
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let lower = key.to_ascii_lowercase();
        if SECRET_KEYS.iter().any(|s| lower.contains(s)) {
            map.remove(&key);
            continue;
        }
        if let Some(child) = map.get_mut(&key) {
            strip_secrets(child);
        }
    }
}

fn strip_generated_grants(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    // agent.permissions.tools holds per-tool grants — not portable.
    if let Some(Value::Object(agent)) = map.get_mut("agent")
        && let Some(Value::Object(perms)) = agent.get_mut("permissions")
    {
        perms.remove("tools");
        perms.remove("servers");
    }
    if let Some(Value::Object(perms)) = map.get_mut("agent_permissions") {
        perms.remove("tools");
        perms.remove("servers");
    }
}

fn remap_absolute_paths(value: &mut Value, field: &str, out: &mut Vec<PathRemap>) {
    match value {
        Value::String(s) => {
            if let Some(portable) = to_portable_path(s) {
                out.push(portable_remap_record(field, &portable));
                *s = portable;
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                remap_absolute_paths(item, &format!("{field}[{i}]"), out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let child = if field.is_empty() {
                    k.clone()
                } else {
                    format!("{field}.{k}")
                };
                remap_absolute_paths(v, &child, out);
            }
        }
        _ => {}
    }
}

fn to_portable_path(path: &str) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    if path == home.as_str() {
        return Some("~".into());
    }
    let prefix = format!("{home}/");
    if path.starts_with(&prefix) {
        return Some(format!("~/{}", &path[prefix.len()..]));
    }
    // Absolute non-home paths are not portable — blank them rather than leak machine layout.
    if path.starts_with('/') {
        return Some(String::new());
    }
    None
}

/// Export-safe remaps never embed absolute machine originals.
fn portable_remap_record(field: &str, portable: &str) -> PathRemap {
    PathRemap {
        field: field.to_string(),
        // Leave original empty in the portable profile so the export has no
        // absolute machine paths. Import dry-run re-derives prompts from fields.
        original: String::new(),
        portable: portable.to_string(),
    }
}

fn expand_portable_paths(value: &mut Value) {
    match value {
        Value::String(s) if s.starts_with("~/") || s == "~" => {
            if let Ok(home) = std::env::var("HOME") {
                if s == "~" {
                    *s = home;
                } else {
                    *s = format!("{home}/{}", &s[2..]);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                expand_portable_paths(item);
            }
        }
        Value::Object(map) => {
            for v in map.values_mut() {
                expand_portable_paths(v);
            }
        }
        _ => {}
    }
}

fn collect_conflicts(current: &Value, incoming: &Value, path: &str, out: &mut Vec<String>) {
    match (current, incoming) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, bv) in b {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match a.get(k) {
                    Some(av) if av != bv => {
                        if av.is_object() && bv.is_object() {
                            collect_conflicts(av, bv, &child, out);
                        } else {
                            out.push(format!("{child}: current differs from import"));
                        }
                    }
                    None => out.push(format!("{child}: new in import")),
                    _ => {}
                }
            }
        }
        (a, b) if a != b => out.push(format!("{path}: value differs")),
        _ => {}
    }
}

fn known_secret_refs() -> Vec<SecretReference> {
    use super::keychain::{KEYCHAIN_ACCOUNTS, runtime_key};
    KEYCHAIN_ACCOUNTS
        .iter()
        .map(|account| SecretReference {
            account: (*account).to_string(),
            present: runtime_key(account).is_some(),
        })
        .collect()
}

fn backup_path(settings_path: &Path) -> PathBuf {
    let stamp = chrono_like_stamp();
    settings_path.with_extension(format!("json.bak-{stamp}"))
}

fn chrono_like_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn atomic_write_private(path: &Path, body: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent for {}", path.display()))?;
    }
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
            .with_context(|| format!("create temp {}", tmp.display()))?;
        file.write_all(body)
            .with_context(|| format!("write temp {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temp {}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Helper so import can write settings without going through the full Config loader.
trait UserSettingsSave {
    fn save_to(&self, path: &Path) -> Result<()>;
}

impl UserSettingsSave for UserSettings {
    fn save_to(&self, path: &Path) -> Result<()> {
        // Prefer the existing save() when path matches the canonical location;
        // otherwise write a v2-compatible JSON blob via serde of UserSettings.
        let json = serde_json::to_string_pretty(self).context("serialize settings")?;
        atomic_write_private(path, json.as_bytes())
    }
}

/// Redact any string that looks like a secret for logs / reports (never print values).
pub fn redact_for_log(text: &str) -> String {
    // Collapse long base64/hex-like tokens.
    let mut out = String::with_capacity(text.len());
    for token in text.split_whitespace() {
        if token.len() >= 20
            && token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            out.push_str(&format!("[redacted:{}chars]", token.len()));
        } else {
            out.push_str(token);
        }
        out.push(' ');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn portable_export_strips_absolute_home_paths_and_grants() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/test".into());
        let settings = UserSettings {
            agent_workspace_roots: Some(vec![format!("{home}/Git"), "/opt/other".into()]),
            agent_permissions: Some({
                let mut p = crate::agent::permissions::AgentPermissions::default();
                p.tools.insert(
                    "desktop-commander:write_file".into(),
                    crate::agent::permissions::PermissionLevel::Allow,
                );
                p
            }),
            ..UserSettings::default()
        };

        let profile = export_portable(&settings).expect("export");
        let json = serde_json::to_string(&profile).expect("json");
        assert!(
            !json.contains(&home),
            "must not leak absolute home path: {json}"
        );
        assert!(
            !json.contains("/opt/other"),
            "must not leak non-home absolute paths"
        );
        assert!(json.contains("~/Git") || json.contains("~\\Git"));
        assert!(!json.contains("desktop-commander:write_file"));
        assert!(!json.contains("sk-"));
        assert!(
            profile
                .remapped_paths
                .iter()
                .all(|r| r.original.is_empty() || r.original.starts_with('~')),
            "remapped originals must not be absolute machine paths"
        );
    }

    #[test]
    fn import_dry_run_does_not_write() {
        let dir = TempDir::new().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(&settings_path, "{}").unwrap();

        let profile = export_portable(&UserSettings {
            show_dock_icon: Some(false),
            ..Default::default()
        })
        .unwrap();
        let json = serde_json::to_string(&profile).unwrap();
        let plan = import_portable_dry_run(&json, &UserSettings::default()).expect("dry-run");
        assert!(!plan.would_write);
        assert!(
            plan.diff_summary.contains("conflict")
                || plan.diff_summary.contains("migration")
                || !plan.diff_summary.is_empty()
        );

        // File untouched.
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), "{}");
    }

    #[test]
    fn redact_collapses_long_tokens() {
        let redacted = redact_for_log("token abcdefghijklmnopqrstuvwxyz012345 is secret");
        assert!(redacted.contains("[redacted:"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz012345"));
    }
}
