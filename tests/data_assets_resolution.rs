//! Fixture resolution contract for the shell harness surface.
//!
//! `tests/assets/data_assets/README.md` documents ONE resolution order for the
//! private STT fixtures: `CODESCRIBE_DATA_ASSETS` → `~/.codescribe/data_assets`
//! → the gitignored in-repo drop dir. The Rust e2e tests implement it
//! (`data_assets_dir()` in `e2e_overlay_delivery_parity.rs`,
//! `e2e_full_pipeline.rs`, `tail_silence_contract.rs`) and so does
//! `scripts/bench-stt.sh`.
//!
//! The shell harness did not: `scripts/e2e-blackhole-dictation.sh` and the
//! `ENGINE_*` Makefile targets knew only the in-repo tier — the one gitignore
//! keeps empty by design — so `make test-engine-parity` died on
//! `fixture not found: tests/assets/data_assets/05_apple-live-parity.wav`
//! even on a host whose home corpus holds the fixture.
//!
//! These tests pin `scripts/lib/data-assets.sh` (the single resolver both the
//! script and the Makefile now consume). They are hermetic: fake `HOME`, fake
//! repo root, stub WAV bytes — no private audio, no audio hardware.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn resolver() -> PathBuf {
    repo_root().join("scripts/lib/data-assets.sh")
}

/// Unique scratch dir — tests run in parallel threads of one process, so a
/// shared name would race. No `tempfile` dep in this package's dev-deps.
fn scratch(tag: &str) -> PathBuf {
    let unique = format!(
        "codescribe-data-assets-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let dir = std::env::temp_dir().join(unique);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write_stub(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("create fixture dir");
    let path = dir.join(name);
    // Non-empty: the contract is "a fixture that exists", and a zero-byte file
    // would sail past `[ -f ]` while failing every downstream reader.
    fs::write(&path, b"RIFF\0\0\0\0WAVEstub").expect("write stub fixture");
    path
}

/// Run the resolver with a fully controlled environment. `HOME` is always
/// overridden so the operator's real corpus can never leak into an assertion.
fn run(args: &[&str], home: &Path, data_assets: Option<&Path>) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg(resolver());
    cmd.args(args);
    cmd.env("HOME", home);
    match data_assets {
        Some(dir) => cmd.env("CODESCRIBE_DATA_ASSETS", dir),
        None => cmd.env_remove("CODESCRIBE_DATA_ASSETS"),
    };
    cmd.output().expect("run scripts/lib/data-assets.sh")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Copy the resolver into a throwaway repo skeleton so the in-repo drop tier
/// can be exercised without writing into the live (gitignored, shared) tree.
fn fake_repo(tag: &str) -> PathBuf {
    let root = scratch(tag);
    fs::create_dir_all(root.join("scripts/lib")).expect("create fake scripts/lib");
    fs::create_dir_all(root.join("tests/assets/data_assets")).expect("create fake drop dir");
    fs::copy(resolver(), root.join("scripts/lib/data-assets.sh")).expect("copy resolver");
    root
}

#[test]
fn resolver_script_exists_and_is_executable() {
    let path = resolver();
    assert!(
        path.is_file(),
        "missing {} — the shell harness and the Makefile both resolve fixtures through it",
        path.display()
    );
}

#[test]
fn explicit_env_dir_wins_over_home_corpus() {
    let home = scratch("env-home");
    let explicit = scratch("env-explicit");
    write_stub(
        &home.join(".codescribe/data_assets"),
        "05_apple-live-parity.wav",
    );
    let wanted = write_stub(&explicit, "05_apple-live-parity.wav");

    let out = run(
        &["resolve", "05_apple-live-parity.wav"],
        &home,
        Some(&explicit),
    );
    assert!(out.status.success(), "resolve failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), wanted.to_string_lossy());
}

#[test]
fn home_corpus_is_the_fallback_when_env_is_unset() {
    let home = scratch("home-fallback");
    let wanted = write_stub(
        &home.join(".codescribe/data_assets"),
        "05_apple-live-parity.wav",
    );

    let out = run(&["resolve", "05_apple-live-parity.wav"], &home, None);
    assert!(out.status.success(), "resolve failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), wanted.to_string_lossy());
}

#[test]
fn env_dir_without_the_fixture_falls_through_to_home() {
    let home = scratch("env-miss-home");
    let explicit = scratch("env-miss-explicit");
    let wanted = write_stub(
        &home.join(".codescribe/data_assets"),
        "05_apple-live-parity.wav",
    );

    // Dir exists, fixture does not — a partial corpus must not dead-end the run.
    let out = run(
        &["resolve", "05_apple-live-parity.wav"],
        &home,
        Some(&explicit),
    );
    assert!(out.status.success(), "resolve failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), wanted.to_string_lossy());
}

#[test]
fn in_repo_drop_dir_is_the_last_tier() {
    let root = fake_repo("drop-tier");
    let home = scratch("drop-tier-home");
    let wanted = write_stub(
        &root.join("tests/assets/data_assets"),
        "05_apple-live-parity.wav",
    );

    let mut cmd = Command::new("bash");
    cmd.arg(root.join("scripts/lib/data-assets.sh"));
    cmd.args(["resolve", "05_apple-live-parity.wav"]);
    cmd.env("HOME", &home);
    cmd.env_remove("CODESCRIBE_DATA_ASSETS");
    let out = cmd.output().expect("run copied resolver");

    assert!(out.status.success(), "resolve failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), wanted.to_string_lossy());
}

#[test]
fn repo_relative_path_resolves_by_basename() {
    // The exact call shape the Makefile and the harness use: a repo-relative
    // path that does not exist on this checkout, whose fixture lives in the
    // home corpus. Before the resolver this was the fatal `fixture not found`.
    let home = scratch("relative-shape");
    let wanted = write_stub(
        &home.join(".codescribe/data_assets"),
        "05_apple-live-parity.wav",
    );

    let out = run(
        &[
            "resolve",
            "tests/assets/data_assets/05_apple-live-parity.wav",
        ],
        &home,
        None,
    );
    assert!(out.status.success(), "resolve failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), wanted.to_string_lossy());
}

#[test]
fn existing_explicit_path_passes_through_untouched() {
    let home = scratch("passthrough-home");
    let elsewhere = scratch("passthrough-clip");
    let clip = write_stub(&elsewhere, "99_ad-hoc.wav");
    // A decoy of the same basename in the home corpus must NOT win: an operator
    // who names a real path means that path.
    write_stub(&home.join(".codescribe/data_assets"), "99_ad-hoc.wav");

    let out = run(&["resolve", clip.to_str().unwrap()], &home, None);
    assert!(out.status.success(), "resolve failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), clip.to_string_lossy());
}

#[test]
fn missing_fixture_fails_loudly_and_lists_every_checked_path() {
    let home = scratch("missing-home");
    let explicit = scratch("missing-explicit");
    fs::create_dir_all(home.join(".codescribe/data_assets")).expect("create empty home corpus");

    let out = run(
        &["resolve", "05_apple-live-parity.wav"],
        &home,
        Some(&explicit),
    );
    assert!(
        !out.status.success(),
        "an unresolvable fixture must fail — a silent empty path fakes a green gate"
    );

    // The recovery hint in the W0-A brief: report the exact paths checked so a
    // cold worker never has to run a scavenger hunt.
    let err = stderr_of(&out);
    for expected in [
        explicit.to_string_lossy().to_string(),
        home.join(".codescribe/data_assets")
            .to_string_lossy()
            .to_string(),
        "tests/assets/data_assets".to_string(),
    ] {
        assert!(
            err.contains(&expected),
            "checked-path list omits {expected}; got:\n{err}"
        );
    }
}

#[test]
fn dir_subcommand_reports_the_active_fixtures_directory() {
    // The Makefile derives ENGINE_CLIP and the 01–04 glob from this one value,
    // so it must never print an empty string (`$(DIR)/05_….wav` → `/05_….wav`).
    let home = scratch("dir-home");
    let corpus = home.join(".codescribe/data_assets");
    write_stub(&corpus, "01_no-to-dobra.wav");

    let out = run(&["dir"], &home, None);
    assert!(out.status.success(), "dir failed: {}", stderr_of(&out));
    assert_eq!(stdout_of(&out), corpus.to_string_lossy());
}

#[test]
fn dir_subcommand_falls_back_to_the_repo_drop_dir_on_a_clean_checkout() {
    let root = fake_repo("dir-clean");
    let home = scratch("dir-clean-home");

    let mut cmd = Command::new("bash");
    cmd.arg(root.join("scripts/lib/data-assets.sh"));
    cmd.arg("dir");
    cmd.env("HOME", &home);
    cmd.env_remove("CODESCRIBE_DATA_ASSETS");
    let out = cmd.output().expect("run copied resolver");

    assert!(out.status.success(), "dir failed: {}", stderr_of(&out));
    assert_eq!(
        stdout_of(&out),
        root.join("tests/assets/data_assets").to_string_lossy(),
        "a clean public checkout must still yield a usable directory"
    );
}
