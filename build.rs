//! Stamp build identity into the binary.
//!
//! The About dialog and log telemetry need to name the exact build they came
//! from, which is not knowable at runtime — so the commit hash and rustc
//! version are captured here and baked in as compile-time env vars.

use std::process::Command;

/// Emit `CODESCRIBE_BUILD_COMMIT` and `CODESCRIBE_RUSTC_VERSION`.
///
/// Both degrade to `"unknown"` rather than failing the build: a source tarball
/// with no `.git` still has to compile. Re-runs are pinned to `.git/HEAD`, so
/// ordinary edits do not force a rebuild of the whole crate.
fn main() {
    // Git commit hash (8 chars — build identity for the About dialog + log telemetry)
    let commit = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
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

    // Only re-run if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
}
