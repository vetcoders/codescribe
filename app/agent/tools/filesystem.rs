use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use serde_json::{Value, json};

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
    let path_str = input
        .get("path")
        .and_then(Value::as_str)
        .context("Missing required string field 'path'")?;

    let path = validate_path_for_read(path_str)?;
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Path is canonicalized and restricted to $HOME or /tmp in validate_path_for_read().
    let mut content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read UTF-8 text from {}", path.display()))?;

    if content.chars().count() > MAX_TEXT_CHARS {
        content = content.chars().take(MAX_TEXT_CHARS).collect();
    }

    Ok(content)
}

fn validate_path_for_read(path_str: &str) -> Result<PathBuf> {
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

    ensure_allowed_path(&canonical)?;

    let metadata = fs::metadata(&canonical)
        .with_context(|| format!("Failed to inspect file metadata: {}", canonical.display()))?;
    if metadata.len() > MAX_FILE_SIZE_BYTES {
        bail!(
            "File exceeds size limit ({} bytes): {}",
            MAX_FILE_SIZE_BYTES,
            canonical.display()
        );
    }

    Ok(canonical)
}

fn ensure_allowed_path(path: &Path) -> Result<()> {
    let home_var = std::env::var("HOME").context("HOME environment variable is not set")?;
    let home_path = canonical_or_original(PathBuf::from(home_var));
    let tmp_path = canonical_or_original(PathBuf::from("/tmp"));

    if is_path_allowed(path, &home_path, &tmp_path) {
        return Ok(());
    }

    bail!(
        "Path is outside allowed directories ($HOME or /tmp): {}",
        path.display()
    )
}

fn canonical_or_original(path: PathBuf) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path,
    }
}

pub(crate) fn is_path_allowed(path: &Path, home: &Path, tmp: &Path) -> bool {
    path.starts_with(home) || path.starts_with(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_under_home_is_allowed() {
        let home = PathBuf::from("/Users/tester");
        let tmp = PathBuf::from("/private/tmp");
        let path = PathBuf::from("/Users/tester/Documents/note.txt");

        assert!(is_path_allowed(&path, &home, &tmp));
    }

    #[test]
    fn path_under_tmp_is_allowed() {
        let home = PathBuf::from("/Users/tester");
        let tmp = PathBuf::from("/private/tmp");
        let path = PathBuf::from("/private/tmp/codescribe-test.txt");

        assert!(is_path_allowed(&path, &home, &tmp));
    }

    #[test]
    fn path_outside_allowed_roots_is_rejected() {
        let home = PathBuf::from("/Users/tester");
        let tmp = PathBuf::from("/private/tmp");
        let path = PathBuf::from("/etc/hosts");

        assert!(!is_path_allowed(&path, &home, &tmp));
    }
}
