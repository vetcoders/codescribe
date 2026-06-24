use std::process::Command;

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
