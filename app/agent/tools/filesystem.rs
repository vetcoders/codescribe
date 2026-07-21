use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use codescribe_core::config::Config;
use codescribe_core::util::safe_path::safe_open_bounded;
use serde_json::{Value, json};

use super::workspace;

const MAX_FILE_SIZE_BYTES: u64 = 512 * 1024;
const MAX_TEXT_CHARS: usize = 40_000;

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            read_file_definition(),
            Box::new(|input| Box::pin(handle_read_file(input))),
        )
        .expect("register read_file tool");
}

fn read_file_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description:
            "Read the text content of a file. Only works for UTF-8 text files under 40K characters."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                }
            },
            "required": ["path"]
        }),
    }
}

async fn handle_read_file(input: Value) -> Vec<ToolResultContent> {
    match read_file_from_input(&input) {
        Ok(content) => vec![ToolResultContent::Text(content)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn read_file_from_input(input: &Value) -> Result<String> {
    let roots = allowed_read_roots();
    read_file_from_input_with_roots(input, &roots)
}

fn read_file_from_input_with_roots(input: &Value, roots: &[PathBuf]) -> Result<String> {
    let path_str = input
        .get("path")
        .and_then(Value::as_str)
        .context("Missing required string field 'path'")?;

    let (path, root) = validate_path_for_read_with_roots(path_str, roots)?;
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- safe_open_bounded canonicalizes the path and opens it through a cap_std root capability.
    let file = safe_open_bounded(&path, &root)
        .with_context(|| format!("Failed to open allowed file {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("Failed to inspect file metadata: {}", path.display()))?;
    if metadata.len() > MAX_FILE_SIZE_BYTES {
        bail!(
            "File exceeds size limit ({} bytes): {}",
            MAX_FILE_SIZE_BYTES,
            path.display()
        );
    }

    // Bound the actual capability-backed read as well as the metadata check so
    // a concurrent file growth cannot force an unbounded allocation.
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_FILE_SIZE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_SIZE_BYTES {
        bail!(
            "File exceeds size limit ({} bytes): {}",
            MAX_FILE_SIZE_BYTES,
            path.display()
        );
    }
    let mut content = String::from_utf8(bytes)
        .with_context(|| format!("Failed to read UTF-8 text from {}", path.display()))?;

    if content.chars().count() > MAX_TEXT_CHARS {
        content = content.chars().take(MAX_TEXT_CHARS).collect();
    }

    Ok(content)
}

fn validate_path_for_read_with_roots(
    path_str: &str,
    roots: &[PathBuf],
) -> Result<(PathBuf, PathBuf)> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Input path is validated below (absolute, canonicalized, file-only, root-restricted).
    let path = PathBuf::from(path_str);
    if !path.is_absolute() {
        bail!("Path must be absolute: {path_str}");
    }

    if !path.exists() {
        bail!("Path does not exist: {path_str}");
    }

    if !path.is_file() {
        bail!("Path is not a file: {path_str}");
    }

    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {path_str}"))?;

    let roots = canonical_allowed_roots(roots);
    let Some(root) = roots.into_iter().find(|root| canonical.starts_with(root)) else {
        bail!(
            "Path is outside configured workspace roots and Codescribe storage: {}",
            canonical.display()
        );
    };

    Ok((canonical, root))
}

fn allowed_read_roots() -> Vec<PathBuf> {
    let mut roots = workspace::resolved_roots();
    // Codescribe-owned storage is the only deliberate non-workspace exception.
    roots.push(Config::config_dir());
    canonical_allowed_roots(&roots)
}

fn canonical_allowed_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .filter(|root| root.is_dir())
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

#[cfg(test)]
pub(crate) fn is_path_allowed(path: &std::path::Path, roots: &[PathBuf]) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    canonical_allowed_roots(roots)
        .iter()
        .any(|root| canonical.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::config::UserSettings;
    use serial_test::serial;
    use std::env;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn sandbox_derives_roots_from_fresh_settings() {
        let _env_serial = crate::test_env::data_dir_env_serial();
        let tmp = TempDir::new().expect("tempdir");
        let data = tmp.path().join("data");
        let root_a = tmp.path().join("workspace-a");
        let root_b = tmp.path().join("workspace-b");
        let outside = tmp.path().join("home/Documents");
        for dir in [&data, &root_a, &root_b, &outside] {
            fs::create_dir_all(dir).expect("create test directory");
        }

        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &data);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _process_roots = EnvGuard::remove("AGENT_WORKSPACE_ROOTS");
        UserSettings {
            agent_workspace_roots: Some(vec![
                root_a.display().to_string(),
                root_b.display().to_string(),
            ]),
            ..Default::default()
        }
        .save()
        .expect("persist configured roots");

        let inside_a = root_a.join("a.txt");
        let inside_b = root_b.join("b.txt");
        let own_storage = data.join("thread.json");
        let outside_file = outside.join("private.txt");
        fs::write(&inside_a, "alpha").expect("write root-a file");
        fs::write(&inside_b, "beta").expect("write root-b file");
        fs::write(&own_storage, "thread").expect("write own-storage file");
        fs::write(&outside_file, "outside").expect("write outside file");

        assert_eq!(
            read_file_from_input(&json!({ "path": inside_a }))
                .expect("read first persisted workspace root"),
            "alpha"
        );
        assert_eq!(
            read_file_from_input(&json!({ "path": inside_b }))
                .expect("read second persisted workspace root"),
            "beta"
        );
        assert_eq!(
            read_file_from_input(&json!({ "path": own_storage }))
                .expect("read Codescribe-owned storage"),
            "thread"
        );
        let error = read_file_from_input(&json!({ "path": outside_file }))
            .expect_err("path outside every persisted root must be denied");
        assert!(
            error
                .to_string()
                .contains("outside configured workspace roots"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn sandbox_allows_only_workspace_roots_and_codescribe_storage() {
        let tmp = TempDir::new().expect("tempdir");
        let root_a = tmp.path().join("workspace-a");
        let root_b = tmp.path().join("workspace-b");
        let storage = tmp.path().join(".codescribe");
        let home_outside = tmp.path().join("home/Documents");
        for dir in [&root_a, &root_b, &storage, &home_outside] {
            fs::create_dir_all(dir).expect("create sandbox directory");
        }
        let inside_a = root_a.join("a.txt");
        let inside_b = root_b.join("b.txt");
        let own_storage = storage.join("thread.json");
        let outside = home_outside.join("private.txt");
        fs::write(&inside_a, "alpha").expect("write root-a file");
        fs::write(&inside_b, "beta").expect("write root-b file");
        fs::write(&own_storage, "thread").expect("write own-storage file");
        fs::write(&outside, "outside").expect("write outside file");
        let allowed = vec![root_a.clone(), root_b.clone(), storage.clone()];

        assert_eq!(
            read_file_from_input_with_roots(&json!({ "path": inside_a }), &allowed)
                .expect("read first configured root"),
            "alpha"
        );
        assert_eq!(
            read_file_from_input_with_roots(&json!({ "path": inside_b }), &allowed)
                .expect("read second configured root"),
            "beta"
        );
        assert_eq!(
            read_file_from_input_with_roots(&json!({ "path": own_storage }), &allowed)
                .expect("read Codescribe-owned storage"),
            "thread"
        );
        let error = read_file_from_input_with_roots(&json!({ "path": outside }), &allowed)
            .expect_err("a HOME-like path outside every configured root must be denied");
        assert!(
            error
                .to_string()
                .contains("outside configured workspace roots"),
            "unexpected error: {error}"
        );
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn sandbox_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().expect("tempdir");
        let allowed = tmp.path().join("workspace");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&allowed).expect("create allowed root");
        fs::create_dir_all(&outside).expect("create outside root");
        let secret = outside.join("secret.txt");
        fs::write(&secret, "secret").expect("write outside secret");
        let escape = allowed.join("escape.txt");
        symlink(&secret, &escape).expect("create escaping symlink");

        let error = read_file_from_input_with_roots(
            &json!({ "path": escape }),
            std::slice::from_ref(&allowed),
        )
        .expect_err("canonicalized symlink target outside root must be denied");
        assert!(
            error
                .to_string()
                .contains("outside configured workspace roots"),
            "unexpected error: {error}"
        );
        assert!(!is_path_allowed(&escape, &[allowed]));
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = env::var_os(key);
            // SAFETY: the test mutating process env is serialized.
            unsafe { env::set_var(key, value) };
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = env::var_os(key);
            // SAFETY: the test mutating process env is serialized.
            unsafe { env::remove_var(key) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: the test mutating process env is serialized.
            unsafe {
                match self.previous.as_ref() {
                    Some(value) => env::set_var(self.key, value),
                    None => env::remove_var(self.key),
                }
            }
        }
    }
}
