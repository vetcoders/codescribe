//! Contract tests of the Transcript Bus, kept as `transcript_bus::tests`.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use codescribe_core::pipeline::acoustic_ledger::{
    AcousticLedger, SealCoverageReceipt, TerminalFinalityRefusal,
};

use crate::presentation::emitter::TranscriptRevision;

use super::attachments::*;
use super::envelope::*;
use super::routing::*;
use super::{ProjectedSealCoverageReceipt, TranscriptBus, TranscriptBusEvidenceEvent};

fn capture_attachment_fixture() -> (
    tempfile::TempDir,
    Vec<std::path::PathBuf>,
    serde_json::Value,
) {
    let home = std::env::var_os("HOME").unwrap();
    let temp = tempfile::tempdir_in(home).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let buses = vec![root.join("channel-1.jsonl"), root.join("channel-2.jsonl")];
    let recipients = serde_json::json!([
        {"provider":"codex", "provider_session_id":"one", "lease_id":"a", "channel":"1", "audience":"one", "bus":buses[0]},
        {"provider":"codex", "provider_session_id":"two", "lease_id":"b", "channel":"2", "audience":"two", "bus":buses[1]}
    ]);
    let source = serde_json::json!({"schema":"codescribe.transcript-evidence.v1", "session_id":"agent-channel-0-fixture",
        "sequence":0, "capture_epoch":7, "reducer_revision":1, "reducer_action":"record_ledger_projection",
        "occurrence_session_id":"occurrence-fixture", "sample_start":10, "sample_end":50, "document_index":0,
        "channel":"0", "audience":"all", "recipients":recipients, "rendered_text":"Voice words"});
    let opened = serde_json::json!({"schema":"codescribe.channel-session.v1", "session_id":"agent-channel-0-fixture", "state":"open"});
    for bus in &buses {
        std::fs::write(bus, format!("{opened}\n{source}\n")).unwrap();
    }
    let file = root.join("image.png");
    std::fs::write(&file, b"image fixture").unwrap();
    let request = serde_json::json!({"schema":"codescribe.capture-attachment.v1", "kind":"capture_attachment",
        "stage_id":"111111111111111111111111", "session_id":"agent-channel-0-fixture", "capture_epoch":7,
        "source_sequence":0, "emitted_at":"2026-10-20T22:00:00Z", "attachments":[{"name":"image.png", "path":file,
        "media_type":"image/png", "bytes":13, "sha256":"a".repeat(64)}]});
    (temp, buses, request)
}

#[test]
fn capture_attachment_bounded_tail_can_start_inside_an_older_chunked_document() {
    use super::super::transcript_bus_maintenance::generation;
    use std::io::Write;
    let (_temp, buses, request) = capture_attachment_fixture();
    let current = std::fs::read(&buses[0]).expect("bounded-tail attachment fixture must be valid");
    // The real 64 MiB bound cuts through this older encoded document;
    // the complete open + selected source still lie inside the window.
    let older = serde_json::json!({
        "schema":"codescribe.transcript-evidence.v1", "session_id":"older-capture",
        "rendered_text":"x".repeat(50 * 1024 * 1024)
    });
    let prefix = generation::encode(&older).expect("bounded-tail attachment fixture must be valid");
    assert!(prefix.len() > 64 << 20);
    let mut file =
        std::fs::File::create(&buses[0]).expect("bounded-tail attachment fixture must be valid");
    file.write_all(&prefix)
        .expect("bounded-tail attachment fixture must be valid");
    file.write_all(&current)
        .expect("bounded-tail attachment fixture must be valid");
    file.sync_all()
        .expect("bounded-tail attachment fixture must be valid");
    let receipt = super::append_agent_event(&buses[0], &request)
        .expect("bounded-tail attachment fixture must be valid");
    assert_eq!(
        receipt["buses"]
            .as_array()
            .expect("bounded-tail attachment fixture must be valid")
            .len(),
        2
    );
    for bus in &buses {
        let (_, staged) = capture_attachment_source(bus, "agent-channel-0-fixture", &request)
            .expect("bounded-tail attachment fixture must be valid");
        assert!(staged);
    }
}

#[test]
fn capture_attachment_freezes_broadcast_and_commits_all_buses_without_typed_message() {
    let (_temp, buses, request) = capture_attachment_fixture();
    let receipt = super::append_agent_event(&buses[0], &request).unwrap();
    assert_eq!(receipt["buses"].as_array().unwrap().len(), 2);
    for (index, bus) in buses.iter().enumerate() {
        let text = std::fs::read_to_string(bus).unwrap();
        let rows = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let stages = rows
            .iter()
            .filter(|row| row["schema"] == "codescribe.capture-attachment.v1")
            .collect::<Vec<_>>();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0]["recipients"].as_array().unwrap().len(), 2);
        assert_eq!(stages[0]["source_sequence"], 0);
        assert!(
            !rows
                .iter()
                .any(|row| row["schema"] == "codescribe.agent-user-message.v1")
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row["schema"] == "codescribe.capture-attachment-commit.v1")
                .count(),
            usize::from(index == 0)
        );
    }
    super::append_agent_event(&buses[0], &request).unwrap();
    for bus in &buses {
        let text = std::fs::read_to_string(bus).unwrap();
        assert_eq!(
            text.lines()
                .filter(|line| line.contains("\"kind\":\"capture_attachment\""))
                .count(),
            1
        );
    }
}

#[test]
fn capture_attachment_keeps_pasted_segment_when_speech_advances() {
    use std::io::Write;
    let (_temp, buses, request) = capture_attachment_fixture();
    for bus in &buses {
        let first = std::fs::read_to_string(bus).unwrap();
        let mut next: serde_json::Value =
            serde_json::from_str(first.lines().nth(1).unwrap()).unwrap();
        next["sequence"] = 1.into();
        next["reducer_revision"] = 2.into();
        next["document_index"] = 1.into();
        next["sample_start"] = 50.into();
        next["sample_end"] = 100.into();
        next["rendered_text"] = "Continued speech".into();
        writeln!(
            std::fs::OpenOptions::new().append(true).open(bus).unwrap(),
            "{next}"
        )
        .unwrap();
    }
    super::append_agent_event(&buses[0], &request).unwrap();
    // A retry also resolves that same historical segment and is idempotent.
    super::append_agent_event(&buses[0], &request).unwrap();
    for bus in &buses {
        let rows = std::fs::read_to_string(bus).unwrap();
        let stages = rows
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|row| row["schema"] == "codescribe.capture-attachment.v1")
            .collect::<Vec<_>>();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0]["source_sequence"], 0);
        assert_eq!(stages[0]["reducer_revision"], 1);
        assert_eq!(stages[0]["sample_start"], 10);
        assert_eq!(stages[0]["sample_end"], 50);
        assert_eq!(stages[0]["document_index"], 0);
    }
}

#[test]
fn capture_attachment_accepts_a_sealed_segment_while_capture_stays_open() {
    use std::io::Write;
    let (_temp, buses, request) = capture_attachment_fixture();
    for bus in &buses {
        let first = std::fs::read_to_string(bus).unwrap();
        let mut sealed: serde_json::Value =
            serde_json::from_str(first.lines().nth(1).unwrap()).unwrap();
        sealed["sequence"] = 1.into();
        sealed["reducer_action"] = "record_ledger_terminal_seal".into();
        writeln!(
            std::fs::OpenOptions::new().append(true).open(bus).unwrap(),
            "{sealed}"
        )
        .unwrap();
    }
    let mut selected = request;
    selected["source_sequence"] = 1.into();
    super::append_agent_event(&buses[0], &selected).unwrap();
}

#[test]
fn capture_attachment_historical_segment_cannot_borrow_changed_capture_or_address() {
    use std::io::Write;
    for change in ["capture_epoch", "audience", "recipients", "status"] {
        let (_temp, buses, request) = capture_attachment_fixture();
        for bus in &buses {
            let first = std::fs::read_to_string(bus).unwrap();
            let mut next: serde_json::Value =
                serde_json::from_str(first.lines().nth(1).unwrap()).unwrap();
            next["sequence"] = 1.into();
            next[change] = match change {
                "capture_epoch" => 8.into(),
                "audience" => "another audience".into(),
                "recipients" => serde_json::json!([]),
                _ => "session_ended".into(),
            };
            writeln!(
                std::fs::OpenOptions::new().append(true).open(bus).unwrap(),
                "{next}"
            )
            .unwrap();
        }
        assert!(super::append_agent_event(&buses[0], &request).is_err());
        for bus in &buses {
            assert!(
                !std::fs::read_to_string(bus)
                    .unwrap()
                    .contains("capture_attachment")
            );
        }
    }
}

#[test]
fn capture_attachment_refuses_stale_epoch_sequence_or_any_closed_broadcast_recipient() {
    use std::io::Write;
    let (_temp, buses, request) = capture_attachment_fixture();
    for (field, wrong) in [("capture_epoch", 8), ("source_sequence", 1)] {
        let mut invalid = request.clone();
        invalid[field] = wrong.into();
        assert!(super::append_agent_event(&buses[0], &invalid).is_err());
        assert!(
            !std::fs::read_to_string(&buses[0])
                .unwrap()
                .contains("capture_attachment")
        );
    }
    let close = serde_json::json!({"schema":"codescribe.channel-session.v1", "session_id":"agent-channel-0-fixture", "state":"closed"});
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&buses[1])
            .unwrap(),
        "{close}"
    )
    .unwrap();
    assert!(super::append_agent_event(&buses[0], &request).is_err());
    for bus in &buses {
        assert!(
            !std::fs::read_to_string(bus)
                .unwrap()
                .contains("capture_attachment")
        );
    }
}

#[test]
fn capture_attachment_accepts_continuous_channel_segments_and_refuses_sealed_segments() {
    use std::io::Write;
    let (_temp, buses, base_request) = capture_attachment_fixture();
    let recipients = serde_json::json!([
        {"provider":"codex", "provider_session_id":"one", "lease_id":"a", "channel":"7", "audience":"mira", "bus":buses[0]},
        {"provider":"codex", "provider_session_id":"two", "lease_id":"b", "channel":"7", "audience":"mira", "bus":buses[1]}
    ]);
    let recipients_s1 = serde_json::json!({
        "schema":"codescribe.channel-recipients.v1", "kind":"channel_recipients",
        "session_id":"agent-channel-7-fixture-s1", "channel":"7", "audience":"mira", "recipients":recipients
    });
    let opened = serde_json::json!({
        "schema":"codescribe.channel-session.v1", "kind":"channel_session",
        "session_id":"agent-channel-7-fixture", "channel":"7", "state":"open"
    });
    let source_s1 = serde_json::json!({
        "schema":"codescribe.transcript-evidence.v1", "session_id":"agent-channel-7-fixture-s1",
        "sequence":0, "capture_epoch":1, "reducer_revision":1, "reducer_action":"record_ledger_projection",
        "occurrence_session_id":"occurrence-s1", "sample_start":10, "sample_end":50, "document_index":0,
        "channel":"7", "audience":"mira", "recipients":recipients, "rendered_text":"Segment one"
    });
    for bus in &buses {
        std::fs::write(bus, format!("{recipients_s1}\n{opened}\n{source_s1}\n")).unwrap();
    }
    let mut request_s1 = base_request.clone();
    request_s1["stage_id"] = "222222222222222222222222".into();
    request_s1["session_id"] = "agent-channel-7-fixture-s1".into();
    request_s1["capture_epoch"] = 1.into();
    request_s1["source_sequence"] = 0.into();
    let receipt_s1 = super::append_agent_event(&buses[0], &request_s1).unwrap();
    assert_eq!(receipt_s1["buses"].as_array().unwrap().len(), 2);

    // Seal segment 1 with segment_sealed and open segment 2 on the same continuous channel session.
    let segment_sealed_s1 = serde_json::json!({
        "schema":"codescribe.channel-session.v1", "kind":"channel_session",
        "session_id":"agent-channel-7-fixture-s1", "channel_session_id":"agent-channel-7-fixture",
        "channel":"7", "state":"segment_sealed"
    });
    let source_s2 = serde_json::json!({
        "schema":"codescribe.transcript-evidence.v1", "session_id":"agent-channel-7-fixture-s2",
        "sequence":0, "capture_epoch":1, "reducer_revision":1, "reducer_action":"record_ledger_projection",
        "occurrence_session_id":"occurrence-s2", "sample_start":60, "sample_end":120, "document_index":0,
        "channel":"7", "audience":"mira", "recipients":recipients, "rendered_text":"Segment two"
    });
    for bus in &buses {
        writeln!(
            std::fs::OpenOptions::new().append(true).open(bus).unwrap(),
            "{segment_sealed_s1}\n{source_s2}"
        )
        .unwrap();
    }

    // Segment 1 is now sealed and must refuse new capture attachments.
    let mut late_s1 = request_s1.clone();
    late_s1["stage_id"] = "333333333333333333333333".into();
    assert!(super::append_agent_event(&buses[0], &late_s1).is_err());

    // Segment 2 is open under the parent channel session and must accept capture attachments.
    let mut request_s2 = base_request.clone();
    request_s2["stage_id"] = "444444444444444444444444".into();
    request_s2["session_id"] = "agent-channel-7-fixture-s2".into();
    request_s2["capture_epoch"] = 1.into();
    request_s2["source_sequence"] = 0.into();
    let receipt_s2 = super::append_agent_event(&buses[0], &request_s2).unwrap();
    assert_eq!(receipt_s2["buses"].as_array().unwrap().len(), 2);

    // Sealing the parent channel session closes segment 2 as well.
    let parent_sealed = serde_json::json!({
        "schema":"codescribe.channel-session.v1", "kind":"channel_session",
        "session_id":"agent-channel-7-fixture", "channel":"7", "state":"sealed"
    });
    for bus in &buses {
        writeln!(
            std::fs::OpenOptions::new().append(true).open(bus).unwrap(),
            "{parent_sealed}"
        )
        .unwrap();
    }
    let mut late_s2 = request_s2;
    late_s2["stage_id"] = "555555555555555555555555".into();
    assert!(super::append_agent_event(&buses[0], &late_s2).is_err());
}
#[test]
fn typed_publication_uses_private_journal_and_refuses_audio_claims() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bus.jsonl");
    let lease = hex::encode(Sha256::digest(b"codex\0agent-a"));
    let owner = serde_json::json!({"provider":"codex", "provider_session_id":"agent-a",
        "lease_id":&lease[..32], "channel":"2", "audience":"lena", "bus":path});
    let mut event = owner.clone();
    event.as_object_mut().unwrap().extend(
        serde_json::json!({
        "schema":"codescribe.agent-user-message.v1", "kind":"agent_user_message",
        "message_id":"111111111111111111111111", "source_event_id":"111111111111111111111111",
        "source":"typed", "text":"Iwo", "emitted_at":"2026-10-05T10:00:00Z",
        "recipients":[owner]})
        .as_object()
        .unwrap()
        .clone(),
    );
    assert!(
        super::append_agent_event(&path, &event).unwrap()["length"]
            .as_u64()
            .unwrap()
            > 0
    );
    let before = std::fs::read(&path).unwrap();
    event["capture_epoch"] = serde_json::json!(1);
    assert!(super::append_agent_event(&path, &event).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    event.as_object_mut().unwrap().remove("capture_epoch");
    event["recipients"][0]["provider_session_id"] = serde_json::json!("foreign");
    assert!(super::append_agent_event(&path, &event).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn delivery_controls_publish_typed_voice_and_owner_scoped_requests() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("controls.jsonl");
    let lease = hex::encode(Sha256::digest(b"codex\0agent-a"));
    let base = serde_json::json!({
        "schema": "codescribe.delivery-control.v1",
        "control_id": "111111111111111111111111",
        "emitted_at": "2026-10-15T10:00:00.123456Z",
        "provider": "codex", "provider_session_id": "agent-a",
        "lease_id": &lease[..32], "bus": path, "channel": "2", "audience": "lena",
    });
    let mut expected = Vec::new();
    for kind in ["revise", "withdraw", "release"] {
        for target in [
            serde_json::json!({}),
            serde_json::json!({"message_id": "222222222222222222222222"}),
            serde_json::json!({"session_id": "agent-channel-2-take-a", "utterance_id": 42}),
            serde_json::json!({"session_id": "agent-channel-2-take-b", "utterance_id": 42}),
        ] {
            let mut event = base.clone();
            event["kind"] = serde_json::json!(kind);
            if kind == "revise" {
                event["text"] = serde_json::json!("Revised Iwo");
            }
            event
                .as_object_mut()
                .unwrap()
                .extend(target.as_object().unwrap().clone());
            assert!(
                super::append_agent_event(&path, &event).unwrap()["length"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
            expected.push(event);
        }
    }
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(crate::durable_bus_oracle::rows(&raw).unwrap(), expected);
}

#[test]
fn delivery_controls_reject_invalid_identity_owner_target_and_audio_claims() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("controls.jsonl");
    let lease = hex::encode(Sha256::digest(b"codex\0agent-a"));
    let valid = serde_json::json!({
        "schema": "codescribe.delivery-control.v1", "kind": "revise",
        "control_id": "111111111111111111111111", "text": "Revised Iwo",
        "emitted_at": "2026-10-15T10:00:00.123456Z",
        "provider": "codex", "provider_session_id": "agent-a",
        "lease_id": &lease[..32], "bus": path, "channel": "2", "audience": "lena",
    });
    super::append_agent_event(&path, &valid).unwrap();
    let before = std::fs::read(&path).unwrap();
    for key in [
        "control_id",
        "emitted_at",
        "provider",
        "provider_session_id",
        "lease_id",
        "bus",
        "channel",
        "audience",
        "text",
    ] {
        let mut event = valid.clone();
        event.as_object_mut().unwrap().remove(key);
        assert!(super::append_agent_event(&path, &event).is_err(), "{key}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    for (key, value) in [
        ("schema", serde_json::json!("codescribe.unknown.v1")),
        ("kind", serde_json::json!("send")),
        ("control_id", serde_json::json!("bad-id")),
        ("control_id", serde_json::json!("AAAAAAAAAAAAAAAAAAAAAAAA")),
        ("emitted_at", serde_json::json!("not-a-timestamp")),
        ("provider", serde_json::json!("Codex")),
        ("provider_session_id", serde_json::json!("foreign")),
        ("lease_id", serde_json::json!("wrong-owner")),
        ("bus", serde_json::json!("/another/bus.jsonl")),
        ("channel", serde_json::json!("0")),
        ("channel", serde_json::json!(2)),
        ("audience", serde_json::json!(" ")),
        ("message_id", serde_json::json!("bad-id")),
        ("utterance_id", serde_json::json!("42")),
        ("utterance_id", serde_json::json!(-1)),
        ("utterance_id", serde_json::json!(1.5)),
        ("utterance_id", serde_json::json!(42)),
        ("session_id", serde_json::json!(" ")),
        ("session_id", serde_json::json!(42)),
        ("text", serde_json::json!(" ")),
        ("text", serde_json::json!("x".repeat(64 * 1024 + 1))),
        ("status", serde_json::json!("transcript_sealed")),
        ("wav", serde_json::json!("/capture.wav")),
        ("capture_epoch", serde_json::json!(1)),
        ("sample_start", serde_json::json!(0)),
        ("sample_end", serde_json::json!(16_000)),
        ("occurrence_session_id", serde_json::json!("take")),
        ("hold_until", serde_json::json!("2026-10-15T10:00:10Z")),
    ] {
        let mut event = valid.clone();
        event[key] = value;
        let error = super::append_agent_event(&path, &event).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{key}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn panel_focus_request_and_result_publish_without_transcript_claims() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("channel-5.jsonl");
    let lease = hex::encode(Sha256::digest(b"claude-code\0agent-s"));
    let request = serde_json::json!({
        "schema": "codescribe.agent-panel-focus.v1", "kind": "focus_agent_panel",
        "request_id": "333333333333333333333333",
        "emitted_at": "2026-10-18T08:30:00.123Z",
        "provider": "claude-code", "provider_session_id": "agent-s",
        "lease_id": &lease[..32], "bus": path, "channel": "5", "source": "overlay_click",
    });
    let result = serde_json::json!({
        "schema": "codescribe.agent-panel-focus-result.v1", "kind": "agent_panel_focus_result",
        "request_id": "333333333333333333333333", "emitted_at": "2026-10-18T08:30:00Z",
        "provider": "claude-code", "provider_session_id": "agent-s",
        "lease_id": &lease[..32], "status": "focused", "exit_code": 0, "elapsed_ms": 41,
        "frame": {"session": "vibecrafted", "pane_id": 9,
                  "host_bundle_id": "io.vetcoders.vibecrafted", "host_pid": 43854},
    });
    // A follower without a pane answers for the agent's terminal tab.
    let terminal_result = serde_json::json!({
        "schema": "codescribe.agent-panel-focus-result.v1", "kind": "agent_panel_focus_result",
        "request_id": "444444444444444444444444", "emitted_at": "2026-10-21T08:30:00Z",
        "provider": "claude-code", "provider_session_id": "agent-s",
        "lease_id": &lease[..32], "status": "automation_denied", "exit_code": 1,
        "elapsed_ms": 140, "target": "iterm2", "frame": null,
        "terminal": {"host": "iterm2", "host_bundle_id": "com.googlecode.iterm2",
                     "tty": "/dev/ttys005"},
    });
    for event in [&request, &result, &terminal_result] {
        assert!(
            super::append_agent_event(&path, event).unwrap()["length"]
                .as_u64()
                .unwrap()
                > 0
        );
    }
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        crate::durable_bus_oracle::rows(&raw).unwrap(),
        vec![request.clone(), result.clone(), terminal_result.clone()]
    );
    let before = std::fs::read(&path).unwrap();
    for (base, key, value) in [
        (&request, "request_id", serde_json::json!("bad-id")),
        (&request, "lease_id", serde_json::json!("wrong-owner")),
        (&request, "bus", serde_json::json!("/another/bus.jsonl")),
        (&request, "channel", serde_json::json!("0")),
        (&request, "emitted_at", serde_json::json!("not-a-timestamp")),
        (&request, "status", serde_json::json!("transcript_sealed")),
        (&request, "text", serde_json::json!("pokaż panel")),
        (&request, "kind", serde_json::json!("agent_reply")),
        (&result, "status", serde_json::json!("transcript_sealed")),
        (&result, "exit_code", serde_json::json!("0")),
        (&result, "elapsed_ms", serde_json::json!(-1)),
        (&result, "frame", serde_json::json!("terminal_9")),
        (&terminal_result, "target", serde_json::json!("kitty")),
        (
            &terminal_result,
            "terminal",
            serde_json::json!("/dev/ttys005"),
        ),
        (&terminal_result, "status", serde_json::json!("tab_missing")),
        (&result, "wav", serde_json::json!("/capture.wav")),
        (&result, "provider_session_id", serde_json::json!("foreign")),
    ] {
        let mut event = base.clone();
        event[key] = value;
        let error = super::append_agent_event(&path, &event).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{key}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

fn fixture_refusal(receipt: &SealCoverageReceipt) -> TerminalFinalityRefusal {
    let mut ledger = AcousticLedger::new();
    assert!(ledger.record_seal_coverage(receipt.clone()));
    ledger
        .terminal_finality(&receipt.session_id, receipt.capture_epoch)
        .into_refusal()
        .expect("fixture has no issued terminal seal")
}
use super::super::emitter::{TranscriptReducer, UserRevisionIntent};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, DocumentRevisionProvenance, EnergyCalibration, ObservationIdentity,
    ObservationProducer, OccurrenceIdentity,
};
use std::sync::Arc;

/// Serializes tests that emit the persistence-failure `warn!` callsite.
/// tracing caches each callsite's `Interest` process-globally and
/// `DefaultCallsite::set_interest` is a plain last-writer-wins store: a
/// sibling test's first hit of the callsite with no subscriber computes
/// `Interest::never` from an empty dispatcher registry and can store it
/// *after* the capturing test rebuilt the cache for its scoped
/// subscriber, silently disabling the callsite on the capturing thread as
/// well. Holding this lock for the whole body of every test that can emit
/// the callsite removes that cross-thread dependency.
static PERSISTENCE_LOG_SERIAL: Mutex<()> = Mutex::new(());

fn persistence_log_serial() -> std::sync::MutexGuard<'static, ()> {
    PERSISTENCE_LOG_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[test]
fn history_lists_three_revisions_of_one_take_with_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history.jsonl");
    let bus = TranscriptBus::open_at(session("history-take"), path.clone(), None).unwrap();
    bus.publish_started();
    let (ledger, _, base) = committed_fixture("history-take");
    let published = bus.publish_revision(&base, &ledger);
    assert_eq!(published.len(), 2);
    let mut formatter = published[0].clone();
    formatter.reducer_revision += 1;
    formatter.reducer_action = "apply_manual_edit".to_string();
    formatter.rendered_text = "Formatted words".to_string();
    formatter.acoustic_receipts[0].manual_edit_receipt =
        Some("formatter-history-take-2-3-0".to_string());
    let mut retranscribe = formatter.clone();
    retranscribe.reducer_revision += 1;
    retranscribe.rendered_text = "Retranscribed words".to_string();
    retranscribe.acoustic_receipts[0].manual_edit_receipt =
        Some("retranscribe-history-take-3-4-1".to_string());
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", serde_json::to_string(&formatter).unwrap()).unwrap();
    writeln!(file, "{}", serde_json::to_string(&retranscribe).unwrap()).unwrap();
    let history = document_history_at(&path, "history-take").unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].revision, base.revision);
    assert_eq!(history[0].provenance, "acoustic-ledger");
    assert_eq!(history[1].provenance, "formatter");
    assert_eq!(history[2].provenance, "retranscribe");
    assert_eq!(history[2].rendered_text, "Retranscribed words");
    assert!(
        document_history_at(&path, "another-take")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recent_take_history_reads_only_the_bus_tail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large-history.jsonl");
    let row = format!(
        "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"old\",\"padding\":\"{}\"}}\n",
        "x".repeat(2048)
    );
    let mut out = std::fs::File::create(&path).unwrap();
    while out.metadata().unwrap().len() < 10 * 1024 * 1024 {
        out.write_all(row.as_bytes()).unwrap();
    }
    drop(out);
    let older_bytes = std::fs::metadata(&path).unwrap().len();
    let bus = TranscriptBus::open_at(session("recent-take"), path.clone(), None).unwrap();
    bus.publish_started();
    let (ledger, _, base) = committed_fixture("recent-take");
    assert!(!bus.publish_revision(&base, &ledger).is_empty());
    let recent_bytes = crate::durable_bus_oracle::logical_text(&path).len() as u64 - older_bytes;
    let mut bytes_read = 0;
    let history = document_history_at_counted(&path, "recent-take", &mut bytes_read).unwrap();
    assert!(!history.is_empty());
    assert!(
        bytes_read <= recent_bytes + 64 * 1024,
        "read {bytes_read} bytes for {recent_bytes} bytes of recent-session rows"
    );
}

/// Regression guard: a bus that still carries this session's
/// `session_started` row must stop at that row and return exactly the
/// history it returns today, unaffected by the new scan budget or the
/// substring prefilter.
#[test]
fn history_with_session_started_is_unchanged_by_the_scan_budget() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history-unchanged.jsonl");
    let bus = TranscriptBus::open_at(session("budget-unaffected"), path.clone(), None).unwrap();
    bus.publish_started();
    let (ledger, _, base) = committed_fixture("budget-unaffected");
    let published = bus.publish_revision(&base, &ledger);
    assert!(!published.is_empty());
    let mut edit = published[0].clone();
    edit.reducer_revision += 1;
    edit.reducer_action = "apply_manual_edit".to_string();
    edit.rendered_text = "Edited words".to_string();
    edit.acoustic_receipts[0].manual_edit_receipt =
        Some("formatter-budget-unaffected-2-3-0".to_string());
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{}", serde_json::to_string(&edit).unwrap()).unwrap();
    drop(file);
    let mut bytes_read = 0;
    let history = document_history_at_counted(&path, "budget-unaffected", &mut bytes_read).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].revision, base.revision);
    assert_eq!(history[1].provenance, "formatter");
    assert_eq!(history[1].rendered_text, "Edited words");
    assert!(
        bytes_read < DOCUMENT_HISTORY_SCAN_BUDGET_BYTES,
        "a bus carrying session_started must stop well short of the budget"
    );
}

/// When a take began while persistence was disabled
/// (`TranscriptBus::open_at` refuses to append after a bus whose last
/// byte is not `\n`), `session_started` is never written for it. Before
/// this cut the backward scan then walked the whole bus; now it must
/// stop at `DOCUMENT_HISTORY_SCAN_BUDGET_BYTES` and still surface the
/// tail revision that was actually asked for.
#[test]
fn history_without_session_started_stops_at_the_scan_budget_and_keeps_the_tail() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unbounded.jsonl");
    let filler = format!(
        "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"other-take\",\"padding\":\"{}\"}}\n",
        "x".repeat(4096)
    );
    let mut out = std::fs::File::create(&path).unwrap();
    while out.metadata().unwrap().len() < DOCUMENT_HISTORY_SCAN_BUDGET_BYTES + 8 * 1024 * 1024 {
        out.write_all(filler.as_bytes()).unwrap();
    }
    let padded_len = out.metadata().unwrap().len();

    // Build a real revision event without ever writing `session_started`
    // for it: generate it on a scratch bus, then append the raw line
    // directly, the way an actually-disabled-persistence take would.
    let scratch_path = temp.path().join("scratch.jsonl");
    let scratch_bus =
        TranscriptBus::open_at(session("unbounded-take"), scratch_path, None).unwrap();
    let (ledger, _, base) = committed_fixture("unbounded-take");
    let published = scratch_bus.publish_revision(&base, &ledger);
    assert!(!published.is_empty());
    for event in &published {
        writeln!(out, "{}", serde_json::to_string(event).unwrap()).unwrap();
    }
    drop(out);
    let total_len = std::fs::metadata(&path).unwrap().len();
    assert!(total_len > padded_len);

    let mut bytes_read = 0;
    let history = document_history_at_counted(&path, "unbounded-take", &mut bytes_read).unwrap();

    assert!(!history.is_empty(), "tail revision must still surface");
    assert_eq!(history[0].revision, base.revision);
    assert!(
        bytes_read < total_len,
        "scan read {bytes_read} of {total_len} bytes; the budget should stop it short of the whole file"
    );
    assert!(
        bytes_read >= DOCUMENT_HISTORY_SCAN_BUDGET_BYTES,
        "scan stopped at {bytes_read} bytes, short of the {DOCUMENT_HISTORY_SCAN_BUDGET_BYTES} byte budget"
    );
}

/// Not run by default: writes and scans a ~1 GiB fixture to measure
/// `document_history_at`'s wall time with the scan budget and substring
/// prefilter in place, for a session that never appears in the bus (the
/// worst case: every row is read but none ever reaches `serde_json`).
/// Run explicitly with:
///   cargo test -p codescribe --lib \
///     transcript_bus::tests::document_history_scan_budget_bounds_a_one_gib_bus \
///     -- --ignored --nocapture
#[test]
fn document_history_scan_budget_bounds_a_one_gib_bus() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("one-gib.jsonl");
    let filler = format!(
        "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"other-take\",\"padding\":\"{}\"}}\n",
        "x".repeat(4096)
    );
    let block = filler.repeat(256);
    let mut out = std::fs::File::create(&path).unwrap();
    let mut written = 0usize;
    while written < 1024 * 1024 * 1024 {
        out.write_all(block.as_bytes()).unwrap();
        written += block.len();
    }
    drop(out);

    let started = std::time::Instant::now();
    let mut bytes_read = 0;
    let history = document_history_at_counted(&path, "absent-take", &mut bytes_read).unwrap();
    let elapsed = started.elapsed();

    assert!(history.is_empty());
    assert!(bytes_read <= DOCUMENT_HISTORY_SCAN_BUDGET_BYTES + 64 * 1024);
    eprintln!("document_history_at scanned {bytes_read} bytes of a 1 GiB bus in {elapsed:?}");
}

#[test]
fn history_versions_match_when_each_revision_carries_text_only_once() {
    let temp = tempfile::tempdir().unwrap();
    let repeated_path = temp.path().join("repeated.jsonl");
    let deduplicated_path = temp.path().join("deduplicated.jsonl");
    let bus = TranscriptBus::open_at(session("history-once"), repeated_path.clone(), None).unwrap();
    bus.publish_started();
    let (ledger, _, base) = committed_fixture("history-once");
    let mut versions = vec![bus.publish_revision(&base, &ledger)[0].clone()];
    for offset in 1..=2 {
        let mut next = versions[0].clone();
        next.reducer_revision += offset;
        next.reducer_action = "apply_manual_edit".to_string();
        next.rendered_text = format!("version {}", next.reducer_revision);
        versions.push(next);
    }
    let start = r#"{"schema":"codescribe.transcript.v1","session_id":"history-once","status":"session_started"}"#;
    let mut repeated = format!("{start}\n");
    let mut deduplicated = format!("{start}\n");
    for event in &versions {
        let full = serde_json::to_string(event).unwrap();
        repeated.push_str(&format!("{full}\n{full}\n"));
        let mut without_text = serde_json::to_value(event).unwrap();
        without_text
            .as_object_mut()
            .unwrap()
            .remove("rendered_text");
        deduplicated.push_str(&format!(
            "{full}\n{}\n",
            serde_json::to_string(&without_text).unwrap()
        ));
    }
    std::fs::write(&repeated_path, repeated).unwrap();
    std::fs::write(&deduplicated_path, deduplicated).unwrap();
    let expected = document_history_at(&repeated_path, "history-once").unwrap();
    assert_eq!(expected.len(), 3);
    assert_eq!(
        document_history_at(&deduplicated_path, "history-once").unwrap(),
        expected
    );
}

#[test]
fn managed_journal_refuses_age_rewrite_and_preserves_revision_history() {
    use std::time::{Duration, SystemTime};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("history-retention.jsonl");
    let bus = TranscriptBus::open_at(session("old-take"), path.clone(), None).unwrap();
    let (ledger, _, base) = committed_fixture("old-take");
    let mut event = bus.publish_revision(&base, &ledger)[0].clone();
    event.emitted_at = "2020-01-01T00:00:00Z".to_string();
    std::fs::write(&path, format!(
        "{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"old-take\",\"status\":\"session_started\"}}\n{}\n{{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"old-take\",\"status\":\"session_ended\"}}\n",
        serde_json::to_string(&event).unwrap()
    )).unwrap();
    let before = document_history_at(&path, "old-take").unwrap();
    assert_eq!(before.len(), 1);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(120)),
    )
    .unwrap();
    let original = crate::durable_bus_oracle::logical_text(&path);
    let refused = super::super::transcript_bus_maintenance::compact_bus(&path, 14, false);
    assert!(
        refused.is_err(),
        "managed daily generations never age-drop evidence"
    );
    assert_eq!(crate::durable_bus_oracle::logical_text(&path), original);
    assert_eq!(document_history_at(&path, "old-take").unwrap(), before);
}

#[derive(Default)]
struct Fault {
    remaining: Option<usize>,
    flush: bool,
    writes: usize,
    flushes: usize,
}

/// Real file-backed writes with controlled prefix/flush failures. Clearing
/// the fault makes the underlying sink usable, so no-retry is falsifiable.
struct FaultWriter {
    inner: Box<dyn Write + Send>,
    fault: Arc<Mutex<Fault>>,
}

impl Write for FaultWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut fault = self.fault.lock().unwrap();
        fault.writes += 1;
        if fault.remaining == Some(0) {
            return Err(io::Error::other("controlled append failure"));
        }
        let limit = fault.remaining.unwrap_or(bytes.len()).min(bytes.len());
        let written = self.inner.write(&bytes[..limit])?;
        if let Some(remaining) = &mut fault.remaining {
            *remaining -= written;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut fault = self.fault.lock().unwrap();
        fault.flushes += 1;
        if fault.flush {
            return Err(io::Error::other("controlled flush failure"));
        }
        self.inner.flush()
    }
}

fn session(id: &str) -> TranscriptSession {
    TranscriptSession {
        session_id: id.to_string(),
        mode: TranscriptMode::Agent,
        has_latched_target: false,
        latched_target_is_self: false,
        audience: None,
        badge_only: false,
    }
}

fn unlatched_dictation(id: &str) -> TranscriptSession {
    TranscriptSession {
        session_id: id.to_string(),
        mode: TranscriptMode::Dictation,
        has_latched_target: false,
        latched_target_is_self: false,
        audience: None,
        badge_only: false,
    }
}

#[test]
fn channel_rows_carry_audience_and_do_not_offer_paste() {
    let dir = tempfile::tempdir().unwrap();
    let mut channel = session("channel-leon");
    channel.audience = Some("Leon".to_string());
    channel.badge_only = true;
    let bus = TranscriptBus::open_at(channel, dir.path().join("channel.jsonl"), None).unwrap();
    let (ledger, _, revision) = committed_fixture("channel-leon");
    let events = bus.publish_revision(&revision, &ledger);
    assert!(!events.is_empty());
    assert!(events.iter().all(|event| {
        event.audience.as_deref() == Some("Leon")
            && !event.can_paste
            && !event.can_insert
            && !event.can_send_to_agent
    }));
    let raw = std::fs::read_to_string(dir.path().join("channel.jsonl")).unwrap();
    assert!(raw.contains("\"audience\":\"Leon\"") || raw.contains("\"audience\": \"Leon\""));

    let plain_dir = tempfile::tempdir().unwrap();
    let plain = TranscriptBus::open_at(
        session("plain-take"),
        plain_dir.path().join("plain.jsonl"),
        None,
    )
    .unwrap();
    let (ledger, _, revision) = committed_fixture("plain-take");
    let events = plain.publish_revision(&revision, &ledger);
    assert!(events.iter().all(|event| event.audience.is_none()));
    let raw = std::fs::read_to_string(plain_dir.path().join("plain.jsonl")).unwrap();
    assert!(
        !raw.contains("\"audience\""),
        "rows without an audience omit the field: {raw}"
    );

    let broadcast_dir = tempfile::tempdir().unwrap();
    let mut broadcast = session("channel-all");
    broadcast.audience = Some("*".to_string());
    broadcast.badge_only = true;
    let bus =
        TranscriptBus::open_at(broadcast, broadcast_dir.path().join("all.jsonl"), None).unwrap();
    let (ledger, _, revision) = committed_fixture("channel-all");
    let events = bus.publish_revision(&revision, &ledger);
    assert!(
        events
            .iter()
            .all(|event| event.audience.as_deref() == Some("*"))
    );
}

#[test]
fn channel_send_hold_only_stamps_the_closing_clean_and_evidence_twins() {
    for seconds in [0, 10] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("held-channel.jsonl");
        let id = "channel-send-hold";
        let mut channel = session(id);
        channel.audience = Some("Leon".into());
        channel.badge_only = true;
        let bus = TranscriptBus::open_at(channel, path.clone(), None).unwrap();
        bus.publish_started();
        let (mut ledger, mut reducer, revision) = committed_fixture(id);
        bus.publish_revision(&revision, &ledger);
        let draft = std::fs::read_to_string(&path).unwrap();
        assert!(!draft.contains("hold_until"));
        assert!(!draft.contains("transcript_sealed"));
        for entry in &revision.entries {
            ledger.schedule_frontier(entry.occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&entry.occurrence, ObservationProducer::Apple));
        }
        let seal = ledger.seal_terminal(id, 7).unwrap();
        let sealed = reducer.apply_ledger_seal(&seal).unwrap();
        bus.publish_revision(&sealed, &ledger);
        let before_close = std::fs::read_to_string(&path).unwrap();
        assert!(
            !before_close.contains("hold_until"),
            "the reducer seal is still pre-close"
        );
        bus.publish_channel_closed(seconds, None).unwrap();
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-session.v1",
            "state": "sealed",
            "session_id": id,
        }));
        let raw = std::fs::read_to_string(&path).unwrap();
        let rows = crate::durable_bus_oracle::rows(&raw).unwrap();
        let closing = rows
            .iter()
            .filter(|row| row["status"] == "transcript_sealed")
            .collect::<Vec<_>>();
        assert_eq!(closing.len(), 2);
        assert_eq!(closing[0]["schema"], "codescribe.transcript-evidence.v1");
        assert_eq!(closing[1]["schema"], "codescribe.transcript.v1");
        assert_eq!(closing[0]["rendered_text"], closing[1]["text"]);
        assert_eq!(closing[0]["reducer_revision"], sealed.revision);
        assert!(
            closing.iter().all(|row| row.get("coverage").is_none()),
            "a ledger-sealed close keeps its authority"
        );
        assert!(
            closing.iter().all(|row| row.get("utterance_id").is_none()),
            "a producer request is not the engine's canonical utterance ID"
        );
        assert!(closing.iter().all(|row| row["session_id"] == id));
        assert_eq!(closing[0]["hold_until"], closing[1]["hold_until"]);
        assert_eq!(
            rows.last().unwrap()["schema"],
            "codescribe.channel-session.v1"
        );
        if seconds == 0 {
            assert!(closing.iter().all(|row| row.get("hold_until").is_none()));
        } else {
            let emitted =
                chrono::DateTime::parse_from_rfc3339(closing[0]["emitted_at"].as_str().unwrap())
                    .unwrap();
            let deadline = closing[0]["hold_until"].as_str().unwrap();
            assert!(deadline.ends_with('Z'));
            assert_eq!(
                deadline.split('.').nth(1).unwrap().len(),
                7,
                "six digits plus Z"
            );
            let deadline = chrono::DateTime::parse_from_rfc3339(deadline).unwrap();
            assert_eq!(
                (deadline - emitted).num_microseconds(),
                Some(i64::from(seconds) * 1_000_000)
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| row.get("hold_until").is_some())
                    .count(),
                2
            );
        }
        bus.publish_channel_closed(seconds, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            raw,
            "closing is idempotent"
        );
        assert!(
            !serde_json::to_string(bus.writer.lock().unwrap().last_projection.as_ref().unwrap())
                .unwrap()
                .contains("hold_until")
        );
    }
}

/// Maciej, 2026-10-17 09:59Z: on a day the ledger sealed no channel take,
/// spoken messages left at once while typed ones waited out the hold.
#[test]
fn unsealed_channel_close_still_holds_the_spoken_words_without_certifying_them() {
    for seconds in [0, 10] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unsealed-channel.jsonl");
        let id = "channel-unsealed-hold";
        let mut channel = session(id);
        channel.audience = Some("Leon".into());
        channel.badge_only = true;
        let bus = TranscriptBus::open_at(channel, path.clone(), None).unwrap();
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture(id);
        bus.publish_revision(&revision, &ledger);
        assert!(!bus.writer.lock().unwrap().sealed);
        bus.publish_channel_closed(seconds, None).unwrap();
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-session.v1",
            "state": "sealed",
            "session_id": id,
        }));
        let raw = std::fs::read_to_string(&path).unwrap();
        let rows = crate::durable_bus_oracle::rows(&raw).unwrap();
        let closing = rows
            .iter()
            .filter(|row| row["status"] == "transcript_sealed")
            .collect::<Vec<_>>();
        assert_eq!(closing.len(), 2);
        assert_eq!(closing[0]["schema"], "codescribe.transcript-evidence.v1");
        assert_eq!(closing[1]["schema"], "codescribe.transcript.v1");
        assert_eq!(closing[0]["rendered_text"], closing[1]["text"]);
        assert_eq!(closing[0]["rendered_text"], revision.rendered_text);
        assert_eq!(closing[0]["reducer_revision"], revision.revision);
        assert!(
            closing.iter().all(|row| row["coverage"] == "refused"),
            "the words are held and delivered, not certified by the ledger"
        );
        assert_eq!(closing[0]["hold_until"], closing[1]["hold_until"]);
        assert_eq!(closing[0].get("hold_until").is_some(), seconds > 0);
        assert!(
            bus.publish_ended(
                TranscriptSessionEndReason::CoverageRefused,
                true,
                TranscriptDelivery::Unattempted,
            )
            .is_some(),
            "the joined close lets the unsealed session end"
        );
        let ended = std::fs::read_to_string(&path).unwrap();
        bus.publish_channel_closed(seconds, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            ended,
            "closing is idempotent"
        );
    }
}

/// The live channel opens its bus the way `agent_channel` does: a
/// recipients receipt and a capture epoch, never `publish_started`.
/// Every other channel test starts the bus by hand, which hid that no
/// production take ever reached the closing decision (take f32ff617).
#[test]
fn live_channel_close_decides_without_a_session_started_row() {
    use codescribe_core::pipeline::acoustic_ledger::{SealCoverageReceipt, SealCoverageStatus};

    // Measured speech at 48 kHz: absent, 224 ms (f32ff617), 400 ms.
    for speech_samples in [None, Some(10_752), Some(19_200)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live-channel.jsonl");
        let id = "agent-channel-1-live";
        let mut channel = session(id);
        channel.audience = Some("Leon".into());
        channel.badge_only = true;
        let bus = TranscriptBus::open_at(channel, path.clone(), None).unwrap();
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-recipients.v1",
            "kind": "channel_recipients",
            "session_id": id,
            "channel": "1",
            "audience": "Leon",
            "recipients": ["leon"],
        }));
        bus.observe_capture_opened(id, 7);
        let (mut ledger, _, revision) = committed_fixture(id);
        bus.publish_revision(&revision, &ledger);
        if let Some(speech_samples) = speech_samples {
            assert!(ledger.record_seal_coverage(SealCoverageReceipt {
                sample_rate_hz: Some(48_000),
                session_id: id.into(),
                capture_epoch: 7,
                speech_samples,
                covered_samples: 0,
                uncovered_speech_ranges: Vec::new(),
                max_uncovered_samples: speech_samples,
                incomplete_threshold_samples: 4_000,
                status: SealCoverageStatus::Incomplete,
                speech_producer: "silero_boundaries".into(),
                availability: "observed".into(),
                observed_samples: Some(124_928),
            }));
        }
        bus.observe_channel_publication_evidence(&ledger);
        bus.publish_channel_closed(10, None).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let rows = crate::durable_bus_oracle::rows(&raw).unwrap();
        assert!(
            rows.iter().all(|row| row["status"] != "session_started"),
            "the live channel shape writes no session_started row: {raw}"
        );
        let sealed = rows
            .iter()
            .filter(|row| row["status"] == "transcript_sealed")
            .collect::<Vec<_>>();
        let refused = rows
            .iter()
            .filter(|row| row["status"] == "channel_publication_refused")
            .collect::<Vec<_>>();
        if speech_samples == Some(10_752) {
            assert!(sealed.is_empty(), "sub-floor unwitnessed text: {raw}");
            assert_eq!(refused.len(), 1, "{raw}");
            assert_eq!(refused[0]["reason"], "sound_without_speech");
            assert_eq!(refused[0]["capture_epoch"], 7);
            let ended = rows.last().unwrap();
            assert_eq!(ended["status"], "session_ended");
            assert_eq!(ended["phase"], "no_speech");
        } else {
            assert!(refused.is_empty(), "{raw}");
            assert_eq!(sealed.len(), 2, "evidence and clean closing twins: {raw}");
            assert!(sealed.iter().all(|row| row["coverage"] == "refused"));
            assert!(sealed.iter().all(|row| row.get("hold_until").is_some()));
            assert!(sealed.iter().all(|row| row["channel"] == "1"));
            assert!(
                sealed
                    .iter()
                    .all(|row| row["closing_decision"] == "publish")
            );
        }
    }
}

#[test]
fn channel_lifecycle_cannot_flush_before_the_joined_hold_snapshot() {
    for seconds in [0, 10] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ordered-channel.jsonl");
        let id = "channel-close-order";
        let mut channel = session(id);
        channel.audience = Some("Leon".into());
        channel.badge_only = true;
        let bus = TranscriptBus::open_at(channel, path.clone(), None).unwrap();
        bus.publish_started();
        let (mut ledger, mut reducer, revision) = committed_fixture(id);
        bus.publish_revision(&revision, &ledger);
        let before_end = std::fs::read(&path).unwrap();
        assert!(
            bus.publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before_end);
        assert!(!bus.writer.lock().unwrap().ended);

        for entry in &revision.entries {
            ledger.schedule_frontier(entry.occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&entry.occurrence, ObservationProducer::Apple));
        }
        let seal = ledger.seal_terminal(id, 7).unwrap();
        let sealed = reducer.apply_ledger_seal(&seal).unwrap();
        assert!(!bus.publish_revision(&sealed, &ledger).is_empty());
        let before_end = std::fs::read(&path).unwrap();
        assert!(
            bus.publish_ended_with_delivery_text(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
                None,
            )
            .is_none(),
            "a reducer seal is not the joined channel close"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before_end);
        assert!(!bus.writer.lock().unwrap().ended);

        bus.publish_channel_closed(seconds, None).unwrap();
        assert!(
            bus.publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::Unattempted,
            )
            .is_some()
        );
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-session.v1", "state": "sealed", "session_id": id,
        }));
        let raw = std::fs::read_to_string(&path).unwrap();
        let rows = crate::durable_bus_oracle::rows(&raw).unwrap();
        let flush = rows
            .iter()
            .position(|row| row["status"] == "session_ended")
            .unwrap();
        let evidence = &rows[flush - 2];
        let clean = &rows[flush - 1];
        assert_eq!(evidence["schema"], "codescribe.transcript-evidence.v1");
        assert_eq!(clean["schema"], "codescribe.transcript.v1");
        assert_eq!(evidence["status"], "transcript_sealed");
        assert_eq!(clean["status"], "transcript_sealed");
        assert_eq!(evidence["rendered_text"], sealed.rendered_text);
        assert_eq!(evidence["hold_until"], clean["hold_until"]);
        assert_eq!(evidence.get("hold_until").is_some(), seconds > 0);
        assert_eq!(
            rows.last().unwrap()["schema"],
            "codescribe.channel-session.v1"
        );
        assert!(
            rows[..flush - 2]
                .iter()
                .all(|row| row.get("hold_until").is_none())
        );
    }
}

#[test]
fn send_hold_cannot_stamp_dictation_or_non_channel_agent_rows() {
    for session in [
        unlatched_dictation("plain-send-hold"),
        session("plain-send-hold"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.jsonl");
        let bus = TranscriptBus::open_at(session, path.clone(), None).unwrap();
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("plain-send-hold");
        bus.publish_revision(&revision, &ledger);
        let before = std::fs::read(&path).unwrap();
        bus.publish_channel_closed(10, None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

/// Integrator (2026-10-01, take agent-channel-0-b351ea82): Fn+0 rows
/// reached only the shared bus while every follower read its channel bus.
#[test]
fn integrator_broadcast_bus_writes_every_row_to_every_destination() {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("transcript-events.jsonl");
    let one = dir.path().join("buses/channel-1.jsonl");
    let three = dir.path().join("buses/channel-3.jsonl");
    let mut broadcast = session("agent-channel-0-fanout");
    broadcast.audience = Some("*".to_string());
    broadcast.badge_only = true;
    let bus = TranscriptBus::open_with_paths(
        broadcast,
        shared.clone(),
        vec![one.clone(), three.clone(), one.clone(), shared.clone()],
    );
    bus.publish_started();
    let (ledger, _, revision) = committed_fixture("agent-channel-0-fanout");
    bus.publish_revision(&revision, &ledger);
    bus.record_channel_receipt(&serde_json::json!({
        "schema": "codescribe.channel-session.v1",
        "kind": "channel_session",
        "state": "sealed",
        "channel": "0",
        "session_id": "agent-channel-0-fanout",
    }));
    let read = |path: &Path| std::fs::read_to_string(path).unwrap_or_default();
    let shared_rows = read(&shared);
    assert!(shared_rows.lines().count() >= 2, "{shared_rows}");
    assert_eq!(read(&one), shared_rows, "channel 1 gets the same rows once");
    assert_eq!(
        read(&three),
        shared_rows,
        "channel 3 gets the same rows once"
    );
    assert!(shared_rows.contains("\"audience\":\"*\""), "{shared_rows}");
    assert!(shared_rows.contains("codescribe.channel-session.v1"));
}

#[test]
fn integrator_broken_broadcast_destination_leaves_the_others_writing() {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("transcript-events.jsonl");
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"file").unwrap();
    let broken = blocker.join("channel-2.jsonl");
    let good = dir.path().join("buses/channel-1.jsonl");
    let mut broadcast = session("agent-channel-0-broken");
    broadcast.audience = Some("*".to_string());
    broadcast.badge_only = true;
    let bus = TranscriptBus::open_with_paths(
        broadcast,
        shared.clone(),
        vec![broken.clone(), good.clone()],
    );
    bus.publish_started();
    let (ledger, _, revision) = committed_fixture("agent-channel-0-broken");
    bus.publish_revision(&revision, &ledger);
    let shared_rows = std::fs::read_to_string(&shared).unwrap();
    assert!(!shared_rows.is_empty());
    assert_eq!(std::fs::read_to_string(&good).unwrap(), shared_rows);
    assert!(!broken.exists());
}

/// Quiet delivery contract (Founder seal cc6c8248; only its reopen half was
/// replaced on 2026-10-26): between one start row per document and the
/// terminal seal, channel revisions stay off disk.
#[test]
fn channel_bus_carries_only_document_starts_and_the_terminal_seal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("channel.jsonl");
    let mut channel = session("channel-quiet");
    channel.audience = Some("Leon".to_string());
    channel.badge_only = true;
    let bus = TranscriptBus::open_at(channel, path.clone(), None).unwrap();
    bus.publish_started();

    let mut ledger = AcousticLedger::new();
    let mut reducer = TranscriptReducer::default();
    let calibration = EnergyCalibration::new("bus-fault-fixture", 1.0, 1);
    let mut occurrences = Vec::new();
    for (index, label) in ["Zażółć", "gęślą jaźń."].into_iter().enumerate() {
        let start = index as u64 * 16_000;
        let occurrence = OccurrenceIdentity::new("channel-quiet", 7, start, start + 16_000);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(start + 16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation = ObservationIdentity::new(
            ObservationProducer::Apple,
            index as u64 + 1,
            0,
            occurrence.clone(),
        );
        let receipt = ledger.admit(&observation, label);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .expect("live revision");
        // Each revision re-projects every document entry; the second
        // publish therefore re-offers document 0 and must not re-write it.
        let events = bus.publish_revision(&revision, &ledger);
        assert!(
            !events.is_empty(),
            "in-process projections survive quiet persistence"
        );
        occurrences.push(occurrence);
    }
    for occurrence in &occurrences {
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
    }
    let seal = ledger.seal_terminal("channel-quiet", 7).unwrap();
    let sealed = reducer.apply_ledger_seal(&seal).unwrap();
    assert!(!bus.publish_revision(&sealed, &ledger).is_empty());

    let raw = std::fs::read_to_string(&path).unwrap();
    let rows = crate::durable_bus_oracle::rows(&raw).unwrap();
    let action_counts = rows.iter().fold(
        std::collections::HashMap::<String, usize>::new(),
        |mut counts, row| {
            if let Some(action) = row.get("reducer_action").and_then(|value| value.as_str()) {
                *counts.entry(action.to_string()).or_default() += 1;
            }
            counts
        },
    );
    assert_eq!(
        action_counts.get("apply_ledger_decision"),
        Some(&2),
        "exactly one start row per document reaches the channel bus: {raw}"
    );
    assert_eq!(
        action_counts.get("record_ledger_terminal_seal"),
        Some(&2),
        "the terminal seal projects once per document entry: {raw}"
    );

    // A dictation session (no audience) keeps every projected row on disk:
    // the same two-admission sequence writes 1 + 2 = 3 decision rows.
    let plain_dir = tempfile::tempdir().unwrap();
    let plain_path = plain_dir.path().join("plain.jsonl");
    let plain = TranscriptBus::open_at(unlatched_dictation("plain-loud"), plain_path.clone(), None)
        .unwrap();
    let mut ledger = AcousticLedger::new();
    let mut reducer = TranscriptReducer::default();
    for (index, label) in ["Zażółć", "gęślą jaźń."].into_iter().enumerate() {
        let start = index as u64 * 16_000;
        let occurrence = OccurrenceIdentity::new("plain-loud", 7, start, start + 16_000);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(start + 16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, index as u64 + 1, 0, occurrence);
        let receipt = ledger.admit(&observation, label);
        let revision = reducer
            .apply_ledger_mutation(&ledger, &observation, &receipt)
            .expect("live revision");
        assert!(!plain.publish_revision(&revision, &ledger).is_empty());
    }
    let raw = std::fs::read_to_string(&plain_path).unwrap();
    let decisions = crate::durable_bus_oracle::rows(&raw)
        .unwrap()
        .iter()
        .filter(|row| row["reducer_action"] == "apply_ledger_decision")
        .count();
    assert_eq!(decisions, 3, "dictation keeps every revision row: {raw}");
}

fn inject_fault(bus: &TranscriptBus) -> Arc<Mutex<Fault>> {
    let fault = Arc::new(Mutex::new(Fault::default()));
    let mut writer = bus.writer.lock().unwrap();
    let inner = writer.file.take().expect("real file before injection");
    writer.file = Some(Box::new(FaultWriter {
        inner,
        fault: Arc::clone(&fault),
    }));
    fault
}

/// UNRUN W2: a lifecycle end preserves the real coverage receipt and never
/// sets the ledger-seal latch, even when an external sink accepted words.
#[test]
fn coverage_refusal_ends_once_without_sealing_the_book() {
    use codescribe_core::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };
    use codescribe_core::stt::tail_provider::TailSampleRange;
    let dir = tempfile::tempdir().unwrap();
    let bus = TranscriptBus::open_at(session("refused-book"), dir.path().join("bus.jsonl"), None)
        .unwrap();
    bus.publish_started();
    let (mut ledger, mut reducer, _) = committed_fixture("refused-book");
    let receipt = ledger.assess_seal_coverage(
        "refused-book",
        7,
        &AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("refused-book", 7),
            "capture_energy",
            AcousticAvailability::Observed {
                observed_samples: 64_000,
            },
            vec![TailSampleRange {
                session: "refused-book".into(),
                capture_epoch: 7,
                sample_start: 0,
                sample_end: 64_000,
            }],
        ),
        8_000,
    );
    assert!(ledger.record_seal_coverage(receipt.clone()));
    let revision = reducer.apply_seal_coverage(&receipt, None);
    let published = bus.publish_revision(&revision, &ledger);
    assert_eq!(published.len(), 2);
    assert!(bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));
    let mut forged = receipt.clone();
    forged.max_uncovered_samples += 1;
    let forged_revision = reducer.apply_seal_coverage(&forged, None);
    assert!(bus.publish_revision(&forged_revision, &ledger).is_empty());
    assert!(bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));
    let terminal = bus
        .publish_ended(
            TranscriptSessionEndReason::CoverageRefused,
            true,
            TranscriptDelivery::SinkAccepted,
        )
        .unwrap();
    assert!(!bus.writer.lock().unwrap().sealed);
    assert_eq!(terminal.phase, TranscriptProjectionPhase::CoverageRefused);
    assert_eq!(terminal.delivery, TranscriptDelivery::SinkAccepted);
    assert_eq!(terminal.rendered_text, revision.rendered_text);
    assert_eq!(
        terminal.seal_coverage,
        Some(ProjectedSealCoverageReceipt::from(&receipt))
    );
    assert!(terminal.lifecycle_terminal);
    assert!(terminal.can_retranscribe);
    assert!(
        bus.publish_ended(
            TranscriptSessionEndReason::Completed,
            true,
            TranscriptDelivery::SinkAccepted,
        )
        .is_none()
    );
    assert!(!bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));
}

/// The projection carries the typed availability reason, an explicitly
/// absent ratio, and survives a JSON round-trip byte-for-byte — including
/// the exact-receipt equality the recovery guard depends on.
/// Integrator (2026-10-01): the overlay places speech without words only
/// with the take's own clock; receipts written before the field read as no
/// clock, never as a guessed rate.
#[test]
fn integrator_coverage_projection_carries_the_capture_clock_and_reads_old_rows() {
    use codescribe_core::pipeline::acoustic_ledger::{SealCoverageReceipt, SealCoverageStatus};

    let receipt = SealCoverageReceipt {
        sample_rate_hz: Some(48_000),
        session_id: "clock".into(),
        capture_epoch: 1,
        speech_samples: 96_000,
        covered_samples: 33_600,
        uncovered_speech_ranges: Vec::new(),
        max_uncovered_samples: 62_400,
        incomplete_threshold_samples: 4_000,
        status: SealCoverageStatus::Incomplete,
        speech_producer: "capture_energy".into(),
        availability: "observed".into(),
        observed_samples: Some(96_000),
    };
    let projected = ProjectedSealCoverageReceipt::from(&receipt);
    assert_eq!(projected.sample_rate_hz, Some(48_000));
    let json = serde_json::to_string(&projected).unwrap();
    assert!(json.contains("\"sample_rate_hz\":48000"), "{json}");
    assert_eq!(
        serde_json::from_str::<ProjectedSealCoverageReceipt>(&json).unwrap(),
        projected
    );

    let mut old = projected.clone();
    old.sample_rate_hz = None;
    let old_row = serde_json::to_string(&old).unwrap();
    assert!(!old_row.contains("sample_rate_hz"), "{old_row}");
    assert_eq!(
        serde_json::from_str::<ProjectedSealCoverageReceipt>(&old_row)
            .unwrap()
            .sample_rate_hz,
        None
    );
}

#[test]
fn coverage_projection_round_trips_missing_ratio_and_exact_receipt_equality() {
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidenceGap, SealCoverageReceipt, SealCoverageStatus,
    };

    let unavailable = SealCoverageReceipt {
        sample_rate_hz: None,
        session_id: "round-trip".into(),
        capture_epoch: 7,
        speech_samples: 0,
        covered_samples: 0,
        uncovered_speech_ranges: Vec::new(),
        max_uncovered_samples: 0,
        incomplete_threshold_samples: 4_000,
        status: SealCoverageStatus::Unavailable(AcousticEvidenceGap::NotObserved),
        speech_producer: "capture_energy".into(),
        availability: "not_observed".into(),
        observed_samples: None,
    };
    let projected = ProjectedSealCoverageReceipt::from(&unavailable);
    assert_eq!(projected.status, "unavailable");
    assert_eq!(
        projected.unavailable_reason.as_deref(),
        Some("not_observed")
    );
    assert_eq!(projected.coverage_ratio, None);
    assert_eq!(projected.observed_samples, None);

    let json = serde_json::to_string(&projected).unwrap();
    assert!(
        !json.contains("coverage_ratio"),
        "an absent ratio is absent from the wire, never NaN and never 1.0: {json}"
    );
    assert!(!json.contains("NaN"), "{json}");
    assert!(
        json.contains("\"unavailable_reason\":\"not_observed\""),
        "{json}"
    );
    let decoded: ProjectedSealCoverageReceipt = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, projected);
    assert_eq!(
        decoded,
        ProjectedSealCoverageReceipt::from(&unavailable),
        "exact projected equality must survive the round-trip"
    );

    // Measured silence keeps a real 1.0 and no reason token.
    let silence = SealCoverageReceipt {
        status: SealCoverageStatus::Complete,
        availability: "observed".into(),
        observed_samples: Some(16_000),
        ..unavailable.clone()
    };
    let projected = ProjectedSealCoverageReceipt::from(&silence);
    assert_eq!(projected.status, "complete");
    assert_eq!(projected.unavailable_reason, None);
    assert_eq!(projected.coverage_ratio, Some(1.0));
    let decoded: ProjectedSealCoverageReceipt =
        serde_json::from_str(&serde_json::to_string(&projected).unwrap()).unwrap();
    assert_eq!(decoded, projected);
    assert_ne!(
        decoded,
        ProjectedSealCoverageReceipt::from(&unavailable),
        "measured silence and absent measurement must not project equal"
    );
}

/// A refused document is matched on the whole projected receipt. An
/// unavailable receipt must match its own projection exactly, and a forged
/// one — same session, different availability — must not.
#[test]
fn refused_document_match_survives_the_added_availability_fields() {
    use codescribe_core::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };

    let dir = tempfile::tempdir().unwrap();
    let bus = TranscriptBus::open_at(session("refused-avail"), dir.path().join("bus.jsonl"), None)
        .unwrap();
    bus.publish_started();
    let (mut ledger, mut reducer, _) = committed_fixture("refused-avail");
    // No observer measured this take: the words are committed, the extent
    // is unknown, and the ledger refuses on that ground alone.
    let receipt = ledger.assess_seal_coverage(
        "refused-avail",
        7,
        &AcousticSpeechEvidence::unavailable(
            CaptureEvidenceIdentity::new("refused-avail", 7),
            "capture_energy",
            AcousticAvailability::NotObserved,
        ),
        8_000,
    );
    assert!(!receipt.status.is_complete());
    assert_eq!(receipt.coverage_ratio(), None);
    assert!(ledger.record_seal_coverage(receipt.clone()));
    let revision = reducer.apply_seal_coverage(&receipt, None);
    assert_eq!(bus.publish_revision(&revision, &ledger).len(), 2);
    assert!(bus.matches_refused_document(&fixture_refusal(&receipt), &revision.rendered_text));

    let mut forged = receipt.clone();
    forged.availability = "observed".into();
    forged.observed_samples = Some(64_000);
    assert!(
        !bus.matches_refused_document(&fixture_refusal(&forged), &revision.rendered_text),
        "a forged availability must not match the published projection"
    );
    assert!(
        !bus.matches_refused_document(&fixture_refusal(&receipt), "inne słowa"),
        "the document text is still part of the match"
    );
    let mut foreign = receipt.clone();
    foreign.session_id = "successor".into();
    assert!(
        !bus.matches_refused_document(&fixture_refusal(&foreign), &revision.rendered_text),
        "a foreign session never matches this bus"
    );
}

/// Synthetic calibrated evidence admitted by the actual ledger and reducer.
/// Two entries catch an append failure that incorrectly breaks the loop.
#[test]
fn finality_refusal_matches_complete_or_absent_coverage_without_inventing_a_seal() {
    for measured in [false, true] {
        let id = if measured {
            "complete-no-seal"
        } else {
            "absent-no-seal"
        };
        let (mut ledger, mut reducer, mut revision) = committed_fixture(id);
        let dir = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(session(id), dir.path().join("bus.jsonl"), None).unwrap();
        bus.publish_started();
        if measured {
            let coverage = SealCoverageReceipt {
                sample_rate_hz: None,
                session_id: id.into(),
                capture_epoch: 7,
                speech_samples: 32_000,
                covered_samples: 32_000,
                uncovered_speech_ranges: vec![],
                max_uncovered_samples: 0,
                incomplete_threshold_samples: 4_000,
                status: codescribe_core::pipeline::acoustic_ledger::SealCoverageStatus::Complete,
                speech_producer: "synthetic_test".into(),
                availability: "observed".into(),
                observed_samples: Some(32_000),
            };
            assert!(ledger.record_seal_coverage(coverage.clone()));
            revision = reducer.apply_seal_coverage(&coverage, None);
        }
        assert!(!bus.publish_revision(&revision, &ledger).is_empty());
        let refusal = ledger.terminal_finality(id, 7).into_refusal().unwrap();
        assert_eq!(refusal.coverage().is_some(), measured);
        assert!(bus.matches_refused_document(&refusal, &revision.rendered_text));
        assert!(!bus.matches_refused_document(&refusal, "different words"));
        let foreign = ledger.terminal_finality(id, 8).into_refusal().unwrap();
        assert!(!bus.matches_refused_document(&foreign, &revision.rendered_text));
        let missing = AcousticLedger::new()
            .terminal_finality(id, 7)
            .into_refusal()
            .unwrap();
        assert_eq!(
            bus.matches_refused_document(&missing, &revision.rendered_text),
            !measured
        );
        bus.publish_ended(
            TranscriptSessionEndReason::CoverageRefused,
            true,
            TranscriptDelivery::SinkAccepted,
        )
        .unwrap();
        assert!(!bus.matches_refused_document(&refusal, &revision.rendered_text));
        assert!(!bus.writer.lock().unwrap().sealed);
    }
}

fn committed_fixture(id: &str) -> (AcousticLedger, TranscriptReducer, TranscriptRevision) {
    committed_fixture_with_first_label(id, "Zażółć")
}

fn committed_fixture_with_first_label(
    id: &str,
    first_label: &str,
) -> (AcousticLedger, TranscriptReducer, TranscriptRevision) {
    let mut ledger = AcousticLedger::new();
    let mut reducer = TranscriptReducer::default();
    let calibration = EnergyCalibration::new("bus-fault-fixture", 1.0, 1);
    let mut revision = None;
    for (index, label) in [first_label, "gęślą\n jaźń."].into_iter().enumerate() {
        let start = index as u64 * 16_000;
        let occurrence = OccurrenceIdentity::new(id, 7, start, start + 16_000);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(start),
            vad_close_sample: Some(start + 16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation =
            ObservationIdentity::new(ObservationProducer::Apple, index as u64 + 1, 0, occurrence);
        let receipt = ledger.admit(&observation, label);
        revision = reducer.apply_ledger_mutation(&ledger, &observation, &receipt);
        assert!(revision.is_some());
    }
    (ledger, reducer, revision.unwrap())
}

fn assert_committed(
    events: &[TranscriptBusEvidenceEvent],
    revision: &TranscriptRevision,
    id: &str,
) {
    assert_eq!(events.len(), 2);
    assert!(!revision.rendered_text.is_empty());
    for (event, entry) in events.iter().zip(&revision.entries) {
        assert_eq!(event.session_id, id);
        assert_eq!(
            event.rendered_text.as_bytes(),
            revision.rendered_text.as_bytes()
        );
        assert_eq!(event.occurrence_session_id, entry.occurrence.session);
        assert_eq!(event.capture_epoch, entry.occurrence.capture_epoch);
        assert_eq!(event.sample_start, entry.occurrence.sample_start);
        assert_eq!(event.sample_end, entry.occurrence.sample_end);
        let receipt = &event.acoustic_receipts[0];
        assert_eq!(receipt.session_id, id);
        assert_eq!(receipt.word_evidence_receipts, entry.word_evidence_receipts);
        assert_eq!(
            receipt.layer_decision_receipts,
            entry.layer_decision_receipts
        );
        assert_eq!(receipt.seal_receipt, entry.seal_receipt);
        assert_eq!(receipt.manual_edit_receipt, entry.manual_edit_receipt);
        assert_eq!(event.delivery, TranscriptDelivery::Unattempted);
        assert!(!event.lifecycle_terminal);
    }
}

fn end_once(bus: &TranscriptBus, expected: &str) -> TranscriptBusEvidenceEvent {
    let terminal = bus
        .publish_ended(
            TranscriptSessionEndReason::Completed,
            true,
            TranscriptDelivery::ComposerPending,
        )
        .expect("live terminal survives persistence failure");
    assert_eq!(terminal.rendered_text.as_bytes(), expected.as_bytes());
    assert!(terminal.lifecycle_terminal);
    assert!(terminal.terminal);
    assert!(terminal.can_copy);
    assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
    assert!(
        bus.publish_ended(
            TranscriptSessionEndReason::Completed,
            true,
            TranscriptDelivery::SinkAccepted,
        )
        .is_none()
    );
    assert_eq!(
        bus.writer.lock().unwrap().last_projection.as_ref(),
        Some(&terminal)
    );
    terminal
}

#[test]
fn open_failure_keeps_the_production_bus_and_exact_terminal_text() {
    let _serial = persistence_log_serial();
    let temp = tempfile::tempdir().unwrap();
    // Opening a directory as a file fails without touching user permissions.
    let bus = TranscriptBus::open_with_path(session("open-fault"), temp.path().to_path_buf());
    assert!(bus.writer.lock().unwrap().file.is_none());
    bus.publish_started();
    let (ledger, _, revision) = committed_fixture("open-fault");
    let events = bus.publish_revision(&revision, &ledger);
    assert_committed(&events, &revision, "open-fault");
    let terminal = end_once(&bus, &revision.rendered_text);
    assert_eq!(terminal.session_id, "open-fault");
    assert_eq!(terminal.acoustic_receipts, events[1].acoustic_receipts);
    assert_eq!(terminal.sequence, 4);
}

#[test]
fn persistence_failure_is_logged_without_logging_the_committed_document() {
    // End-to-end proof that the production `warn!` still fires with the
    // exact contract content. The serial guard keeps sibling tests from
    // hitting the same callsite while this scoped subscriber's interest
    // is cached (see `PERSISTENCE_LOG_SERIAL`).
    let _serial = persistence_log_serial();
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("diagnostics.log");
    let log_file = std::fs::File::create(&log_path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || log_file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        // This test is the only one running that can hit the warn
        // callsite, so recomputing interest with this dispatcher
        // registered settles the cache deterministically.
        tracing::callsite::rebuild_interest_cache();
        let bus =
            TranscriptBus::open_with_path(session("diagnostic-fault"), temp.path().to_path_buf());
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("diagnostic-fault");
        bus.publish_revision(&revision, &ledger);
        end_once(&bus, &revision.rendered_text);
    });
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(log.contains("diagnostic-fault"));
    assert!(log.contains("disabled_for_session"));
    assert!(log.contains("without append proof"));
    assert!(!log.contains("Zażółć"));
}

#[test]
fn persistence_failure_reaches_the_diagnostic_sink_without_the_document() {
    // The deterministic twin of the tracing capture above: the production
    // failure report is observed through the injected sink, so the
    // assertion cannot depend on tracing's process-global callsite state.
    let _serial = persistence_log_serial();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let mut bus = TranscriptBus::open_at(session("sink-fault"), path.clone(), None).unwrap();
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&diagnostics);
    bus.set_persistence_diagnostic_sink(Arc::new(move |diagnostic| {
        observed.lock().unwrap().push((
            diagnostic.session_id.to_string(),
            diagnostic.file.to_string(),
            diagnostic.error.to_string(),
        ));
    }));
    let fault = inject_fault(&bus);
    fault.lock().unwrap().remaining = Some(0);
    bus.publish_started();
    let (ledger, _, revision) = committed_fixture("sink-fault");
    let events = bus.publish_revision(&revision, &ledger);
    assert_committed(&events, &revision, "sink-fault");
    end_once(&bus, &revision.rendered_text);
    // The first failed append disables persistence for the session, so
    // exactly one diagnostic is reported.
    let diagnostics = diagnostics.lock().unwrap();
    assert_eq!(diagnostics.len(), 1, "diagnostics: {diagnostics:?}");
    let (session_id, file, error) = &diagnostics[0];
    assert_eq!(session_id, "sink-fault");
    assert_eq!(file, "events.jsonl");
    assert!(error.contains("controlled append failure"));
    for payload in [session_id, file, error] {
        assert!(!payload.contains("Zażółć"));
    }
}

#[test]
fn failed_start_and_revision_writes_do_not_suppress_committed_entries() {
    let _serial = persistence_log_serial();
    for fail_start in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(session("write-fault"), path.clone(), None).unwrap();
        let fault = inject_fault(&bus);
        if fail_start {
            fault.lock().unwrap().remaining = Some(0);
        }
        bus.publish_started();
        bus.publish_started();
        fault.lock().unwrap().remaining = Some(0);
        let (ledger, _, revision) = committed_fixture("write-fault");
        let events = bus.publish_revision(&revision, &ledger);
        assert_committed(&events, &revision, "write-fault");
        assert_eq!(events[0].sequence, 2);
        assert_eq!(events[1].sequence, 3);
        end_once(&bus, &revision.rendered_text);
        assert!(bus.writer.lock().unwrap().file.is_none());
        assert_eq!(
            std::fs::read_to_string(path).unwrap().lines().count(),
            usize::from(!fail_start)
        );
    }
}

#[test]
fn terminal_write_and_flush_failures_keep_one_delivery_obligation() {
    let _serial = persistence_log_serial();
    for flush in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(session("end-fault"), path.clone(), None).unwrap();
        let fault = inject_fault(&bus);
        bus.publish_started();
        let (ledger, _, revision) = committed_fixture("end-fault");
        let events = bus.publish_revision(&revision, &ledger);
        assert_committed(&events, &revision, "end-fault");
        if flush {
            fault.lock().unwrap().flush = true;
        } else {
            fault.lock().unwrap().remaining = Some(0);
        }
        let terminal = end_once(&bus, &revision.rendered_text);
        assert_eq!(terminal.sequence, 4);
        assert_eq!(terminal.acoustic_receipts, events[1].acoustic_receipts);
        assert!(bus.writer.lock().unwrap().file.is_none());
        assert_eq!(
            crate::durable_bus_oracle::rows(&crate::durable_bus_oracle::logical_text(&path))
                .unwrap()
                .len(),
            if flush { 4 } else { 3 }
        );
    }
}

#[test]
fn prefix_failure_is_not_retried_and_new_sessions_do_not_append_to_it() {
    let _serial = persistence_log_serial();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bus = TranscriptBus::open_at(session("prefix"), path.clone(), None).unwrap();
    let fault = inject_fault(&bus);
    bus.publish_started();
    let before = std::fs::read(&path).unwrap();
    fault.lock().unwrap().remaining = Some(19);
    let (ledger, _, revision) = committed_fixture("prefix");
    assert_committed(
        &bus.publish_revision(&revision, &ledger),
        &revision,
        "prefix",
    );
    let partial = std::fs::read(&path).unwrap();
    assert_eq!(partial.len(), before.len() + 19);
    assert!(!partial.ends_with(b"\n"));
    let writes = fault.lock().unwrap().writes;
    fault.lock().unwrap().remaining = None;
    end_once(&bus, &revision.rendered_text);
    assert_eq!(fault.lock().unwrap().writes, writes);
    assert_eq!(std::fs::read(&path).unwrap(), partial);

    let next = TranscriptBus::open_with_path(session("next"), path.clone());
    next.publish_started();
    let empty = next
        .publish_ended(
            TranscriptSessionEndReason::Completed,
            false,
            TranscriptDelivery::Unattempted,
        )
        .unwrap();
    assert!(empty.rendered_text.is_empty());
    assert_eq!(empty.session_id, "next");
    assert_eq!(empty.phase, TranscriptProjectionPhase::NoSpeech);
    assert_eq!(std::fs::read(&path).unwrap(), partial);

    // A separate explicit journal restores persistence; a stale generation
    // manifest is never silently rewritten to bless a replacement inode.
    let recovered_path = temp.path().join("recovered.jsonl");
    let recovered = TranscriptBus::open_with_path(session("recovered"), recovered_path.clone());
    recovered.publish_started();
    let (ledger, _, revision) = committed_fixture("recovered");
    assert_committed(
        &recovered.publish_revision(&revision, &ledger),
        &revision,
        "recovered",
    );
    end_once(&recovered, &revision.rendered_text);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        partial,
        "failed original journal remains exact"
    );
    let rows =
        crate::durable_bus_oracle::rows(&crate::durable_bus_oracle::logical_text(&recovered_path))
            .unwrap();
    assert_eq!(rows.len(), 4);
    for row in rows {
        assert_eq!(row["session_id"], "recovered");
    }
}

#[test]
fn uncertain_flush_is_not_replayed_when_sink_recovers() {
    let _serial = persistence_log_serial();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bus = TranscriptBus::open_at(session("flush"), path.clone(), None).unwrap();
    let fault = inject_fault(&bus);
    bus.publish_started();
    fault.lock().unwrap().flush = true;
    let (ledger, _, revision) = committed_fixture("flush");
    assert_committed(
        &bus.publish_revision(&revision, &ledger),
        &revision,
        "flush",
    );
    let uncertain = std::fs::read(&path).unwrap();
    let writes = fault.lock().unwrap().writes;
    let flushes = fault.lock().unwrap().flushes;
    fault.lock().unwrap().flush = false;
    end_once(&bus, &revision.rendered_text);
    assert_eq!(std::fs::read(&path).unwrap(), uncertain);
    assert_eq!(fault.lock().unwrap().writes, writes);
    assert_eq!(fault.lock().unwrap().flushes, flushes);
    let next = TranscriptBus::open_with_path(session("after-flush"), path.clone());
    next.publish_started();
    let terminal = next
        .publish_ended(
            TranscriptSessionEndReason::Completed,
            false,
            TranscriptDelivery::Unattempted,
        )
        .unwrap();
    assert!(terminal.rendered_text.is_empty());
    let rows = std::fs::read_to_string(path).unwrap();
    assert_eq!(rows.lines().count(), 4);
    for line in rows.lines() {
        let _: serde_json::Value = serde_json::from_str(line).unwrap();
    }
}

#[test]
fn terminal_user_revision_survives_failure_without_reopening_delivery() {
    let _serial = persistence_log_serial();
    for failing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let bus = TranscriptBus::open_at(session("edit"), path.clone(), None).unwrap();
        let fault = inject_fault(&bus);
        bus.publish_started();
        let (mut ledger, mut reducer, revision) = committed_fixture("edit");
        assert_committed(&bus.publish_revision(&revision, &ledger), &revision, "edit");
        for occurrence in ledger.occurrences().cloned().collect::<Vec<_>>() {
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
        }
        let seal = ledger.seal_terminal("edit", 7).unwrap();
        let sealed = reducer.apply_ledger_seal(&seal).unwrap();
        bus.publish_revision(&sealed, &ledger);
        end_once(&bus, &sealed.rendered_text);
        assert!(bus.publish_revision(&revision, &ledger).is_empty());
        if failing {
            fault.lock().unwrap().remaining = Some(0);
        }
        let edited = reducer
            .apply_user_revision(
                &mut ledger,
                &UserRevisionIntent {
                    session_id: "edit".to_string(),
                    source_revision: sealed.revision,
                    rendered_text: "Poprawione — dokładne bajty.\nDrugi wiersz.".to_string(),
                    provenance: DocumentRevisionProvenance::UserEdit,
                },
            )
            .unwrap();
        let events = bus.publish_revision(&edited, &ledger);
        assert_committed(&events, &edited, "edit");
        assert!(
            events
                .iter()
                .all(|event| event.terminal && event.reducer_action == "apply_manual_edit")
        );
        assert!(
            bus.publish_ended(
                TranscriptSessionEndReason::Completed,
                true,
                TranscriptDelivery::ComposerPending,
            )
            .is_none()
        );
        assert_eq!(
            bus.writer.lock().unwrap().last_projection.as_ref(),
            events.last()
        );
        assert_eq!(
            crate::durable_bus_oracle::rows(&crate::durable_bus_oracle::logical_text(&path))
                .unwrap()
                .len(),
            if failing { 6 } else { 8 }
        );
    }
}

#[test]
fn evidence_v1_without_additive_seal_receipts_still_decodes() {
    let legacy = serde_json::json!({
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 12,
        "emitted_at": "2026-08-27T22:36:00Z",
        "session_id": "b2b3b95e-4ddc-4845-a5ce-149b21eec166",
        "mode": "dictation",
        "reducer_revision": 7,
        "reducer_action": "record_ledger_terminal_seal",
        "occurrence_session_id": "b2b3b95e-4ddc-4845-a5ce-149b21eec166",
        "capture_epoch": 1,
        "sample_start": 304819,
        "sample_end": 869376,
        "document_index": 0,
        "label": "Kurde",
        "rendered_text": "Kurde",
        "acoustic_receipts": []
    });
    let mut decoded: TranscriptBusEvidenceEvent = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.phase, TranscriptProjectionPhase::Listening);
    assert!(!decoded.can_paste);
    assert!(!decoded.can_insert);
    assert!(!decoded.can_copy);
    assert!(!decoded.can_retranscribe);
    assert!(!decoded.can_format);
    assert!(!decoded.terminal);
    assert!(decoded.seal_coverage.is_none());
    assert!(decoded.comparison.is_none());

    decoded.phase = TranscriptProjectionPhase::Formatted;
    decoded.can_paste = true;
    decoded.can_insert = true;
    decoded.can_copy = true;
    decoded.can_retranscribe = true;
    decoded.can_format = true;
    decoded.terminal = true;
    let encoded = serde_json::to_string(&decoded).unwrap();
    let round_trip: TranscriptBusEvidenceEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(round_trip, decoded);
}

#[test]
fn bus_flushes_session_lifecycle_privately_without_text_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bus = TranscriptBus::open_at(session("session-agent"), path.clone(), Some(48_000)).unwrap();

    bus.publish_started();
    bus.publish_started();

    let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].status, "session_started");
    assert_eq!(lines[0].sequence, 1);
    assert!(lines[0].text.is_empty());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

/// Lines written before 2026-10-24 may end with `chord_prefix`
/// (docs/TRANSCRIPT_BUS.md); a typed reader still decodes them.
#[test]
fn a_historical_chord_prefix_terminal_line_still_decodes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("historical-chord-prefix.jsonl");
    let bus =
        TranscriptBus::open_at(session("historical-chord-prefix"), path.clone(), None).unwrap();
    bus.publish_started();
    bus.publish_ended(
        TranscriptSessionEndReason::Completed,
        false,
        TranscriptDelivery::Unattempted,
    )
    .expect("a started session ends once");

    let written = std::fs::read_to_string(&path).unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(written.lines().last().unwrap()).unwrap();
    raw["end_reason"] = serde_json::Value::from("chord_prefix");
    let historical: CleanTranscriptEvent =
        serde_json::from_str(&raw.to_string()).expect("a pre-2026-10-24 line decodes");
    assert_eq!(historical.status, "session_ended");
    assert_eq!(
        historical.end_reason,
        Some(TranscriptSessionEndReason::ChordPrefix)
    );
}

/// `session_ended` is the text-free terminal lifecycle line: written once,
/// only after `session_started`, never for a session that never started.
#[test]
fn bus_ends_a_started_session_exactly_once_and_never_an_unstarted_one() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let session = unlatched_dictation("session-ended");

    let never_started = TranscriptBus::open_at(session.clone(), path.clone(), None).unwrap();
    let never_started_terminal = never_started.publish_ended(
        TranscriptSessionEndReason::StartSuperseded,
        false,
        TranscriptDelivery::Unattempted,
    );
    assert!(never_started_terminal.is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 0);

    let bus = TranscriptBus::open_at(session, path.clone(), None).unwrap();
    bus.publish_started();
    let terminal = bus
        .publish_ended(
            TranscriptSessionEndReason::Completed,
            false,
            TranscriptDelivery::SinkAccepted,
        )
        .expect("started session must produce one terminal projection");
    let duplicate_terminal = bus.publish_ended(
        TranscriptSessionEndReason::StartFailed,
        false,
        TranscriptDelivery::Unattempted,
    );
    assert!(duplicate_terminal.is_none());
    assert_eq!(terminal.reducer_action, "session_ended");
    assert_eq!(terminal.phase, TranscriptProjectionPhase::NoSpeech);
    assert!(terminal.terminal);
    assert!(terminal.rendered_text.is_empty());

    let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].status, "session_started");
    assert_eq!(lines[1].status, "session_ended");
    assert_eq!(lines[1].sequence, 2);
    assert_eq!(lines[1].session_id, "session-ended");
    assert!(lines[1].text.is_empty());
    assert_eq!(lines[0].end_reason, None);
    // The first terminal wins; a later call with another reason is a no-op.
    assert_eq!(
        lines[1].end_reason,
        Some(TranscriptSessionEndReason::Completed)
    );
}

/// Acceptance: only the lifecycle line states a destination, and it states
/// the controller's disposition verbatim.
///
/// The failure this pins is the one that made a delivery contract necessary
/// at all: an observer that has to infer "where did this go" from a phase or
/// a display label will eventually infer it wrong.
#[test]
fn only_the_lifecycle_line_carries_a_delivery_disposition() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bus = TranscriptBus::open_at(session("session-delivery"), path, None).unwrap();
    bus.publish_started();

    let terminal = bus
        .publish_ended(
            TranscriptSessionEndReason::Completed,
            false,
            TranscriptDelivery::ComposerPending,
        )
        .expect("a started session ends with one terminal projection");

    assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
    assert!(
        terminal.lifecycle_terminal,
        "the line that ends the session must say so"
    );
}

/// A take that never delivered says `Unattempted`, which is not the same
/// claim as "delivered nowhere on purpose" and not the same as a failure.
#[test]
fn a_take_with_no_delivery_attempt_claims_no_destination() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    let bus =
        TranscriptBus::open_at(unlatched_dictation("session-unattempted"), path, None).unwrap();
    bus.publish_started();

    let terminal = bus
        .publish_ended(
            TranscriptSessionEndReason::StartFailed,
            false,
            TranscriptDelivery::Unattempted,
        )
        .expect("a started session ends with one terminal projection");

    assert_eq!(terminal.delivery, TranscriptDelivery::Unattempted);
}

/// A legacy row without the additive fields decodes to the honest defaults:
/// no destination claimed, and not a lifecycle line.
#[test]
fn legacy_rows_default_to_no_destination_and_no_lifecycle_claim() {
    let legacy = serde_json::json!({
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 1,
        "emitted_at": "2026-09-09T20:00:00Z",
        "session_id": "legacy",
        "mode": "dictation",
        "reducer_revision": 1,
        "reducer_action": "apply_ledger_decision",
        "occurrence_session_id": "legacy",
        "capture_epoch": 1,
        "sample_start": 0,
        "sample_end": 16000,
        "document_index": 0,
        "label": "Kurde",
        "rendered_text": "Kurde",
        "acoustic_receipts": []
    });
    let decoded: TranscriptBusEvidenceEvent = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.delivery, TranscriptDelivery::Unattempted);
    assert!(!decoded.lifecycle_terminal);
}

// Recovery falsifiers: these contracts are intentionally UNRUN under W2.
// Preserve predecessor negative controls across the publication recovery.
#[test]
fn incremental_bus_refuses_rendered_bytes_not_bound_to_the_shaping_receipt() {
    let (mut ledger, mut reducer, _) =
        committed_fixture_with_first_label("shaping-bytes", "zażółć");
    let occurrence = OccurrenceIdentity::new("shaping-bytes", 7, 0, 16_000);
    ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    let seal = ledger.seal(&occurrence).unwrap().clone();
    reducer.apply_ledger_seal(&seal).unwrap();
    let mut revision = reducer
        .apply_incremental_shaping(&mut ledger, &occurrence)
        .unwrap();
    revision.rendered_text = "Unrelated replacement without source authority.".to_string();
    let temp = tempfile::tempdir().unwrap();
    let bus = TranscriptBus::open_at(
        session("shaping-bytes"),
        temp.path().join("bus.jsonl"),
        None,
    )
    .unwrap();
    assert!(bus.publish_revision(&revision, &ledger).is_empty());
    assert!(bus.writer.lock().unwrap().last_projection.is_none());
}

#[test]
fn incremental_bus_refuses_a_shaping_receipt_absent_from_the_ledger() {
    let (mut ledger, mut reducer, _) =
        committed_fixture_with_first_label("shaping-forgery", "zażółć");
    let occurrence = OccurrenceIdentity::new("shaping-forgery", 7, 0, 16_000);
    ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    let seal = ledger.seal(&occurrence).unwrap().clone();
    reducer.apply_ledger_seal(&seal).unwrap();
    let mut revision = reducer
        .apply_incremental_shaping(&mut ledger, &occurrence)
        .unwrap();
    let crate::presentation::emitter::ReducerAction::ApplyIncrementalShaping { receipt } =
        &mut revision.action
    else {
        panic!("the real reducer must return a shaping action");
    };
    receipt.receipt_id = "light-plus-incremental-not-minted".to_string();
    let temp = tempfile::tempdir().unwrap();
    let bus = TranscriptBus::open_at(
        session("shaping-forgery"),
        temp.path().join("bus.jsonl"),
        None,
    )
    .unwrap();
    assert!(bus.publish_revision(&revision, &ledger).is_empty());
    assert!(bus.writer.lock().unwrap().last_projection.is_none());
}

/// A live per-occurrence shape is observed exactly like any other committed
/// revision — same evidence rows, same rendered document, same receipts —
/// but it must not borrow the vocabulary of an edit or of a terminal. The
/// take is still being spoken, so the book stays open and the projection
/// stays `listening`.
#[test]
fn an_incremental_shaping_publishes_a_listening_revision_without_closing_the_book() {
    let (mut ledger, mut reducer, committed) =
        committed_fixture_with_first_label("shaping-bus", "zażółć");
    let occurrence = OccurrenceIdentity::new("shaping-bus", 7, 0, 16_000);
    ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    let seal = ledger
        .seal(&occurrence)
        .expect("a closed qualified occurrence seals")
        .clone();
    assert!(seal.is_occurrence_seal());
    let sealed = reducer
        .apply_ledger_seal(&seal)
        .expect("the occurrence seal projects");

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("incremental-shaping.jsonl");
    let bus = TranscriptBus::open_at(session("shaping-bus"), path, None).unwrap();
    bus.publish_started();
    assert_eq!(bus.publish_revision(&sealed, &ledger).len(), 2);

    let shaping = reducer
        .apply_incremental_shaping(&mut ledger, &occurrence)
        .expect("a sealed committed occurrence shapes");
    let events = bus.publish_revision(&shaping, &ledger);

    assert_eq!(
        events.len(),
        2,
        "the whole document is projected, not only the shaped span"
    );
    for event in &events {
        assert_eq!(event.reducer_action, "apply_incremental_shaping");
        assert_eq!(event.phase, TranscriptProjectionPhase::Listening);
        assert!(!event.terminal, "a shape is not a terminal revision");
        assert!(!event.lifecycle_terminal, "a shape never ends the session");
        assert_eq!(event.delivery, TranscriptDelivery::Unattempted);
        assert_eq!(event.rendered_text, shaping.rendered_text);
        assert!(
            event.acoustic_receipts[0].manual_edit_receipt.is_none(),
            "a shape must not project as a human correction"
        );
    }
    // Only the shaped occurrence changed; the open one is byte-exact.
    assert_eq!(committed.rendered_text, "zażółć gęślą\n jaźń.");
    assert_eq!(shaping.rendered_text, "Zażółć gęślą\n jaźń.");
    assert_eq!(events[0].label, "zażółć", "the spoken label is unchanged");

    assert!(
        !bus.writer.lock().unwrap().sealed,
        "an occurrence-level revision must never close the committed book"
    );
}
#[test]
fn retained_shaping_authenticates_the_complete_ordered_snapshot_or_emits_nothing() {
    let (mut ledger, mut reducer, _) =
        committed_fixture_with_first_label("retained-proof", "zażółć");
    let first = OccurrenceIdentity::new("retained-proof", 7, 0, 16_000);
    let second = OccurrenceIdentity::new("retained-proof", 7, 16_000, 32_000);
    for occurrence in [&first, &second] {
        ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(ledger.note_frontier_return(occurrence, ObservationProducer::Apple));
        let seal = ledger.seal(occurrence).unwrap().clone();
        reducer.apply_ledger_seal(&seal).unwrap();
    }
    let shaped = reducer
        .apply_incremental_shaping(&mut ledger, &first)
        .unwrap();
    let revision = reducer.record_context_marker(0, "context").unwrap();
    assert!(revision.revision > shaped.revision);
    let temp = tempfile::tempdir().unwrap();
    let bus =
        TranscriptBus::open_at(session("retained-proof"), temp.path().join("bus"), None).unwrap();
    let mut candidates = Vec::new();
    let mut altered = revision.clone();
    altered.rendered_text.push_str(" injected");
    candidates.push(altered);
    let mut missing = revision.clone();
    missing.entries[0].presentation_receipt = None;
    candidates.push(missing);
    let mut fabricated = revision.clone();
    fabricated.entries[0]
        .presentation_receipt
        .as_mut()
        .unwrap()
        .receipt_id
        .push_str("-forged");
    candidates.push(fabricated);
    let mut stale_source = revision.clone();
    stale_source.entries[0]
        .presentation_receipt
        .as_mut()
        .unwrap()
        .source_revision += 1;
    candidates.push(stale_source);
    let mut reordered = revision.clone();
    reordered.entries.swap(0, 1);
    candidates.push(reordered);
    let mut omitted = revision.clone();
    omitted.entries.pop();
    candidates.push(omitted);
    let mut foreign = revision.clone();
    foreign.entries[1].occurrence.session = "foreign".to_string();
    candidates.push(foreign);
    for candidate in candidates {
        assert!(bus.publish_revision(&candidate, &ledger).is_empty());
        let writer = bus.writer.lock().unwrap();
        assert_eq!(writer.sequence, 0);
        assert!(writer.last_projection.is_none());
    }
    // Same acoustic labels/geometry/seal IDs, but no minted shaping:
    // a valid private snapshot still needs the exact live ledger receipt.
    let (mut without_shaping, _, _) =
        committed_fixture_with_first_label("retained-proof", "zażółć");
    for occurrence in [&first, &second] {
        without_shaping.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
        assert!(without_shaping.note_frontier_return(occurrence, ObservationProducer::Apple));
        without_shaping.seal(occurrence).unwrap();
    }
    assert!(bus.publish_revision(&revision, &without_shaping).is_empty());
    let rows = bus.publish_revision(&revision, &ledger);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].acoustic_receipts[0]
            .presentation_receipt
            .as_ref()
            .unwrap()
            .revision,
        shaped.revision,
        "retained provenance names its original revision"
    );
    assert!(rows[1].acoustic_receipts[0].presentation_receipt.is_none());
    let bytes = std::fs::read(temp.path().join("bus")).unwrap();
    assert!(bus.publish_revision(&revision, &ledger).is_empty());
    assert_eq!(std::fs::read(temp.path().join("bus")).unwrap(), bytes);

    let manual = ObservationIdentity::new(ObservationProducer::ManualHuman, 99, 0, first);
    assert!(ledger.admit(&manual, "replacement").grants_mutation());
    let fresh_bus =
        TranscriptBus::open_at(session("retained-proof"), temp.path().join("stale"), None).unwrap();
    assert!(
        fresh_bus.publish_revision(&revision, &ledger).is_empty(),
        "an intact old snapshot is still refused after its acoustic source changes"
    );
}

#[test]
fn serialized_projection_absence_and_malformed_proof_never_mint_authority() {
    let (mut ledger, mut reducer, plain) =
        committed_fixture_with_first_label("serialized-shape", "zażółć");
    let temp = tempfile::tempdir().unwrap();
    let bus =
        TranscriptBus::open_at(session("serialized-shape"), temp.path().join("bus"), None).unwrap();
    let plain_rows = bus.publish_revision(&plain, &ledger);
    let old = serde_json::to_value(&plain_rows[0]).unwrap();
    assert!(
        old["acoustic_receipts"][0]
            .get("presentation_receipt")
            .is_none()
    );
    let old: TranscriptBusEvidenceEvent = serde_json::from_value(old).unwrap();
    assert!(old.acoustic_receipts[0].presentation_receipt.is_none());
    let occurrence = OccurrenceIdentity::new("serialized-shape", 7, 0, 16_000);
    ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
    assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
    let seal = ledger.seal(&occurrence).unwrap().clone();
    reducer.apply_ledger_seal(&seal).unwrap();
    let revision = reducer
        .apply_incremental_shaping(&mut ledger, &occurrence)
        .unwrap();
    let rows = bus.publish_revision(&revision, &ledger);
    let encoded = serde_json::to_value(&rows[0]).unwrap();
    let decoded: TranscriptBusEvidenceEvent = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(
        decoded.acoustic_receipts[0].presentation_receipt,
        rows[0].acoustic_receipts[0].presentation_receipt
    );
    for field in [
        "source_revision",
        "left_context",
        "left_context_sha256",
        "shaped_text",
    ] {
        let mut incomplete = encoded.clone();
        incomplete["acoustic_receipts"][0]["presentation_receipt"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<TranscriptBusEvidenceEvent>(incomplete).is_err());
    }
    // A decoded observer record is not a TranscriptRevision capability.
    // Rewriting it never enters publish_revision or creates a ledger receipt.
    let count = ledger.incremental_shapings().len();
    let mut fabricated = encoded;
    fabricated["acoustic_receipts"][0]["presentation_receipt"]["receipt_id"] = "not-minted".into();
    let decoded: TranscriptBusEvidenceEvent = serde_json::from_value(fabricated).unwrap();
    assert!(!ledger.incremental_shapings().iter().any(|receipt| {
        receipt.receipt_id
            == decoded.acoustic_receipts[0]
                .presentation_receipt
                .as_ref()
                .unwrap()
                .receipt_id
    }));
    assert_eq!(ledger.incremental_shapings().len(), count);
}

/// A covered overlap receipt is conserved and visible to the reducer, and
/// the bus still publishes one committed occurrence.
#[test]
fn unanchored_overlap_does_not_publish_a_second_bus_token() {
    use codescribe_core::pipeline::acoustic_ledger::MutationReceipt;

    let temp = tempfile::tempdir().unwrap();
    let bus =
        TranscriptBus::open_with_path(session("unanchored-bus"), temp.path().join("bus.jsonl"));
    bus.publish_started();
    let mut ledger = AcousticLedger::new();
    let mut reducer = TranscriptReducer::default();
    let admitted = OccurrenceIdentity::new("unanchored-bus", 7, 24_000, 48_000);
    let calibration = EnergyCalibration::new("bus-unanchored", 1.0, 1);
    let evidence = AcousticEvidence {
        occurrence: admitted.clone(),
        duration_ms: 1_000.0,
        energy_integral: 10.0,
        mean_rms_dbfs: -12.0,
        peak_dbfs: -3.0,
        vad_open_sample: Some(admitted.sample_start),
        vad_close_sample: Some(admitted.sample_end),
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(ledger.qualify(&evidence, &calibration).is_qualified());
    let observation = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, admitted.clone());
    let receipt = ledger.admit(&observation, "beta");
    let revision = reducer
        .apply_ledger_mutation(&ledger, &observation, &receipt)
        .expect("committed neighbour");
    let events = bus.publish_revision(&revision, &ledger);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sample_start, 24_000);
    assert_eq!(events[0].sample_end, 48_000);
    assert_eq!(events[0].rendered_text, "beta");

    let covered = OccurrenceIdentity::new("unanchored-bus", 7, 32_000, 40_000);
    let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 2, 0, covered);
    let unanchored = ledger.admit(&whisper, "beta");
    assert!(matches!(
        &unanchored,
        MutationReceipt::KeepVisibleUnanchored { label, .. } if label == "beta"
    ));
    assert!(
        reducer
            .apply_ledger_mutation(&ledger, &whisper, &unanchored)
            .is_none()
    );
    assert_eq!(reducer.visible_projection(), "beta");
    assert_eq!(revision.entries.len(), 1);
    assert_eq!(ledger.conservation().kept_visible_unanchored, 1);
    assert_eq!(ledger.text_of(&admitted), Some("beta"));
    let writer = bus.writer.lock().unwrap();
    assert_eq!(writer.sequence, events[0].sequence);
    assert_eq!(
        writer
            .last_projection
            .as_ref()
            .map(|event| event.rendered_text.as_str()),
        Some("beta")
    );
    assert_eq!(
        writer
            .last_projection
            .as_ref()
            .map(|event| event.sample_end),
        Some(48_000)
    );
}

/// A differing alternative kept wholly inside a committed occurrence is
/// reducer paint evidence only. The next committed revision the Bus
/// publishes carries the committed label, never the alternative. A seal
/// does not settle an independent unresolved observation.
#[test]
fn differing_unanchored_alternative_never_enters_a_bus_revision() {
    use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bus.jsonl");
    let bus = TranscriptBus::open_with_path(session("alternative-bus"), path.clone());
    bus.publish_started();
    let mut ledger = AcousticLedger::new();
    let mut reducer = TranscriptReducer::default();
    let apple = OccurrenceIdentity::new("alternative-bus", 7, 0, 48_000);
    let calibration = EnergyCalibration::new("bus-alternative", 1.0, 1);
    let evidence = AcousticEvidence {
        occurrence: apple.clone(),
        duration_ms: 3_000.0,
        energy_integral: 10.0,
        mean_rms_dbfs: -12.0,
        peak_dbfs: -3.0,
        vad_open_sample: Some(apple.sample_start),
        vad_close_sample: Some(apple.sample_end),
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(ledger.qualify(&evidence, &calibration).is_qualified());
    let observation = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, apple.clone());
    let receipt = ledger.admit(&observation, "Apple mówi tak");
    let revision = reducer
        .apply_ledger_mutation(&ledger, &observation, &receipt)
        .expect("committed Apple occurrence");
    assert_eq!(bus.publish_revision(&revision, &ledger).len(), 1);

    let pin = OccurrenceIdentity::new("alternative-bus", 7, 16_000, 32_000);
    let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 2, 1_000, pin);
    let kept = ledger.keep_visible_unanchored(
        &whisper,
        "Whisper mówi inaczej",
        NoAuthorityReason::ExclusiveTailAwaitingWholeSpan,
    );
    assert!(
        reducer
            .apply_ledger_mutation(&ledger, &whisper, &kept)
            .is_none()
    );
    assert_eq!(reducer.visible_projection(), "Apple mówi tak");
    assert_eq!(
        reducer.unanchored_evidence("alternative-bus", 7)[0].text,
        "Whisper mówi inaczej"
    );

    ledger.schedule_frontier(apple.clone(), [ObservationProducer::Apple]);
    assert!(ledger.note_frontier_return(&apple, ObservationProducer::Apple));
    let seal = ledger.seal(&apple).expect("closed occurrence").clone();
    let sealed = reducer.apply_ledger_seal(&seal).expect("seal revision");
    let events = bus.publish_revision(&sealed, &ledger);
    assert!(!events.is_empty());
    assert!(events.iter().all(|event| {
        !event.rendered_text.contains("Whisper") && !event.label.contains("Whisper")
    }));
    assert_eq!(
        reducer.unanchored_evidence("alternative-bus", 7)[0].text,
        "Whisper mówi inaczej",
        "a containing seal leaves the alternative reviewable outside the Bus"
    );
    assert!(!std::fs::read_to_string(&path).unwrap().contains("Whisper"));
}

#[test]
fn forensic_l77_shared_document_conserves_all_durable_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic-bus.jsonl");
    let bus = TranscriptBus::open_at(session("synthetic-byte-budget"), path.clone(), None).unwrap();
    bus.publish_started();
    let before = std::fs::metadata(&path).unwrap().len() as usize;
    let label = "x".repeat(4096);
    let (ledger, _, revision) = committed_fixture_with_first_label("synthetic-byte-budget", &label);
    let events = bus.publish_revision(&revision, &ledger);
    // In-process HUD and receipt projections retain their existing contract.
    assert_eq!(events.len(), 2);
    assert_committed(&events, &revision, "synthetic-byte-budget");
    let bytes = std::fs::read(&path).unwrap();
    let tail = std::str::from_utf8(&bytes[before..]).unwrap();
    let rows: Vec<serde_json::Value> = tail
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!rows.is_empty());
    let projected: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|row| {
            row.get("rendered_text").and_then(|v| v.as_str())
                == Some(revision.rendered_text.as_str())
        })
        .collect();
    assert!(
        !projected.is_empty(),
        "durable revision must retain the exact full document"
    );
    fn visit_receipts(value: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
        if let serde_json::Value::Object(object) = value {
            if let Some(array) = object.get("acoustic_receipts").and_then(|v| v.as_array()) {
                out.extend(array.iter().cloned());
            }
            // One full document; every occurrence retains its own receipt.
            if let Some(array) = object.get("occurrence_rows").and_then(|v| v.as_array()) {
                for row in array {
                    visit_receipts(row, out);
                }
            }
        }
    }

    let mut persisted_receipts = Vec::new();
    for row in &rows {
        visit_receipts(row, &mut persisted_receipts);
    }
    let expected_receipts: Vec<serde_json::Value> = events
        .iter()
        .flat_map(|event| event.acoustic_receipts.iter())
        .map(|receipt| serde_json::to_value(receipt).unwrap())
        .collect();
    assert_eq!(
        persisted_receipts, expected_receipts,
        "byte reduction must conserve every original occurrence receipt in order"
    );
    let document_bytes: usize = rows
        .iter()
        .filter_map(|row| row.get("rendered_text").and_then(|v| v.as_str()))
        .map(str::len)
        .sum();
    assert!(
        document_bytes <= revision.rendered_text.len() + 64,
        "one durable revision repeated its full document per occurrence: actual {document_bytes}, document {}",
        revision.rendered_text.len()
    );
}

#[test]
fn playback_journal_preserves_states_and_refuses_false_spoken_receipts() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("playback.jsonl");
    let lease = hex::encode(Sha256::digest(b"codex\0playback-owner"));
    let mut event = serde_json::json!({
        "schema": "codescribe.agent-reply-playback.v1",
        "kind": "agent_reply_playback",
        "provider": "codex", "provider_session_id": "playback-owner",
        "lease_id": &lease[..32], "emitted_at": "2026-10-31T05:00:00Z",
        "reply_id": "111111111111111111111111",
        "playback_ticket": "222222222222222222222222"
    });
    for state in [
        "waiting", "playing", "spoken", "failed", "refused", "stopped",
    ] {
        event["state"] = state.into();
        event["spoken"] = (state == "spoken").into();
        assert!(super::append_agent_event(&path, &event).is_ok(), "{state}");
        let before = std::fs::read(&path).unwrap();
        event["spoken"] = (state != "spoken").into();
        assert!(super::append_agent_event(&path, &event).is_err(), "{state}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    event["state"] = "queued".into();
    event["spoken"] = false.into();
    assert!(super::append_agent_event(&path, &event).is_err());
    event["state"] = "waiting".into();
    for ticket in ["", "22222222222222222222222", "AAAAAAAAAAAAAAAAAAAAAAAA"] {
        event["playback_ticket"] = ticket.into();
        assert!(super::append_agent_event(&path, &event).is_err());
    }
}

include!("../transcript_bus_co_audience_tests.rs");
