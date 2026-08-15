//! MCP config store — CRUD over `~/.codescribe/mcp.json` for the Settings
//! management UI.
//!
//! Two hard guarantees so a hand-edited config is never silently destroyed:
//!   1. **Unknown fields are preserved.** Mutations operate on the raw JSON tree
//!      (`serde_json::Value`) and only touch the specific server entry's
//!      `command` / `args` / `enabled`. Per-server `env`, `timeout_seconds`, any
//!      custom keys, and unrelated top-level keys survive untouched.
//!   2. **Writes are atomic.** We serialize to a sibling temp file, `fsync`, then
//!      `rename` over the target (atomic on the same filesystem) so a crash mid
//!      write can never leave a truncated `mcp.json`.
//!
//! A present-but-invalid `mcp.json` makes every mutation error out *before*
//! writing — we refuse to overwrite JSON we could not parse.
//!
//! This module also hosts the one-shot "test this server" runner (spawn +
//! `initialize` + `tools/list`) used by the Settings Test button.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::mcp::{McpClient, McpConfigFile, McpServerConfig, default_mcp_config_path};

/// JSON object key for the server map inside `mcp.json` (`mcpServers`).
const SERVERS_KEY: &str = "mcpServers";

/// A server row for the management UI: identity + spawn shape + the NAMES of any
/// env vars (never their values — secrets stay on disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSummary {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub auth_ref: Option<String>,
}

/// Desired spawn shape when adding / updating a server through the UI. Env is not
/// edited here (secrets stay file-side); `update_server` preserves any existing
/// `env` block.
#[derive(Debug, Clone)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub endpoint: Option<String>,
    pub auth_ref: Option<String>,
}

/// List every configured server (sorted by name) from the canonical config path
/// (`~/.codescribe/mcp.json`). A missing `mcp.json` is an empty list, never an
/// error — the UI shows an empty section with an add form.
pub fn list_servers() -> Result<Vec<McpServerSummary>> {
    list_servers_at(&default_mcp_config_path()?)
}

/// Path-explicit twin of [`list_servers`]. Every public entry point in this
/// module delegates to an `_at` variant so the tests can drive the real logic
/// against a temp dir instead of the operator's live `~/.codescribe/mcp.json`.
pub fn list_servers_at(path: &Path) -> Result<Vec<McpServerSummary>> {
    let Some(config) = McpConfigFile::load_optional(path)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<McpServerSummary> = config
        .servers
        .into_iter()
        .map(|(name, cfg)| {
            let mut env_keys: Vec<String> = cfg.env.into_keys().collect();
            env_keys.sort();
            McpServerSummary {
                name,
                command: cfg.command,
                args: cfg.args,
                env_keys,
                enabled: cfg.enabled.unwrap_or(true),
                endpoint: cfg.url,
                auth_ref: cfg.auth_ref,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Add a new server. Errors if the name already exists so a real edit is never
/// silently overwritten by an "add". Creates `mcp.json` (and its parent dir) when
/// absent.
pub fn add_server(spec: &McpServerSpec) -> Result<()> {
    add_server_at(&default_mcp_config_path()?, spec)
}

/// Path-explicit twin of [`add_server`]. Both validations run BEFORE the file is
/// touched, so a rejected spec leaves no `mcp.json` behind at all.
fn add_server_at(path: &Path, spec: &McpServerSpec) -> Result<()> {
    validate_name(&spec.name)?;
    validate_server_spec(spec)?;

    let mut root = load_value(path)?;
    {
        let servers = servers_map_mut(&mut root)?;
        if servers.contains_key(&spec.name) {
            bail!("MCP server \"{}\" already exists", spec.name);
        }
        servers.insert(spec.name.clone(), server_object(spec));
    }
    write_atomic(path, &root)
}

/// Update an existing server's spawn shape in place, PRESERVING every other field
/// of that entry (`env`, `timeout_seconds`, custom keys) and every unrelated
/// top-level key. Errors if the named server does not exist.
pub fn update_server(name: &str, spec: &McpServerSpec) -> Result<()> {
    update_server_at(&default_mcp_config_path()?, name, spec)
}

/// Path-explicit twin of [`update_server`]. Mutates the existing entry in place
/// rather than replacing it — that in-place edit is what preserves `env`,
/// `timeout_seconds`, and any hand-added key on the server being updated.
///
/// Note the name/spec split: `name` addresses the entry to edit while
/// `spec.name` is ignored here, so this call cannot rename a server.
fn update_server_at(path: &Path, name: &str, spec: &McpServerSpec) -> Result<()> {
    validate_server_spec(spec)?;

    let mut root = load_value(path)?;
    {
        let servers = servers_map_mut(&mut root)?;
        let entry = servers
            .get_mut(name)
            .with_context(|| format!("MCP server \"{name}\" not found"))?;
        let obj = entry
            .as_object_mut()
            .with_context(|| format!("MCP server \"{name}\" is not a JSON object"))?;
        write_server_shape(obj, spec);
        obj.insert("enabled".to_string(), Value::Bool(spec.enabled));
    }
    write_atomic(path, &root)
}

/// Remove a server. Errors if it does not exist.
pub fn remove_server(name: &str) -> Result<()> {
    remove_server_at(&default_mcp_config_path()?, name)
}

/// Path-explicit twin of [`remove_server`]. Removing an absent server is an
/// error, not a silent success — a typo must not read as "already gone".
fn remove_server_at(path: &Path, name: &str) -> Result<()> {
    let mut root = load_value(path)?;
    {
        let servers = servers_map_mut(&mut root)?;
        if servers.remove(name).is_none() {
            bail!("MCP server \"{name}\" not found");
        }
    }
    write_atomic(path, &root)
}

/// Health-probe result for one server: the identity it advertised in the
/// `initialize` handshake (name / version / protocol, each optional) plus its
/// live tool count. Surfaced next to the server in the Settings management list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpProbeSummary {
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub protocol_version: Option<String>,
    pub tool_count: usize,
}

/// Spawn the named server, handshake, and return its identity + live tool count.
/// Blocking: runs the async discovery on a dedicated thread + one-shot
/// current-thread runtime so it is safe to call from a synchronous FFI context
/// (and from within an already-running runtime). `timeout` bounds the whole
/// handshake.
pub fn probe_server_blocking(name: &str, timeout: Duration) -> Result<McpProbeSummary> {
    probe_server_blocking_at(&default_mcp_config_path()?, name, timeout)
}

/// Path-explicit twin of [`probe_server_blocking`]. Uses the strict
/// `McpConfigFile::load` (not `load_optional`): probing against a missing config
/// is a hard error, since there is nothing to spawn.
fn probe_server_blocking_at(path: &Path, name: &str, timeout: Duration) -> Result<McpProbeSummary> {
    let config = McpConfigFile::load(path)?;
    let server = config
        .servers
        .get(name)
        .with_context(|| format!("MCP server \"{name}\" not found"))?
        .clone();
    run_probe_blocking(server, timeout)
}

/// Tool-count-only convenience over [`probe_server_blocking`], preserved for the
/// simpler "how many tools" callers.
pub fn test_server_blocking(name: &str, timeout: Duration) -> Result<usize> {
    Ok(probe_server_blocking(name, timeout)?.tool_count)
}

/// Path-explicit twin of [`test_server_blocking`] for temp-dir integration tests.
#[cfg(test)]
fn test_server_blocking_at(path: &Path, name: &str, timeout: Duration) -> Result<usize> {
    Ok(probe_server_blocking_at(path, name, timeout)?.tool_count)
}

// --- internals ------------------------------------------------------------

/// Run the async handshake to completion from a synchronous caller.
///
/// The dedicated thread is not optional: building a current-thread runtime
/// inside an already-running tokio runtime panics, and this path is reached from
/// the synchronous FFI surface as well as from async Settings code. Spawning
/// isolates the new runtime from whatever the caller is standing in. A panic in
/// the probe surfaces as an `Err` rather than unwinding into the caller.
fn run_probe_blocking(server: McpServerConfig, timeout: Duration) -> Result<McpProbeSummary> {
    std::thread::spawn(move || -> Result<McpProbeSummary> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create MCP test runtime")?;
        runtime.block_on(async move {
            let client = McpClient::new(server).with_timeout(timeout);
            let probe = client.probe().await?;
            Ok(McpProbeSummary {
                server_name: probe.handshake.server_name(),
                server_version: probe.handshake.server_version(),
                protocol_version: probe.handshake.protocol_version.clone(),
                tool_count: probe.tools.len(),
            })
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("MCP test thread panicked"))?
}

/// Build a fresh JSON entry for a newly added server. Unlike the update path
/// there is nothing to preserve here, so the object starts empty.
fn server_object(spec: &McpServerSpec) -> Value {
    let mut obj = Map::new();
    write_server_shape(&mut obj, spec);
    obj.insert("enabled".to_string(), Value::Bool(spec.enabled));
    Value::Object(obj)
}

/// Write the spawn shape (local command vs remote endpoint) onto an existing
/// entry, touching only the keys that define it.
///
/// The two shapes are mutually exclusive, so each branch REMOVES the other's
/// keys: flipping a server from local to remote must not leave a stale
/// `command` behind for a loader to pick up, and flipping back must not leave a
/// stale `url` / `auth_ref`. Keys outside this set (`env`, `timeout_seconds`,
/// custom fields) are never named here, which is what makes the update path
/// preservation-safe.
fn write_server_shape(obj: &mut Map<String, Value>, spec: &McpServerSpec) {
    if let Some(endpoint) = &spec.endpoint {
        obj.remove("command");
        obj.remove("args");
        obj.insert("url".to_string(), Value::String(endpoint.clone()));
        obj.insert(
            "transport".to_string(),
            Value::String("streamable_http".to_string()),
        );
        match &spec.auth_ref {
            Some(auth_ref) => {
                obj.insert("auth_ref".to_string(), Value::String(auth_ref.clone()));
            }
            None => {
                obj.remove("auth_ref");
            }
        }
    } else {
        obj.remove("url");
        obj.remove("endpoint");
        obj.remove("transport");
        obj.remove("auth_ref");
        obj.insert("command".to_string(), Value::String(spec.command.clone()));
        obj.insert("args".to_string(), args_value(&spec.args));
    }
}

/// Convert argv to a JSON string array.
fn args_value(args: &[String]) -> Value {
    Value::Array(args.iter().cloned().map(Value::String).collect())
}

/// Load the raw config tree for mutation. A missing file yields a fresh
/// `{ "mcpServers": {} }`. A present-but-unparseable file is a hard error — we
/// refuse to clobber JSON we could not read.
fn load_value(path: &Path) -> Result<Value> {
    if !path.exists() {
        let mut root = Map::new();
        root.insert(SERVERS_KEY.to_string(), Value::Object(Map::new()));
        return Ok(Value::Object(root));
    }
    // Sanitize before reading: `canonicalize` resolves `..` segments and symlinks
    // to a real absolute path, so the value handed to the filesystem is a
    // validated path rather than an unchecked string.
    let path = path
        .canonicalize()
        .with_context(|| format!("Failed to resolve MCP config {}", path.display()))?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read MCP config {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| {
        format!(
            "{} is not valid JSON — refusing to overwrite it",
            path.display()
        )
    })
}

/// Borrow the `mcpServers` object mutably, creating it if absent. Errors if the
/// root or `mcpServers` is present but not a JSON object.
fn servers_map_mut(root: &mut Value) -> Result<&mut Map<String, Value>> {
    let obj = root
        .as_object_mut()
        .context("mcp.json root must be a JSON object")?;
    let entry = obj
        .entry(SERVERS_KEY.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry
        .as_object_mut()
        .context("\"mcpServers\" must be a JSON object")
}

/// Atomic write: serialize pretty, write a sibling temp, fsync, rename over the
/// target. Best-effort cleanup of the temp on failure.
fn write_atomic(path: &Path, value: &Value) -> Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config dir {}", parent.display()))?;
    }

    let mut bytes = serde_json::to_vec_pretty(value).context("Failed to serialize mcp.json")?;
    bytes.push(b'\n');

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mcp.json");
    let tmp_name = format!(".{file_name}.tmp.{}", std::process::id());
    let tmp_path = match parent {
        Some(parent) => parent.join(tmp_name),
        None => std::path::PathBuf::from(tmp_name),
    };

    let write_result = (|| -> Result<()> {
        // 0o600 before any byte lands: per-server `env` blocks carry secrets, and
        // rename preserves the temp file's mode — a default-umask create here
        // would leave mcp.json world-readable (secret_migration.rs sets the same
        // mode on its writes).
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp_path)
            .with_context(|| format!("Failed to create temp {}", tmp_path.display()))?;
        file.write_all(&bytes)
            .context("Failed to write temp mcp.json")?;
        file.sync_all().context("Failed to fsync temp mcp.json")?;
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }

    std::fs::rename(&tmp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::Error::new(error).context(format!("Failed to install {}", path.display()))
    })
}

/// Reject server names that cannot be addressed safely.
///
/// The charset is deliberately narrow (ASCII alphanumerics, `_`, `-`): the name
/// is a JSON object key AND a user-facing identifier, so quoting, whitespace,
/// and lookalike Unicode would all become ways to shadow an existing server.
/// Surrounding whitespace is rejected rather than trimmed, so what the operator
/// typed is what gets stored.
fn validate_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("MCP server name is empty");
    }
    if trimmed != name {
        bail!("MCP server name must not have surrounding whitespace");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!(
            "MCP server name \"{name}\" contains unsupported characters (use letters, digits, '_' or '-')"
        );
    }
    Ok(())
}

/// Reject an empty (or whitespace-only) spawn command — a local server with
/// nothing to execute would fail at probe time instead of at save time.
fn validate_command(command: &str) -> Result<()> {
    if command.trim().is_empty() {
        bail!("MCP server command is empty");
    }
    Ok(())
}

/// Validate a spec against the shape it claims to be — remote or local, never
/// both.
///
/// Remote endpoints carry three extra rules: the scheme must be `http`/`https`
/// (no `file://` or custom schemes reaching the transport), userinfo credentials
/// are refused outright because `mcp.json` is not the place for secrets
/// (Keychain via `auth_ref` is), and a remote entry must not also define a local
/// command — an ambiguous entry would let the loader pick the shape for us.
fn validate_server_spec(spec: &McpServerSpec) -> Result<()> {
    match spec.endpoint.as_deref() {
        Some(endpoint) => {
            let parsed = reqwest::Url::parse(endpoint)
                .with_context(|| format!("Invalid remote MCP endpoint: {endpoint}"))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                bail!("Remote MCP endpoint must use http or https");
            }
            if !parsed.username().is_empty() || parsed.password().is_some() {
                bail!("Remote MCP credentials must be stored in Keychain, not in the endpoint URL");
            }
            if !spec.command.trim().is_empty() || !spec.args.is_empty() {
                bail!("Remote MCP server must not also define a local command");
            }
        }
        None => validate_command(&spec.command)?,
    }
    Ok(())
}

/// Temp-dir CRUD, permission, preservation, and mock-server probe tests.
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    /// Local enabled server fixture with no remote endpoint or auth_ref.
    fn spec(name: &str, command: &str, args: &[&str]) -> McpServerSpec {
        McpServerSpec {
            name: name.to_string(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            enabled: true,
            endpoint: None,
            auth_ref: None,
        }
    }

    /// Parse `mcp.json` as a raw JSON value for field-preservation assertions.
    fn read_raw(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse")
    }

    /// Add two servers; list is sorted by name and returns command/args/enabled.
    #[test]
    fn add_then_list_roundtrips() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // does not exist yet

        assert!(list_servers_at(&path).expect("list empty").is_empty());

        add_server_at(&path, &spec("loctree-mcp", "loctree-mcp", &["mcp"])).expect("add");
        add_server_at(&path, &spec("aicx-mcp", "aicx", &["mcp"])).expect("add");

        let servers = list_servers_at(&path).expect("list");
        // Sorted by name.
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "aicx-mcp");
        assert_eq!(servers[1].name, "loctree-mcp");
        assert_eq!(servers[1].command, "loctree-mcp");
        assert_eq!(servers[1].args, vec!["mcp".to_string()]);
        assert!(servers[1].enabled);
    }

    /// Fresh and re-written `mcp.json` must be mode 0o600 (env secrets on disk).
    #[cfg(unix)]
    #[test]
    fn writes_land_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");

        add_server_at(&path, &spec("loctree-mcp", "loctree-mcp", &["mcp"])).expect("add");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "fresh mcp.json must be owner-only");

        // A pre-existing world-readable config gets replaced by the 0o600 temp on
        // the next mutation (rename installs the temp's mode).
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        add_server_at(&path, &spec("aicx-mcp", "aicx", &["mcp"])).expect("add second");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mutation must tighten a loose mcp.json");
    }

    /// Remote rows store url + auth_ref only; never raw tokens in the file body.
    #[test]
    fn remote_server_persists_only_endpoint_and_keychain_reference() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let token_fragment = "super-secret-token-fragment";
        let spec = McpServerSpec {
            name: "slack".to_string(),
            command: String::new(),
            args: vec![],
            enabled: true,
            endpoint: Some("https://connector.example/mcp".to_string()),
            auth_ref: Some("MCP_CONNECTOR_SLACK_TOKEN".to_string()),
        };
        add_server_at(&path, &spec).expect("add remote");

        let raw = std::fs::read_to_string(&path).expect("read config");
        assert!(raw.contains("https://connector.example/mcp"));
        assert!(raw.contains("MCP_CONNECTOR_SLACK_TOKEN"));
        assert!(!raw.contains(token_fragment));
        assert!(!raw.contains("\"token\""));

        let listed = list_servers_at(&path).expect("list remote");
        assert_eq!(listed[0].endpoint.as_deref(), spec.endpoint.as_deref());
        assert_eq!(listed[0].auth_ref.as_deref(), spec.auth_ref.as_deref());
        assert!(listed[0].command.is_empty());
    }

    /// Adding a peer must not strip foreign top-level keys or sibling env blocks.
    #[test]
    fn add_preserves_unknown_fields() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        // A hand-edited config with an unknown top-level key and a server carrying
        // an env block + a custom field.
        let original = json!({
            "mcpServers": {
                "keep": {
                    "command": "keeper",
                    "env": { "SECRET_TOKEN": "s3cr3t" },
                    "customField": 123
                }
            },
            "topLevelExtra": true
        });
        std::fs::write(&path, original.to_string()).expect("seed");

        add_server_at(&path, &spec("added", "added-cmd", &["x"])).expect("add");

        let raw = read_raw(&path);
        // Unknown top-level key intact.
        assert_eq!(raw["topLevelExtra"], json!(true));
        // Existing server's env + custom field untouched (secret preserved).
        assert_eq!(raw["mcpServers"]["keep"]["customField"], json!(123));
        assert_eq!(
            raw["mcpServers"]["keep"]["env"]["SECRET_TOKEN"],
            json!("s3cr3t")
        );
        // New server present.
        assert_eq!(raw["mcpServers"]["added"]["command"], json!("added-cmd"));
    }

    /// Update rewrites spawn shape only; env, timeouts, and custom keys survive.
    #[test]
    fn update_preserves_env_and_custom_keys() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let original = json!({
            "mcpServers": {
                "srv": {
                    "command": "old",
                    "args": ["a"],
                    "enabled": true,
                    "env": { "TOKEN": "keepme" },
                    "timeout_seconds": 42,
                    "codescribePolicy": { "profile": "desktop-commander-v1" },
                    "weird": [1, 2, 3]
                }
            }
        });
        std::fs::write(&path, original.to_string()).expect("seed");

        let mut updated = spec("srv", "new-cmd", &["b", "c"]);
        updated.enabled = false;
        update_server_at(&path, "srv", &updated).expect("update");

        let raw = read_raw(&path);
        assert_eq!(raw["mcpServers"]["srv"]["command"], json!("new-cmd"));
        assert_eq!(raw["mcpServers"]["srv"]["args"], json!(["b", "c"]));
        assert_eq!(raw["mcpServers"]["srv"]["enabled"], json!(false));
        // Preserved untouched.
        assert_eq!(raw["mcpServers"]["srv"]["env"]["TOKEN"], json!("keepme"));
        assert_eq!(raw["mcpServers"]["srv"]["timeout_seconds"], json!(42));
        assert_eq!(
            raw["mcpServers"]["srv"]["codescribePolicy"]["profile"],
            json!("desktop-commander-v1")
        );
        assert_eq!(raw["mcpServers"]["srv"]["weird"], json!([1, 2, 3]));
    }

    /// Remove is named-only; missing names error rather than no-op.
    #[test]
    fn remove_deletes_only_the_named_server() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server_at(&path, &spec("a", "a", &[])).expect("add");
        add_server_at(&path, &spec("b", "b", &[])).expect("add");

        remove_server_at(&path, "a").expect("remove");
        let names: Vec<String> = list_servers_at(&path)
            .expect("list")
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["b".to_string()]);

        // Removing a missing server errors.
        assert!(remove_server_at(&path, "ghost").is_err());
    }

    /// Unparseable config blocks every mutation and keeps original bytes.
    #[test]
    fn invalid_json_is_never_clobbered() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        std::fs::write(&path, "{ not valid json").expect("seed garbage");

        // Every mutation refuses to run and leaves the file byte-for-byte intact.
        assert!(add_server_at(&path, &spec("x", "x", &[])).is_err());
        assert!(remove_server_at(&path, "x").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("still there"),
            "{ not valid json"
        );
    }

    /// Invalid names and empty commands fail before any file is created.
    #[test]
    fn rejects_invalid_names_and_empty_command() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");

        assert!(add_server_at(&path, &spec("bad name", "cmd", &[])).is_err());
        assert!(add_server_at(&path, &spec("", "cmd", &[])).is_err());
        assert!(add_server_at(&path, &spec("ok", "   ", &[])).is_err());
        // Nothing was written by the rejected adds.
        assert!(!path.exists());
    }

    /// Duplicate names error without overwriting the first entry's command.
    #[test]
    fn duplicate_add_is_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server_at(&path, &spec("dup", "cmd", &[])).expect("add");
        assert!(add_server_at(&path, &spec("dup", "other", &[])).is_err());
        // Original command survives the rejected duplicate.
        assert_eq!(
            list_servers_at(&path).expect("list")[0].command,
            "cmd".to_string()
        );
    }

    /// Atomic write must re-load cleanly through the typed `McpConfigFile` loader.
    #[test]
    fn written_config_is_valid_and_reparses_through_typed_loader() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server_at(&path, &spec("srv", "cmd", &["one", "two"])).expect("add");
        // Atomic write must leave a clean file the typed loader accepts.
        let config = McpConfigFile::load(&path).expect("typed reload");
        let server = config.servers.get("srv").expect("server present");
        assert_eq!(server.command, "cmd");
        assert_eq!(server.args, vec!["one".to_string(), "two".to_string()]);
    }

    /// Mock MCP fixture exposes exactly one tool through the blocking probe.
    #[test]
    fn test_server_blocking_lists_mock_tools() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let script = repo_root()
            .join("tests")
            .join("fixtures")
            .join("mock_mcp.py");
        let mut server = spec("mock", "python3", &[&script.display().to_string()]);
        server.enabled = true;
        add_server_at(&path, &server).expect("add");

        let count = test_server_blocking_at(&path, "mock", Duration::from_secs(5)).expect("test");
        assert_eq!(count, 1, "mock server exposes one tool");
    }

    /// Probe returns mock handshake name/version/protocol plus tool_count.
    #[test]
    fn probe_server_blocking_reports_handshake_identity() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        let script = repo_root()
            .join("tests")
            .join("fixtures")
            .join("mock_mcp.py");
        let mut server = spec("mock", "python3", &[&script.display().to_string()]);
        server.enabled = true;
        add_server_at(&path, &server).expect("add");

        let summary =
            probe_server_blocking_at(&path, "mock", Duration::from_secs(5)).expect("probe");
        assert_eq!(summary.tool_count, 1);
        assert_eq!(summary.server_name.as_deref(), Some("mock-mcp"));
        assert_eq!(summary.server_version.as_deref(), Some("0.1.0"));
        assert_eq!(summary.protocol_version.as_deref(), Some("2025-06-18"));
    }

    /// Probing a name absent from config is a hard error.
    #[test]
    fn test_server_blocking_errors_for_missing_server() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server_at(&path, &spec("present", "python3", &[])).expect("add");
        assert!(test_server_blocking_at(&path, "absent", Duration::from_secs(1)).is_err());
    }

    /// Workspace root from `CARGO_MANIFEST_DIR` so fixtures resolve under tests/.
    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("core manifest has a repo parent")
            .to_path_buf()
    }
}
