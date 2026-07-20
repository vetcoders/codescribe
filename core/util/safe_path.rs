//! Safe path utilities with canonicalization and boundary validation.
//!
//! Provides path validation to prevent path traversal attacks.
//! All paths are canonicalized (resolving symlinks and `..`) and optionally
//! bounded to an allowed root directory.
//!
//! # Security Model
//!
//! Codescribe is a desktop app, not a web server. Paths come from:
//! - Local audio recordings (temp files created by app - trusted)
//! - CLI arguments (user's own files on their system - trusted)
//! - Config files (user's own config - trusted)
//!
//! However, we still apply defense-in-depth by canonicalizing all paths
//! before use, which eliminates symlink attacks and `..` traversal.

use anyhow::{Context, Result};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Canonicalize a path, resolving symlinks and relative components.
///
/// Returns the canonical absolute path, or an error if the path doesn't exist
/// or cannot be resolved.
///
/// # Example
/// ```ignore
/// let safe = safe_canonicalize(Path::new("../../../etc/passwd"))?;
/// // Returns error or resolved path within filesystem
/// ```
pub fn safe_canonicalize(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {}", path.display()))
}

/// Canonicalize a path and verify it's within an allowed root directory.
///
/// This prevents path traversal attacks where `../..` or symlinks could
/// escape the intended directory.
///
/// # Arguments
/// * `path` - The path to validate
/// * `root` - The allowed root directory (will also be canonicalized)
///
/// # Returns
/// The canonical path if it's within the root, or an error if:
/// - The path doesn't exist
/// - The path resolves outside the root directory
///
/// # Example
/// ```ignore
/// let root = Path::new("/app/data");
/// let safe = safe_canonicalize_bounded(Path::new("../../../etc/passwd"), root)?;
/// // Returns Err - path escapes root
/// ```
pub fn safe_canonicalize_bounded(path: &Path, root: &Path) -> Result<PathBuf> {
    let root_canon = root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize root directory: {}", root.display()))?;

    let path_canon = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {}", path.display()))?;

    if !path_canon.starts_with(&root_canon) {
        anyhow::bail!(
            "Path traversal detected: {} is outside allowed root {}",
            path_canon.display(),
            root_canon.display()
        );
    }

    Ok(path_canon)
}

/// Open a file after canonicalizing the path.
///
/// This is a safe wrapper around `std::fs::File::open` that first
/// canonicalizes the path to resolve symlinks and relative components.
///
/// # Security Note
/// For desktop apps where paths come from trusted sources (user CLI args,
/// app-created temp files), canonicalization provides defense-in-depth
/// without being strictly necessary.
pub fn safe_open(path: &Path) -> Result<std::fs::File> {
    let canonical = safe_canonicalize(path)?;
    let parent = canonical
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Path has no parent: {}", canonical.display()))?;
    let root_canon = safe_canonicalize(parent)?;
    let relative = relative_existing_path(&canonical, &root_canon)?;
    let dir = open_root_dir(&root_canon)?;
    let file = dir
        .open(&relative)
        .with_context(|| format!("Failed to open file: {}", canonical.display()))?;
    Ok(file.into_std())
}

/// Open an existing file through a capability rooted at `root`.
///
/// Both paths are canonicalized before the relative path is derived, so a
/// symlink that escapes the root is rejected before `cap_std` opens anything.
pub fn safe_open_bounded(path: &Path, root: &Path) -> Result<std::fs::File> {
    let root_canon = safe_canonicalize(root)?;
    let relative = relative_existing_path(path, &root_canon)?;
    let dir = open_root_dir(&root_canon)?;
    let file = dir
        .open(&relative)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;
    Ok(file.into_std())
}

/// Read a file to string after canonicalizing the path.
///
/// Safe wrapper around `std::fs::read_to_string`.
pub fn safe_read_to_string(path: &Path) -> Result<String> {
    let canonical = safe_canonicalize(path)?;
    let parent = canonical
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Path has no parent: {}", canonical.display()))?;
    let root_canon = safe_canonicalize(parent)?;
    let relative = relative_existing_path(&canonical, &root_canon)?;
    let dir = open_root_dir(&root_canon)?;
    dir.read_to_string(&relative)
        .with_context(|| format!("Failed to read file: {}", canonical.display()))
}

/// Read a file to string after canonicalizing and enforcing a root boundary.
pub fn safe_read_to_string_bounded(path: &Path, root: &Path) -> Result<String> {
    let root_canon = safe_canonicalize(root)?;
    let relative = relative_existing_path(path, &root_canon)?;
    let dir = open_root_dir(&root_canon)?;
    dir.read_to_string(&relative)
        .with_context(|| format!("Failed to read file: {}", path.display()))
}

/// Prepare a path for writing by ensuring it stays within a root boundary.
///
/// For relative paths, the root is used as the base.
/// For absolute paths, the path must be within the root.
pub fn safe_prepare_path(path: &Path, root: &Path) -> Result<PathBuf> {
    let root_canon = safe_canonicalize(root)?;
    safe_prepare_path_with_root(path, &root_canon)
}

/// Write a file after validating it stays within a root boundary.
pub fn safe_write_bounded(path: &Path, root: &Path, contents: &str) -> Result<()> {
    let root_canon = safe_canonicalize(root)?;
    let relative = relative_prepared_path(path, &root_canon)?;
    let dir = open_root_dir(&root_canon)?;
    if let Some(parent) = relative.parent() {
        dir.create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = dir
        .open_with(&relative, &options)
        .with_context(|| format!("Failed to open file: {}", relative.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("Failed to write file: {}", relative.display()))
}

/// Append a line to a file after validating it stays within a root boundary.
pub fn safe_append_line_bounded(path: &Path, root: &Path, line: &str) -> Result<()> {
    let root_canon = safe_canonicalize(root)?;
    let relative = relative_prepared_path(path, &root_canon)?;
    let dir = open_root_dir(&root_canon)?;
    if let Some(parent) = relative.parent() {
        dir.create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    let mut file = dir
        .open_with(&relative, &options)
        .with_context(|| format!("Failed to open file: {}", relative.display()))?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Copy a file after validating source and destination bounds.
pub fn safe_copy_bounded(
    src: &Path,
    src_root: &Path,
    dest: &Path,
    dest_root: &Path,
) -> Result<u64> {
    let src_root_canon = safe_canonicalize(src_root)?;
    let dest_root_canon = safe_canonicalize(dest_root)?;
    let src_relative = relative_existing_path(src, &src_root_canon)?;
    let dest_relative = relative_prepared_path(dest, &dest_root_canon)?;
    let src_dir = open_root_dir(&src_root_canon)?;
    let dest_dir = open_root_dir(&dest_root_canon)?;
    if let Some(parent) = dest_relative.parent() {
        dest_dir
            .create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    src_dir
        .copy(&src_relative, &dest_dir, &dest_relative)
        .with_context(|| format!("Failed to copy to {}", dest.display()))
}

/// Create a symlink within bounds or fall back to copy if unsupported.
#[cfg(target_family = "unix")]
pub fn safe_symlink_or_copy_bounded(
    src: &Path,
    src_root: &Path,
    dest: &Path,
    dest_root: &Path,
) -> Result<()> {
    let src_root_canon = safe_canonicalize(src_root)?;
    let dest_root_canon = safe_canonicalize(dest_root)?;
    let src_relative = relative_existing_path(src, &src_root_canon)?;
    let dest_relative = relative_prepared_path(dest, &dest_root_canon)?;
    let src_dir = open_root_dir(&src_root_canon)?;
    let dest_dir = open_root_dir(&dest_root_canon)?;
    if let Some(parent) = dest_relative.parent() {
        dest_dir
            .create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if src_dir
        .hard_link(&src_relative, &dest_dir, &dest_relative)
        .is_err()
    {
        src_dir
            .copy(&src_relative, &dest_dir, &dest_relative)
            .with_context(|| format!("Failed to copy to {}", dest.display()))?;
    }
    Ok(())
}

fn safe_prepare_path_with_root(path: &Path, root_canon: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root_canon.join(path)
    };
    let normalized = normalize_path(&candidate);
    if !normalized.starts_with(root_canon) {
        anyhow::bail!(
            "Path traversal detected: {} is outside allowed root {}",
            normalized.display(),
            root_canon.display()
        );
    }

    if let Some(parent) = normalized.parent()
        && parent.exists()
    {
        let parent_canon = safe_canonicalize(parent)?;
        if !parent_canon.starts_with(root_canon) {
            anyhow::bail!(
                "Path traversal detected: {} is outside allowed root {}",
                parent_canon.display(),
                root_canon.display()
            );
        }
    }

    Ok(normalized)
}

fn relative_existing_path(path: &Path, root_canon: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {}", path.display()))?;
    if !canonical.starts_with(root_canon) {
        anyhow::bail!(
            "Path traversal detected: {} is outside allowed root {}",
            canonical.display(),
            root_canon.display()
        );
    }
    let relative = canonical.strip_prefix(root_canon).unwrap_or(Path::new(""));
    if relative.as_os_str().is_empty() {
        anyhow::bail!("Refusing to operate on root path: {}", root_canon.display());
    }
    Ok(relative.to_path_buf())
}

fn relative_prepared_path(path: &Path, root_canon: &Path) -> Result<PathBuf> {
    let prepared = safe_prepare_path_with_root(path, root_canon)?;
    let relative = prepared.strip_prefix(root_canon).unwrap_or(Path::new(""));
    if relative.as_os_str().is_empty() {
        anyhow::bail!("Refusing to operate on root path: {}", root_canon.display());
    }
    Ok(relative.to_path_buf())
}

fn open_root_dir(root_canon: &Path) -> Result<Dir> {
    Dir::open_ambient_dir(root_canon, ambient_authority())
        .with_context(|| format!("Failed to open root dir: {}", root_canon.display()))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.as_os_str().is_empty() {
                    continue;
                }
                out.pop();
            }
            Component::Normal(segment) => out.push(segment),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_canonicalize_existing_path() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "test").unwrap();

        let result = safe_canonicalize(&file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_canonicalize_nonexistent_path() {
        let result = safe_canonicalize(Path::new("/nonexistent/path/file.txt"));
        assert!(result.is_err());
    }

    #[test]
    fn test_bounded_canonicalize_within_root() {
        let dir = tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        let file_path = subdir.join("test.txt");
        fs::write(&file_path, "test").unwrap();

        let result = safe_canonicalize_bounded(&file_path, dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_bounded_canonicalize_outside_root() {
        let dir = tempdir().unwrap();
        let other_dir = tempdir().unwrap();
        let file_path = other_dir.path().join("test.txt");
        fs::write(&file_path, "test").unwrap();

        let result = safe_canonicalize_bounded(&file_path, dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("outside allowed root")
        );
    }

    #[test]
    fn test_safe_open() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello").unwrap();

        let result = safe_open(&file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_safe_read_to_string() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let result = safe_read_to_string(&file_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello world");
    }
}
