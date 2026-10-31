//! Exercise the public startup boundary in separate processes: RuntimeHost is
//! immutable after its first lookup, and no fixture may touch account data.

use codescribe_core::config::{
    Config,
    churn::{ChurnMove, ChurnMoveOutcome, migrate_churn_data_at_startup},
    runtime_host::{self, RuntimeHost},
};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

const CHILD: &str = "CODESCRIBE_CHURN_OWNERSHIP_CHILD";
const ROOT: &str = "CODESCRIBE_CHURN_OWNERSHIP_ROOT";
const BRIDGE_FILES: &[(&str, &[u8])] = &[
    ("overlay-delivery-cursor/bus.json", b"cursor cache"),
    ("overlay-delivery-cursor.v1.json", b"previous cursor"),
    ("overlay-read-state.v1.json", b"reply read marks"),
];

#[test]
fn embedded_startup_preserves_standalone_bridge_ownership() {
    isolated(
        "embedded_startup_preserves_standalone_bridge_ownership",
        true,
        false,
    );
}

#[test]
fn standalone_startup_migrates_its_bridge_and_is_idempotent() {
    isolated(
        "standalone_startup_migrates_its_bridge_and_is_idempotent",
        false,
        false,
    );
}

#[test]
fn default_standalone_startup_retains_bridge_ownership() {
    isolated(
        "default_standalone_startup_retains_bridge_ownership",
        false,
        true,
    );
}

fn isolated(name: &str, embedded: bool, defaults: bool) {
    if std::env::var(CHILD).as_deref() == Ok(name) {
        exercise(
            Path::new(&std::env::var_os(ROOT).unwrap()),
            embedded,
            defaults,
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("tmp")).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .env(ROOT, &root)
        .env("TMPDIR", root.join("tmp"))
        .env_remove("CODESCRIBE_DATA_DIR")
        .env_remove("CODESCRIBE_AGENT_BRIDGE_HOME");
    if !defaults {
        command
            .env("CODESCRIBE_DATA_DIR", root.join("standalone"))
            .env(
                "CODESCRIBE_AGENT_BRIDGE_HOME",
                root.join("standalone-bridge"),
            );
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{name} child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "the child must select and execute exactly one regression"
    );
}

fn exercise(root: &Path, embedded: bool, defaults: bool) {
    if embedded {
        runtime_host::configure(
            RuntimeHost::new(root.join("embedded"), "test.embedded.churn".into()).unwrap(),
        )
        .unwrap();
    }
    let data = Config::config_dir();
    let bridge = codescribe_core::stt::active_names::bridge_home();
    let churn = Config::churn_dir();
    // Fail before seeding if any resolver escaped the controlled child root.
    assert!(data.starts_with(root), "unsafe data root: {data:?}");
    assert!(bridge.starts_with(root), "unsafe bridge root: {bridge:?}");
    assert_eq!(churn, data.join("data.noindex"));
    if embedded {
        assert_eq!(data, root.join("embedded"));
        assert_ne!(data, root.join("standalone"));
    } else if !defaults {
        assert_eq!(data, root.join("standalone"));
    }

    let old_log = data.join("logs/codescribe.log");
    write(&old_log, b"owned runtime log\n");
    let log_inode = fs::metadata(&old_log).unwrap().ino();
    let runtime = bridge.join("runtime");
    let mut bridge_inodes = Vec::new();
    for (name, bytes) in BRIDGE_FILES {
        let path = runtime.join(name);
        write(&path, bytes);
        bridge_inodes.push(fs::metadata(path).unwrap().ino());
    }
    let identity = runtime.join("native-wake/identity");
    write(&identity, b"path identity stays here");

    let first = migrate_churn_data_at_startup();
    assert_no_refusals(&first);
    assert!(first.iter().any(|item| {
        item.entry.name == "logs" && matches!(item.outcome, ChurnMoveOutcome::Moved)
    }));
    assert!(!old_log.exists());
    let new_log = churn.join("logs/codescribe.log");
    assert_eq!(fs::read(&new_log).unwrap(), b"owned runtime log\n");
    assert_eq!(fs::metadata(&new_log).unwrap().ino(), log_inode);
    assert_bridge_layout(&runtime, &churn, &bridge_inodes, embedded);

    let second = migrate_churn_data_at_startup();
    assert_no_refusals(&second);
    assert!(second.iter().all(|item| matches!(
        item.outcome,
        ChurnMoveOutcome::AlreadyMigrated | ChurnMoveOutcome::NothingToMove
    )));
    assert_eq!(fs::read(&new_log).unwrap(), b"owned runtime log\n");
    assert_eq!(fs::metadata(new_log).unwrap().ino(), log_inode);
    assert_bridge_layout(&runtime, &churn, &bridge_inodes, embedded);
    assert_eq!(fs::read(identity).unwrap(), b"path identity stays here");
}

fn assert_bridge_layout(runtime: &Path, churn: &Path, inodes: &[u64], embedded: bool) {
    for ((name, bytes), inode) in BRIDGE_FILES.iter().zip(inodes) {
        let source = runtime.join(name);
        let destination = churn.join(name);
        let owned_path = if embedded { &source } else { &destination };
        assert_eq!(fs::read(owned_path).unwrap(), *bytes, "{owned_path:?}");
        assert_eq!(fs::metadata(owned_path).unwrap().ino(), *inode);
        let unowned_path = if embedded { destination } else { source };
        assert!(
            !unowned_path.exists(),
            "bridge state appeared under a second owner: {name}"
        );
    }
}

fn assert_no_refusals(moves: &[ChurnMove]) {
    assert!(!moves.is_empty(), "startup migration must execute");
    for item in moves {
        assert!(!item.outcome.is_refusal(), "{item:?}");
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
