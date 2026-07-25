use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use codescribe_core::config::Config;

pub fn workspace_roots() -> Vec<PathBuf> {
    canonical_roots(
        &Config::effective_agent_workspace_roots()
            .into_iter()
            .map(expand_tilde)
            .collect::<Vec<_>>(),
    )
}

pub fn canonical_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .filter(|root| root.is_dir())
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

pub fn validate_existing(path: &str, roots: &[PathBuf]) -> Result<PathBuf> {
    let path = absolute(path)?;
    if !path.exists() {
        bail!("Path does not exist: {path}", path = path.display());
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {}", path.display()))?;
    ensure_within_roots(&canonical, roots)?;
    Ok(canonical)
}

pub fn validate_new_target(path: &str, roots: &[PathBuf]) -> Result<PathBuf> {
    let path = absolute(path)?;
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        bail!(
            "New target paths may not contain '.' or '..' components: {}",
            path.display()
        );
    }
    if path.exists() {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize path: {}", path.display()))?;
        ensure_within_roots(&canonical, roots)?;
        return Ok(canonical);
    }

    let mut parent = path.parent();
    let mut missing = Vec::new();
    let existing_parent = loop {
        match parent {
            Some(candidate) if candidate.exists() => break candidate,
            Some(candidate) => {
                let component = candidate
                    .file_name()
                    .context("New target contains an invalid path component")?;
                missing.push(component.to_os_string());
                parent = candidate.parent();
            }
            None => bail!("No existing parent for target: {}", path.display()),
        }
    };
    let canonical_parent = existing_parent.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize target parent: {}",
            existing_parent.display()
        )
    })?;
    ensure_within_roots(&canonical_parent, roots)?;
    let mut canonical_target = canonical_parent;
    for component in missing.iter().rev() {
        canonical_target.push(component);
    }
    canonical_target.push(
        path.file_name()
            .context("New target must name a file or directory")?,
    );
    Ok(canonical_target)
}

pub fn validate_terminal(command: &str, cwd: &str, roots: &[PathBuf]) -> Result<PathBuf> {
    let cwd = validate_existing(cwd, roots)?;
    if !cwd.is_dir() {
        bail!("Terminal cwd is not a directory: {}", cwd.display());
    }
    if command.contains(['\n', '\r', '\0']) {
        bail!("Terminal command must be a single command line");
    }
    if ["$(", "${", "`"]
        .iter()
        .any(|marker| command.contains(marker))
    {
        bail!("Dynamic shell expansion is blocked by Codescribe terminal policy");
    }
    let tokens = command
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch: char| {
                    matches!(
                        ch,
                        '\'' | '"' | '`' | ';' | '&' | '|' | '$' | '(' | ')' | '<' | '>'
                    )
                })
                .to_ascii_lowercase()
        })
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let Some(program) = tokens.first().map(String::as_str) else {
        bail!("Terminal command is empty");
    };
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    const FORBIDDEN_PROGRAMS: &[&str] = &[
        "sudo",
        "su",
        "shutdown",
        "reboot",
        "halt",
        "poweroff",
        "diskutil",
        "fdisk",
        "mkfs",
        "dscl",
        "sysadminctl",
        "security",
        "profiles",
        "mount",
        "umount",
        "gpt",
        "newfs",
        "shred",
        "chown",
        "chgrp",
        "chmod",
        "launchctl",
        "nvram",
        "csrutil",
        "installer",
        "systemsetup",
        "osascript",
        "kill",
        "killall",
        "pkill",
        "eval",
    ];
    if tokens.iter().any(|token| {
        let token = Path::new(token)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(token);
        FORBIDDEN_PROGRAMS.contains(&token)
    }) {
        bail!("Command is blocked by Codescribe terminal policy: {program}");
    }
    const FORBIDDEN_TARGETS: &[&str] = &[
        "/library/keychains",
        "/login.keychain",
        "/safari/",
        "/chrome/",
        "/chromium/",
        "/firefox/",
        "alloweddirectories",
        "blockedcommands",
    ];
    let normalized = tokens.join(" ");
    if FORBIDDEN_TARGETS
        .iter()
        .any(|target| normalized.contains(target))
    {
        bail!("Command targets protected credentials, profiles, or policy settings");
    }
    if program == "dd" && tokens.iter().any(|token| token.starts_with("of=/dev/")) {
        bail!("Raw disk writes are blocked by Codescribe terminal policy");
    }
    if program == "rm"
        && tokens.iter().any(|token| {
            token == "--recursive"
                || (token.starts_with('-') && token.contains('r') && token.contains('f'))
        })
    {
        bail!("Forced recursive deletion is blocked by Codescribe terminal policy");
    }
    if program == "git"
        && (tokens.windows(2).any(|pair| pair == ["reset", "--hard"])
            || tokens.iter().any(|token| token == "clean")
                && tokens
                    .iter()
                    .any(|token| token.starts_with('-') && token.contains('f')))
    {
        bail!("Destructive Git cleanup is blocked by Codescribe terminal policy");
    }
    for token in tokens.iter().skip(1) {
        let candidate = token
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or(token);
        if absolute(candidate).is_ok()
            && validate_existing(candidate, roots)
                .or_else(|_| validate_new_target(candidate, roots))
                .is_err()
        {
            bail!("Command references a path outside configured workspace roots: {candidate}");
        }
    }
    Ok(cwd)
}

fn absolute(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        bail!("Path must be absolute: {}", path.display());
    }
    Ok(path)
}

fn ensure_within_roots(path: &Path, roots: &[PathBuf]) -> Result<()> {
    let roots = canonical_roots(roots);
    if roots.is_empty() {
        bail!("No agent workspace roots are configured");
    }
    if roots.iter().any(|root| path.starts_with(root)) {
        return Ok(());
    }
    bail!(
        "Path is outside configured agent workspace roots: {}",
        path.display()
    )
}

fn expand_tilde(path: String) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn existing_and_new_targets_stay_inside_workspace() {
        let root = TempDir::new().expect("root");
        let roots = vec![root.path().to_path_buf()];
        let existing = root.path().join("inside.txt");
        fs::write(&existing, "ok").expect("write");

        assert!(validate_existing(existing.to_str().unwrap(), &roots).is_ok());
        assert!(
            validate_new_target(root.path().join("new/child.txt").to_str().unwrap(), &roots)
                .is_ok()
        );
        assert!(
            validate_new_target(
                root.path()
                    .join("missing/../../escape.txt")
                    .to_str()
                    .unwrap(),
                &roots
            )
            .is_err()
        );
    }

    #[test]
    fn empty_roots_and_outside_paths_are_denied() {
        let root = TempDir::new().expect("root");
        let outside = TempDir::new().expect("outside");
        assert!(validate_existing(root.path().to_str().unwrap(), &[]).is_err());
        assert!(
            validate_existing(outside.path().to_str().unwrap(), &[root.path().into()]).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_denied() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().expect("root");
        let outside = TempDir::new().expect("outside");
        let link = root.path().join("escape");
        symlink(outside.path(), &link).expect("symlink");
        assert!(validate_existing(link.to_str().unwrap(), &[root.path().into()]).is_err());
    }

    #[test]
    fn terminal_blocks_privilege_and_disk_commands() {
        let root = TempDir::new().expect("root");
        let roots = vec![root.path().to_path_buf()];
        assert!(validate_terminal("printf ok", root.path().to_str().unwrap(), &roots).is_ok());
        assert!(
            validate_terminal("sudo printf nope", root.path().to_str().unwrap(), &roots).is_err()
        );
        assert!(
            validate_terminal(
                "diskutil eraseDisk APFS X disk0",
                root.path().to_str().unwrap(),
                &roots
            )
            .is_err()
        );
        assert!(
            validate_terminal("rm -rf ./cache", root.path().to_str().unwrap(), &roots).is_err()
        );
        assert!(
            validate_terminal(
                "printf '%s' \"$(sudo whoami)\"",
                root.path().to_str().unwrap(),
                &roots
            )
            .is_err()
        );
    }
}
