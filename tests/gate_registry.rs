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

#[test]
fn localization_gates_remain_required_in_ci() {
    let ledger = include_str!("../Makefile");
    let workflow = include_str!("../.github/workflows/rust.yml");
    let quality = workflow
        .split_once("\n  quality:\n")
        .expect("localization must run in the existing required quality job")
        .1
        .lines()
        .take_while(|line| line.trim().is_empty() || line.starts_with("    "))
        .map(str::trim)
        .collect::<Vec<_>>();

    assert!(quality.contains(&"name: Clippy + Tests"));
    for target in [
        "verify-l10n-catalog",
        "verify-l10n-bridge",
        "verify-l10n-sync",
        "test-l10n-sync",
    ] {
        let prefix = format!("# gate: {target} ");
        let row = ledger
            .lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("missing localization ledger row: {target}"));
        assert!(
            row.split(" -- ")
                .next()
                .unwrap()
                .split_whitespace()
                .any(|field| field == "ci=yes"),
            "localization gate must not be downgraded to local-only: {target}"
        );
        let invocation = format!("run: make {target}");
        assert!(
            quality.contains(&invocation.as_str()),
            "required quality job must invoke {target} directly"
        );
    }

    assert!(quality.contains(&"make l10n-build"));
    assert!(
        quality
            .iter()
            .any(|line| line.contains("L10N_DERIVED=$(mktemp -d"))
    );
    assert!(quality.contains(&"git diff --exit-code -- macos/Codescribe/Resources/Localization"));
    for line in &quality {
        assert!(
            !line.starts_with("if:"),
            "localization must not be conditional"
        );
        assert!(
            !line.starts_with("continue-on-error:"),
            "localization failures must fail the required job"
        );
        assert!(
            !line.contains("--allow-partial"),
            "all bundled languages must be complete"
        );
        assert!(
            !line.starts_with("make l10n-sync"),
            "CI must not repair the catalog"
        );
    }
    for line in workflow.lines().map(str::trim) {
        assert!(
            !line.starts_with("paths:") && !line.starts_with("paths-ignore:"),
            "localization checks must run on every PR"
        );
    }
}
