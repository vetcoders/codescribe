use std::fs;
use std::process::Command;

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
