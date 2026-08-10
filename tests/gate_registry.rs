//! The gate ledger is enforced from inside the test suite on purpose.
//!
//! CI never invokes a Makefile quality target — `.github/workflows/rust.yml`
//! runs `make verify`, and everything else it does it does with cargo directly.
//! So a validator wired only into `make check` would be a classification of
//! what CI runs that CI itself never checks. Running it here puts it inside
//! `cargo test --workspace`, which is what `make verify` executes.
//!
//! Same shape as `e2e_env_registry.rs`, which does this for `docs/ENV_REGISTRY.toml`.

use std::process::Command;

#[test]
fn gate_ledger_matches_makefile_and_ci() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    let output = Command::new("bash")
        .arg("scripts/validate-gates.sh")
        .current_dir(manifest_dir)
        .output()
        .expect("failed to run validate-gates.sh");

    assert!(
        output.status.success(),
        "validate-gates.sh failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
