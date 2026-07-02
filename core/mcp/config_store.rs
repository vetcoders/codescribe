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

use crate::mcp::{McpClient, McpConfigFile, McpServerConfig};

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
}

/// List every configured server (sorted by name). A missing `mcp.json` is an
/// empty list, never an error — the UI shows an empty section with an add form.
pub fn list_servers(path: &Path) -> Result<Vec<McpServerSummary>> {
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
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Add a new server. Errors if the name already exists so a real edit is never
/// silently overwritten by an "add". Creates `mcp.json` (and its parent dir) when
/// absent.
pub fn add_server(path: &Path, spec: &McpServerSpec) -> Result<()> {
    validate_name(&spec.name)?;
    validate_command(&spec.command)?;

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
pub fn update_server(path: &Path, name: &str, spec: &McpServerSpec) -> Result<()> {
    validate_command(&spec.command)?;

    let mut root = load_value(path)?;
    {
        let servers = servers_map_mut(&mut root)?;
        let entry = servers
            .get_mut(name)
            .with_context(|| format!("MCP server \"{name}\" not found"))?;
        let obj = entry
            .as_object_mut()
            .with_context(|| format!("MCP server \"{name}\" is not a JSON object"))?;
        obj.insert("command".to_string(), Value::String(spec.command.clone()));
        obj.insert("args".to_string(), args_value(&spec.args));
        obj.insert("enabled".to_string(), Value::Bool(spec.enabled));
    }
    write_atomic(path, &root)
}

/// Remove a server. Errors if it does not exist.
pub fn remove_server(path: &Path, name: &str) -> Result<()> {
    let mut root = load_value(path)?;
    {
        let servers = servers_map_mut(&mut root)?;
        if servers.remove(name).is_none() {
            bail!("MCP server \"{name}\" not found");
        }
    }
    write_atomic(path, &root)
}

/// Spawn the named server, handshake, and return its live tool count. Blocking:
/// runs the async discovery on a dedicated thread + one-shot current-thread
/// runtime so it is safe to call from a synchronous FFI context (and from within
/// an already-running runtime). `timeout` bounds the whole handshake.
pub fn test_server_blocking(path: &Path, name: &str, timeout: Duration) -> Result<usize> {
    let config = McpConfigFile::load(path)?;
    let server = config
        .servers
        .get(name)
        .with_context(|| format!("MCP server \"{name}\" not found"))?
        .clone();
    run_list_tools_blocking(server, timeout)
}

// --- internals ------------------------------------------------------------

fn run_list_tools_blocking(server: McpServerConfig, timeout: Duration) -> Result<usize> {
    std::thread::spawn(move || -> Result<usize> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to create MCP test runtime")?;
        runtime.block_on(async move {
            let client = McpClient::new(server).with_timeout(timeout);
            let tools = client.list_tools().await?;
            Ok(tools.len())
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("MCP test thread panicked"))?
}

fn server_object(spec: &McpServerSpec) -> Value {
    let mut obj = Map::new();
    obj.insert("command".to_string(), Value::String(spec.command.clone()));
    obj.insert("args".to_string(), args_value(&spec.args));
    obj.insert("enabled".to_string(), Value::Bool(spec.enabled));
    Value::Object(obj)
}

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
    let content = std::fs::read_to_string(path)
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
        let mut file = std::fs::File::create(&tmp_path)
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

fn validate_command(command: &str) -> Result<()> {
    if command.trim().is_empty() {
        bail!("MCP server command is empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    fn spec(name: &str, command: &str, args: &[&str]) -> McpServerSpec {
        McpServerSpec {
            name: name.to_string(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            enabled: true,
        }
    }

    fn read_raw(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse")
    }

    #[test]
    fn add_then_list_roundtrips() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json"); // does not exist yet

        assert!(list_servers(&path).expect("list empty").is_empty());

        add_server(&path, &spec("loctree-mcp", "loctree-mcp", &["mcp"])).expect("add");
        add_server(&path, &spec("aicx-mcp", "aicx", &["mcp"])).expect("add");

        let servers = list_servers(&path).expect("list");
        // Sorted by name.
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "aicx-mcp");
        assert_eq!(servers[1].name, "loctree-mcp");
        assert_eq!(servers[1].command, "loctree-mcp");
        assert_eq!(servers[1].args, vec!["mcp".to_string()]);
        assert!(servers[1].enabled);
    }

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

        add_server(&path, &spec("added", "added-cmd", &["x"])).expect("add");

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
                    "weird": [1, 2, 3]
                }
            }
        });
        std::fs::write(&path, original.to_string()).expect("seed");

        let mut updated = spec("srv", "new-cmd", &["b", "c"]);
        updated.enabled = false;
        update_server(&path, "srv", &updated).expect("update");

        let raw = read_raw(&path);
        assert_eq!(raw["mcpServers"]["srv"]["command"], json!("new-cmd"));
        assert_eq!(raw["mcpServers"]["srv"]["args"], json!(["b", "c"]));
        assert_eq!(raw["mcpServers"]["srv"]["enabled"], json!(false));
        // Preserved untouched.
        assert_eq!(raw["mcpServers"]["srv"]["env"]["TOKEN"], json!("keepme"));
        assert_eq!(raw["mcpServers"]["srv"]["timeout_seconds"], json!(42));
        assert_eq!(raw["mcpServers"]["srv"]["weird"], json!([1, 2, 3]));
    }

    #[test]
    fn remove_deletes_only_the_named_server() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server(&path, &spec("a", "a", &[])).expect("add");
        add_server(&path, &spec("b", "b", &[])).expect("add");

        remove_server(&path, "a").expect("remove");
        let names: Vec<String> = list_servers(&path)
            .expect("list")
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["b".to_string()]);

        // Removing a missing server errors.
        assert!(remove_server(&path, "ghost").is_err());
    }

    #[test]
    fn invalid_json_is_never_clobbered() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        std::fs::write(&path, "{ not valid json").expect("seed garbage");

        // Every mutation refuses to run and leaves the file byte-for-byte intact.
        assert!(add_server(&path, &spec("x", "x", &[])).is_err());
        assert!(remove_server(&path, "x").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("still there"),
            "{ not valid json"
        );
    }

    #[test]
    fn rejects_invalid_names_and_empty_command() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");

        assert!(add_server(&path, &spec("bad name", "cmd", &[])).is_err());
        assert!(add_server(&path, &spec("", "cmd", &[])).is_err());
        assert!(add_server(&path, &spec("ok", "   ", &[])).is_err());
        // Nothing was written by the rejected adds.
        assert!(!path.exists());
    }

    #[test]
    fn duplicate_add_is_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server(&path, &spec("dup", "cmd", &[])).expect("add");
        assert!(add_server(&path, &spec("dup", "other", &[])).is_err());
        // Original command survives the rejected duplicate.
        assert_eq!(
            list_servers(&path).expect("list")[0].command,
            "cmd".to_string()
        );
    }

    #[test]
    fn written_config_is_valid_and_reparses_through_typed_loader() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server(&path, &spec("srv", "cmd", &["one", "two"])).expect("add");
        // Atomic write must leave a clean file the typed loader accepts.
        let config = McpConfigFile::load(&path).expect("typed reload");
        let server = config.servers.get("srv").expect("server present");
        assert_eq!(server.command, "cmd");
        assert_eq!(server.args, vec!["one".to_string(), "two".to_string()]);
    }

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
        add_server(&path, &server).expect("add");

        let count = test_server_blocking(&path, "mock", Duration::from_secs(5)).expect("test");
        assert_eq!(count, 1, "mock server exposes one tool");
    }

    #[test]
    fn test_server_blocking_errors_for_missing_server() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("mcp.json");
        add_server(&path, &spec("present", "python3", &[])).expect("add");
        assert!(test_server_blocking(&path, "absent", Duration::from_secs(1)).is_err());
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("core manifest has a repo parent")
            .to_path_buf()
    }
}
