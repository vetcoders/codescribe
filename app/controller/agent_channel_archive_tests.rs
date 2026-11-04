use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

struct ArchiveFixture {
    dir: tempfile::TempDir,
    binding: PathBuf,
    bus: PathBuf,
    lease: String,
    executable: PathBuf,
}

impl ArchiveFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("scratch archive");
        let root = dir.path();
        let binding = root.join(BINDING_FILENAME);
        let bus = root.join("channel-3.jsonl");
        let lease = hex::encode(&Sha256::digest(b"codex\0archive-session")[..16]);
        std::fs::create_dir(root.join("leases")).unwrap();
        std::fs::write(&bus, b"").unwrap();
        std::fs::write(
            &binding,
            serde_json::to_vec(&serde_json::json!({
                "schema": BINDING_SCHEMA,
                "bindings": {"3": {"audience": "archive-agent", "provider": "codex",
                    "provider_session_id": "archive-session", "bus": bus}}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("leases").join(format!("{lease}.json")),
            serde_json::to_vec(&serde_json::json!({
                "schema": "codescribe.agent-bridge.lease.v1", "lease_id": lease,
                "provider": "codex", "provider_session_id": "archive-session",
                "name": "archive-agent", "bus": bus, "cursor": 0, "pending": []
            }))
            .unwrap(),
        )
        .unwrap();
        let executable = root.join("held-helper.py");
        // The scratch wrapper gates the real canonical helper, not a copy
        // of its binding/lease/archive algorithm or a live bus.
        std::fs::write(
            root.join("canonical.txt"),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scripts/bus-demux.py")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            &executable,
            br#"#!/usr/bin/env python3
import os, sys, time
from pathlib import Path
root = Path(__file__).parent
(root / 'entered').touch()
end = time.monotonic() + 10
while not (root / 'release').exists():
    if time.monotonic() > end:
        sys.exit(9)
    time.sleep(0.01)
source = (root / 'canonical.txt').read_text()
os.execv(sys.executable, [sys.executable, source, *sys.argv[1:]])
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            dir,
            binding,
            bus,
            lease,
            executable,
        }
    }

    fn request(&self) -> AgentArchiveRequest {
        AgentArchiveRequest {
            channel: 3,
            provider: "codex".into(),
            provider_session_id: "archive-session".into(),
            lease_id: self.lease.clone(),
            bus: self.bus.clone(),
            executable: self.executable.clone(),
            bridge_home: self.dir.path().to_owned(),
            release_reader: false,
        }
    }

    async fn wait_for_helper(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.dir.path().join("entered").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("canonical helper was spawned");
    }

    fn release(&self) {
        std::fs::write(self.dir.path().join("release"), b"go").unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn an_open_controller_channel_refuses_archive_before_helper_spawn() {
    let fixture = ArchiveFixture::new();
    let controller = RecordingController::new_without_keychain();
    controller
        .toggle_agent_channel_at(
            3,
            &fixture.binding,
            &fixture.bus,
            ChannelOpenMode::AttachedOnly,
        )
        .await
        .unwrap();
    let before = std::fs::read(&fixture.binding).unwrap();
    assert!(
        controller
            .archive_agent_channel(fixture.request())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&fixture.binding).unwrap(), before);
    assert!(!fixture.dir.path().join("entered").exists());
    assert!(controller.agent_channel_snapshot(3).await.is_some());
}

#[tokio::test]
#[serial_test::serial]
async fn archive_completion_precedes_a_queued_channel_open() {
    let fixture = ArchiveFixture::new();
    let controller = Arc::new(RecordingController::new_without_keychain());
    let archive = tokio::spawn({
        let controller = Arc::clone(&controller);
        let request = fixture.request();
        async move { controller.archive_agent_channel(request).await }
    });
    fixture.wait_for_helper().await;
    assert!(
        controller.serial_lock.try_lock().is_err(),
        "archive owns controller lifecycle"
    );
    let reopen = tokio::spawn({
        let controller = Arc::clone(&controller);
        let binding = fixture.binding.clone();
        let bus = fixture.bus.clone();
        async move {
            controller
                .toggle_agent_channel_at(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(
        !reopen.is_finished(),
        "Fn open waits for archive completion"
    );
    fixture.release();
    archive.await.unwrap().unwrap();
    assert!(
        reopen.await.unwrap().is_err(),
        "released binding cannot open capture"
    );
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert!(load_binding(&fixture.binding).unwrap().bindings.is_empty());
    assert!(
        fixture
            .dir
            .path()
            .join("archives")
            .join(format!("{}-3.json", fixture.lease))
            .exists()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn cancelled_archive_keeps_serial_ownership_until_the_helper_exits() {
    let fixture = ArchiveFixture::new();
    let controller = Arc::new(RecordingController::new_without_keychain());
    let archive = tokio::spawn({
        let controller = Arc::clone(&controller);
        let request = fixture.request();
        async move { controller.archive_agent_channel(request).await }
    });
    fixture.wait_for_helper().await;
    archive.abort();
    assert!(archive.await.unwrap_err().is_cancelled());
    assert!(
        controller.serial_lock.try_lock().is_err(),
        "Swift cancellation cannot release lifecycle while helper is alive"
    );
    let reopen = tokio::spawn({
        let controller = Arc::clone(&controller);
        let binding = fixture.binding.clone();
        let bus = fixture.bus.clone();
        async move {
            controller
                .toggle_agent_channel_at(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!reopen.is_finished());
    fixture.release();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), reopen)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert!(load_binding(&fixture.binding).unwrap().bindings.is_empty());
}
