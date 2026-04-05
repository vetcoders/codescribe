use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn e2e_env_example_matches_registry() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let tmp_env =
        std::env::temp_dir().join(format!("codescribe_env_e2e_{}.env", std::process::id()));

    let status = Command::new("bash")
        .arg("scripts/validate-envs.sh")
        .arg("--env-example")
        .arg("--env-example-path")
        .arg(".env.example")
        .arg("--emit-e2e-env")
        .arg(&tmp_env)
        .current_dir(manifest_dir)
        .status()
        .expect("failed to run validate-envs.sh");

    if tmp_env.exists() {
        let _ = fs::remove_file(&tmp_env);
    }

    assert!(status.success(), "validate-envs.sh failed");
}

#[test]
fn env_registry_fix_mode_emits_actionable_stub_text() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let temp = tempdir().expect("tempdir");
    let root = temp.path();

    fs::create_dir_all(root.join("core")).expect("create core dir");
    fs::create_dir_all(root.join("app")).expect("create app dir");
    fs::create_dir_all(root.join("bin")).expect("create bin dir");
    fs::create_dir_all(root.join("docs")).expect("create docs dir");

    fs::write(
        root.join("core/probe.rs"),
        "use std::env;\nfn probe() { let _ = env::var(\"MISSING_FEATURE_FLAG\"); }\n",
    )
    .expect("write probe");
    fs::write(
        root.join("docs/ENV_REGISTRY.toml"),
        "[meta]\nversion = \"1.0.0\"\nupdated = \"2026-04-05\"\n",
    )
    .expect("write registry");

    let output = Command::new("bash")
        .arg(format!("{manifest_dir}/scripts/validate-envs.sh"))
        .arg("--fix")
        .current_dir(root)
        .output()
        .expect("run validate-envs.sh --fix");

    assert!(
        !output.status.success(),
        "missing env var should make validate-envs.sh fail in fix mode"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[vars.MISSING_FEATURE_FLAG]"));
    assert!(stdout.contains("Document what MISSING_FEATURE_FLAG controls"));
    assert!(!stdout.contains("TODO: Add description"));
}
