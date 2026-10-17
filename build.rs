//! Stamp build identity into the binary.
//!
//! The About dialog and log telemetry need to name the exact build they came
//! from, which is not knowable at runtime — so the commit hash and rustc
//! version are captured here and baked in as compile-time env vars.

use std::process::Command;

/// Emit `CODESCRIBE_BUILD_COMMIT` and `CODESCRIBE_RUSTC_VERSION`.
///
/// Both degrade to `"unknown"` rather than failing the build: a source tarball
/// with no `.git` still has to compile. Re-runs follow HEAD and the branch ref
/// it names, so a commit restamps the build while ordinary edits do not force
/// a rebuild of the whole crate.
fn main() {
    println!("cargo:rerun-if-env-changed=CODESCRIBE_DEVELOPER_SURFACE");
    let developer = std::env::var("CODESCRIBE_DEVELOPER_SURFACE").is_ok_and(|value| value == "1");
    println!(
        "cargo:rustc-env=CODESCRIBE_DEVELOPER_SURFACE={}",
        u8::from(developer)
    );
    // Git commit hash, 9 chars like the app bundle's CSBuildCommit, so
    // `codescribe replay` can compare it with the build that recorded a take.
    let commit = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CODESCRIBE_BUILD_COMMIT={}", commit);

    // Rustc version
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CODESCRIBE_RUSTC_VERSION={}", rustc);

    // Re-run when HEAD moves: a checkout rewrites HEAD, a commit rewrites the
    // branch ref, and both append to this worktree's HEAD reflog (which also
    // covers a branch whose ref is packed). `--git-path` resolves each inside a
    // linked worktree, where `.git` is a file. Only existing paths are watched:
    // Cargo treats a missing one as changed and would rerun on every build.
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    let mut watched = vec!["HEAD".to_string(), "logs/HEAD".to_string()];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    let watched: Vec<String> = watched
        .iter()
        .filter_map(|name| git(&["rev-parse", "--git-path", name]))
        .filter(|path| std::path::Path::new(path).exists())
        .collect();
    if watched.is_empty() {
        println!("cargo:rerun-if-changed=build.rs");
    }
    for path in watched {
        println!("cargo:rerun-if-changed={path}");
    }
}
