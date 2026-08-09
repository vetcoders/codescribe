//! Native filesystem tool (canonical: `read_file`).
//!
//! Reading is sandboxed twice over: the path must canonicalize into one of the
//! configured workspace roots (plus Codescribe's own storage directory), and the
//! open itself goes through a `cap_std` root capability so a symlink cannot walk
//! out. Size is bounded on both the metadata check and the read, so a file that
//! grows mid-read cannot force an unbounded allocation.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use codescribe_core::util::safe_path::safe_open_bounded;
use serde_json::{Value, json};

use super::{output_guard, path_policy, workspace};

/// Hard ceiling on a single `read_file` call (512 KiB).
const MAX_FILE_SIZE_BYTES: u64 = 512 * 1024;

/// Register the read-only `read_file` tool on the shared [`ToolRegistry`].
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            read_file_definition(),
            Box::new(|input| Box::pin(handle_read_file(input))),
            ToolRisk::ReadOnly,
        )
        .expect("register read_file tool");
}

/// Tool schema for `read_file`.
fn read_file_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Read the text content of a UTF-8 file. Long files are returned as the \
             first ~25K characters plus a pointer to the source path for reading the rest."
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

/// Dispatch adapter for `read_file`: turns a [`Result`] into tool content.
async fn handle_read_file(input: Value) -> Vec<ToolResultContent> {
    match read_file_from_input(&input) {
        Ok(content) => vec![ToolResultContent::Text(content)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Read a file using the roots resolved from live settings.
fn read_file_from_input(input: &Value) -> Result<String> {
    let roots = allowed_read_roots();
    read_file_from_input_with_roots(input, &roots)
}

/// Read a file against an explicit root set — the injectable core of the tool.
///
/// Enforces, in order: required `path` field, root containment, the size ceiling
/// on metadata, the same ceiling on the bounded read, UTF-8 decoding, and finally
/// a truncation that always carries a pointer back to the on-disk source.
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

    // Never a silent cut: an oversized file comes back truncated WITH a pointer
    // to the on-disk source, so the agent knows the text continues and where.
    content = output_guard::truncate_with_source(&content, &path);

    Ok(content)
}

/// Prove `path_str` is an absolute, existing, regular file inside one of `roots`.
///
/// Returns the canonicalized path together with the root that admitted it, so the
/// caller can open through that root's capability.
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

    let roots = path_policy::canonical_roots(roots);
    let Some(root) = roots.into_iter().find(|root| canonical.starts_with(root)) else {
        bail!(
            "Path is outside configured workspace roots and Codescribe storage: {}",
            canonical.display()
        );
    };

    Ok((canonical, root))
}

/// Configured workspace roots plus Codescribe's own config directory.
fn allowed_read_roots() -> Vec<PathBuf> {
    let mut roots = workspace::resolved_roots();
    // Codescribe-owned storage is the only deliberate non-workspace exception.
    roots.push(codescribe_core::config::Config::config_dir());
    path_policy::canonical_roots(&roots)
}

/// Test-only predicate: does `path` canonicalize inside any of `roots`?
#[cfg(test)]
pub(crate) fn is_path_allowed(path: &std::path::Path, roots: &[PathBuf]) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    path_policy::canonical_roots(roots)
        .iter()
        .any(|root| canonical.starts_with(root))
}

/// Sandbox proofs: root derivation from settings, root containment, and the
/// symlink-escape denial.
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

    /// Scoped environment override that restores the prior value on drop.
    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        /// Set `key` to `value` for the guard's lifetime.
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = env::var_os(key);
            // SAFETY: the test mutating process env is serialized.
            unsafe { env::set_var(key, value) };
            Self { key, previous }
        }

        /// Unset `key` for the guard's lifetime.
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
