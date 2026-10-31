use super::super::transcript_bus::append_agent_event;
use super::*;
use std::fs;
use std::sync::{Mutex, mpsc};
use std::time::Duration;

const TIME: &str = "2026-10-27T03:00:00Z";

struct Store {
    _directory: tempfile::TempDir,
    bus: PathBuf,
    pair: StationPairing,
}

fn private_write(path: &Path, value: &impl Serialize) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

impl Store {
    fn new(host: char) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let bus = root.join("bus.jsonl");
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&bus)
            .unwrap();
        let file = Arc::new(Mutex::new(file));
        generation::append_durable(
            &bus,
            &file,
            &generation::encode(&json!({"schema":"fixture.init"})).unwrap(),
        )
        .unwrap();
        let owner = LogicalOwner {
            provider: "codex".into(),
            provider_session_id: "fixture-native-session".into(),
            lease_id: hex::encode(Sha256::digest(b"codex\0fixture-native-session"))[..32].into(),
        };
        let pair = StationPairing {
            schema: STATION_PAIR_SCHEMA.into(),
            pair_id: "a".repeat(24),
            home_host_id: "1".repeat(32),
            owner,
            bridge_root: root,
            local_bus: bus.clone(),
            local: Station {
                host_id: host.to_string().repeat(32),
                generation_id: generation::view(&bus).unwrap().unwrap().stream_id,
                channel: "1".into(),
            },
            peer: Station {
                host_id: if host == '1' { "2" } else { "1" }.repeat(32),
                generation_id: uuid::Uuid::new_v4().to_string(),
                channel: "2".into(),
            },
            transitions: vec![],
            guest_broadcast_off_receipt_id: "b".repeat(24),
        };
        let store = Self {
            _directory: directory,
            bus,
            pair,
        };
        store.persist_pair();
        store
    }

    fn persist_pair(&self) {
        private_write(
            &self.pair.bridge_root.join("host-instance.json"),
            &json!({"schema":"codescribe.host-instance.v1", "host_id":self.pair.local.host_id}),
        );
        private_write(
            &self
                .pair
                .bridge_root
                .join("vc.agent-audience-binding.v1.json"),
            &json!({
            "schema":"vc.agent-audience-binding.v1", "bindings":{"1":{
                "provider":self.pair.owner.provider, "provider_session_id":self.pair.owner.provider_session_id,
                "lease_id":self.pair.owner.lease_id, "bus":self.bus,
            }}}),
        );
        private_write(
            &PathBuf::from(format!("{}.station-pair.json", self.bus.display())),
            &self.pair,
        );
    }

    fn typed(&self, id: u64) -> Value {
        json!({"schema":"codescribe.agent-user-message.v1", "kind":"agent_user_message",
            "message_id":format!("{id:024x}"), "source_event_id":format!("{id:024x}"), "source":"typed",
            "provider":self.pair.owner.provider, "provider_session_id":self.pair.owner.provider_session_id,
            "lease_id":self.pair.owner.lease_id, "emitted_at":TIME, "channel":"1", "audience":"agent",
            "text":"same words", "sender":{"name":"founder", "channel":"10"},
            "recipients":[{"provider":self.pair.owner.provider, "provider_session_id":self.pair.owner.provider_session_id,
                "lease_id":self.pair.owner.lease_id,"channel":"1","audience":"agent","bus":self.bus}]})
    }

    fn append(&self, row: &Value) -> generation::BusAppendReceipt {
        let shared = shared_bus_file(&self.bus).unwrap();
        generation::append_durable(&self.bus, &shared, &generation::encode(row).unwrap()).unwrap()
    }

    fn projection(
        &self,
        row: &Value,
        receipt: generation::BusAppendReceipt,
        kind: MessageKind,
    ) -> Value {
        let message_id = match kind {
            MessageKind::Typed => row["message_id"].as_str().unwrap().to_owned(),
            MessageKind::AgentReply => row["reply_id"].as_str().unwrap().to_owned(),
            MessageKind::Voice => hex::encode(Sha256::digest(format!(
                "channel-message\0{}",
                row["session_id"].as_str().unwrap()
            )))[..24]
                .into(),
        };
        let sender = if kind == MessageKind::Voice {
            json!({"name":"founder","channel":"10","owner":null})
        } else if kind == MessageKind::AgentReply {
            json!({"name":row["name"],"channel":row.get("channel"),"owner":self.pair.owner})
        } else {
            json!({"name":row["sender"]["name"],"channel":row["sender"]["channel"],"owner":null})
        };
        json!({"schema":HISTORY_PROJECTION_SCHEMA,"kind":"history_projection","pair_id":self.pair.pair_id,
            "projection":"source","owner":self.pair.owner,"source_receipt":receipt,
            "message":{"origin":{"source_host_id":self.pair.local.host_id,"source_generation_id":receipt.stream_id,"message_id":message_id},
            "final_revision":1,"order":{"counter":0,"source_host_id":self.pair.local.host_id,"message_id":message_id},
            "kind":kind,"emitted_at":TIME,"sender":sender,"recipient":self.pair.owner,"text":row["text"],
            "reply_to":row.get("history_reply_to"),"attachments":[],
            "eligibility":{"broadcast":row["audience"]=="*" || row["origin_channel"]=="0","release_control_id":null}}})
    }

    fn publish(&self, id: u64) -> Value {
        let row = self.typed(id);
        let receipt = serde_json::from_value(append_agent_event(&self.bus, &row).unwrap()).unwrap();
        self.projection(&row, receipt, MessageKind::Typed)
    }

    fn rows(&self) -> Vec<Value> {
        let mut result = vec![];
        let shared = shared_bus_file(&self.bus).unwrap();
        generation::append_durable_many_with(
            &[(self.bus.clone(), shared)],
            || {
                generation::scan_durable_events(&self.bus, |row, _| {
                    result.push(row.clone());
                    Ok(())
                })?;
                Ok(vec![None])
            },
            None,
        )
        .unwrap();
        result
    }

    fn voice(&self, broadcast: bool) -> Value {
        json!({"schema":"codescribe.transcript.v1","status":"transcript_sealed","closing_decision":"publish",
            "session_id":if broadcast {"agent-channel-0-fixture"} else {"agent-channel-1-fixture"},
            "capture_epoch":7,"reducer_revision":1,"emitted_at":TIME,"channel":if broadcast {"0"} else {"1"},
            "audience":if broadcast {"*"} else {"agent"},"text":"same words",
            "recipients":[{"provider":self.pair.owner.provider,"provider_session_id":self.pair.owner.provider_session_id,
                "lease_id":self.pair.owner.lease_id,"channel":"1","bus":self.bus,"audience":"agent"}]})
    }
}

fn linked() -> (Store, Store) {
    let mut a = Store::new('1');
    let mut b = Store::new('2');
    a.pair.peer.generation_id = b.pair.local.generation_id.clone();
    b.pair.peer.generation_id = a.pair.local.generation_id.clone();
    a.persist_pair();
    b.persist_pair();
    (a, b)
}

#[test]
fn original_lost_ack_conflict_and_equal_text_identity() {
    let store = Store::new('1');
    let row = store.typed(1);
    let first = append_agent_event(&store.bus, &row).unwrap();
    assert_eq!(append_agent_event(&store.bus, &row).unwrap(), first);
    let mut conflict = row.clone();
    conflict["text"] = json!("different");
    assert!(append_agent_event(&store.bus, &conflict).is_err());
    append_agent_event(&store.bus, &store.typed(2)).unwrap();
    assert_eq!(
        store
            .rows()
            .iter()
            .filter(|row| row["schema"] == "codescribe.agent-user-message.v1")
            .count(),
        2
    );
}

#[test]
fn concurrent_original_retries_share_one_durable_receipt() {
    let store = Store::new('1');
    let row = store.typed(1);
    let workers = (0..4)
        .map(|_| {
            let bus = store.bus.clone();
            let row = row.clone();
            std::thread::spawn(move || append_agent_event(&bus, &row).unwrap())
        })
        .collect::<Vec<_>>();
    let receipts = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert!(receipts.iter().all(|receipt| receipt == &receipts[0]));
    assert_eq!(store.rows().len(), 2);
}

#[test]
fn source_replica_retry_and_conflicting_payload_never_append_twice() {
    let (a, b) = linked();
    let request = a.publish(1);
    let frozen = append_agent_event(&a.bus, &request).unwrap();
    assert_eq!(append_agent_event(&a.bus, &request).unwrap(), frozen);
    let mut replica = frozen["projection"].clone();
    replica["projection"] = json!("replica");
    let receipt = append_agent_event(&b.bus, &replica).unwrap();
    assert_eq!(append_agent_event(&b.bus, &replica).unwrap(), receipt);
    replica["message"]["text"] = json!("conflict");
    assert!(append_agent_event(&b.bus, &replica).is_err());
    assert_eq!(b.rows().len(), 2);
    assert_eq!(a.rows().len(), 3);
}

#[test]
fn replica_different_ids_keep_equal_text_and_source_advances_lamport() {
    let (a, b) = linked();
    for id in [1, 2] {
        let request = a.publish(id);
        let frozen = append_agent_event(&a.bus, &request).unwrap();
        let mut replica = frozen["projection"].clone();
        replica["projection"] = json!("replica");
        append_agent_event(&b.bus, &replica).unwrap();
    }
    let request = b.publish(3);
    let frozen = append_agent_event(&b.bus, &request).unwrap();
    assert_eq!(frozen["projection"]["message"]["order"]["counter"], 3);
    assert_eq!(
        b.rows()
            .iter()
            .filter(|row| row["schema"] == HISTORY_PROJECTION_SCHEMA)
            .count(),
        3
    );
}

#[test]
fn executable_schema_unknown_fields_and_oversize_refuse_without_append() {
    let store = Store::new('1');
    let request = store.publish(1);
    let before = store.rows().len();
    for keys in [
        vec!["wav"],
        vec!["message", "schema"],
        vec!["message", "native_enqueue"],
        vec!["message", "origin", "capture_epoch"],
    ] {
        let mut forged = request.clone();
        let mut target = &mut forged;
        for key in &keys[..keys.len() - 1] {
            target = &mut target[*key];
        }
        target[keys[keys.len() - 1]] = json!("forged");
        assert!(append_agent_event(&store.bus, &forged).is_err());
    }
    let mut forged = store.typed(2);
    forged["pair_id"] = json!("a".repeat(24));
    assert!(append_agent_event(&store.bus, &forged).is_err());
    let mut oversized = request;
    oversized["message"]["text"] = json!("x".repeat(HISTORY_LIMIT));
    assert!(append_agent_event(&store.bus, &oversized).is_err());
    assert_eq!(store.rows().len(), before);
}

#[test]
fn broadcast_voice_uses_original_exact_owners_and_excludes_later_opt_in() {
    let store = Store::new('1');
    let row = store.voice(true);
    let receipt = store.append(&row);
    let request = store.projection(&row, receipt, MessageKind::Voice);
    append_agent_event(&store.bus, &request).unwrap();
    let excluded = Store::new('1');
    let mut row = excluded.voice(true);
    row["recipients"][0]["provider_session_id"] = json!("another-session");
    let receipt = excluded.append(&row);
    let request = excluded.projection(&row, receipt, MessageKind::Voice);
    assert!(append_agent_event(&excluded.bus, &request).is_err());
    // Today's valid pairing/binding is deliberately insufficient historical eligibility.
    assert!(StationPairing::load(&excluded.bus).is_ok());
}

#[test]
fn typed_broadcast_requires_original_admitted_target_inventory() {
    let store = Store::new('1');
    let mut row = store.typed(1);
    row["origin_channel"] = json!("0");
    let receipt = serde_json::from_value(append_agent_event(&store.bus, &row).unwrap()).unwrap();
    let request = store.projection(&row, receipt, MessageKind::Typed);
    assert!(append_agent_event(&store.bus, &request).is_err());
    let mut row = store.typed(2);
    row["origin_channel"] = json!("0");
    row["target"] = json!({"recipients":[{"channel":"1","name":"agent"}]});
    let receipt = serde_json::from_value(append_agent_event(&store.bus, &row).unwrap()).unwrap();
    append_agent_event(
        &store.bus,
        &store.projection(&row, receipt, MessageKind::Typed),
    )
    .unwrap();
}

#[test]
fn unsolicited_reply_preserves_missing_channel_instead_of_inventing_slot() {
    let store = Store::new('1');
    let row = json!({"schema":"codescribe.agent-reply.v1","kind":"agent_reply","reply_id":"c".repeat(24),
        "provider":store.pair.owner.provider,"provider_session_id":store.pair.owner.provider_session_id,
        "lease_id":store.pair.owner.lease_id,"emitted_at":TIME,"name":"agent","text":"same words", "spoken":false,
        "association":"unsolicited","delivery_id":null});
    let receipt = serde_json::from_value(append_agent_event(&store.bus, &row).unwrap()).unwrap();
    let request = store.projection(&row, receipt, MessageKind::AgentReply);
    let frozen = append_agent_event(&store.bus, &request).unwrap();
    assert!(frozen["projection"]["message"]["sender"]["channel"].is_null());
}

#[test]
fn held_revision_release_and_withdraw_obey_original_deadline() {
    for withdraw in [false, true] {
        let store = Store::new('1');
        let mut row = store.typed(1);
        row["hold_until"] = json!("2099-01-01T00:00:00Z");
        let receipt =
            serde_json::from_value(append_agent_event(&store.bus, &row).unwrap()).unwrap();
        let mut request = store.projection(&row, receipt, MessageKind::Typed);
        assert!(append_agent_event(&store.bus, &request).is_err());
        let mut control = json!({"schema":"codescribe.delivery-control.v1","kind":"revise","control_id":"d".repeat(24),
            "message_id":row["message_id"],"provider":store.pair.owner.provider,"provider_session_id":store.pair.owner.provider_session_id,
            "lease_id":store.pair.owner.lease_id,"bus":store.bus,"channel":"1","audience":"agent","emitted_at":"2026-10-27T03:00:01Z","text":"revised"});
        append_agent_event(&store.bus, &control).unwrap();
        control["kind"] = json!(if withdraw { "withdraw" } else { "release" });
        control["control_id"] = json!("e".repeat(24));
        control["emitted_at"] = json!("2026-10-27T03:00:02Z");
        append_agent_event(&store.bus, &control).unwrap();
        request["message"]["text"] = json!("revised");
        request["message"]["final_revision"] = json!(2);
        request["message"]["eligibility"]["release_control_id"] = control["control_id"].clone();
        if withdraw {
            assert!(append_agent_event(&store.bus, &request).is_err());
        } else {
            append_agent_event(&store.bus, &request).unwrap();
        }
    }
}

#[test]
fn addressed_reply_retains_historical_channel_and_exact_parent() {
    let store = Store::new('1');
    let request = store.publish(1);
    let parent = append_agent_event(&store.bus, &request).unwrap();
    let row = json!({"schema":"codescribe.agent-reply.v1","kind":"agent_reply","reply_id":"c".repeat(24),
        "provider":store.pair.owner.provider,"provider_session_id":store.pair.owner.provider_session_id,
        "lease_id":store.pair.owner.lease_id,"emitted_at":TIME,"name":"agent","channel":"4",
        "text":"same words","spoken":false,"association":"addressed","delivery_id":"d".repeat(24),
        "message_id":request["message"]["origin"]["message_id"]});
    let receipt = serde_json::from_value(append_agent_event(&store.bus, &row).unwrap()).unwrap();
    let mut reply = store.projection(&row, receipt, MessageKind::AgentReply);
    assert!(append_agent_event(&store.bus, &reply).is_err());
    reply["message"]["reply_to"] = parent["projection"]["message"]["origin"].clone();
    let frozen = append_agent_event(&store.bus, &reply).unwrap();
    assert_eq!(frozen["projection"]["message"]["sender"]["channel"], "4");
    assert_eq!(frozen["projection"]["message"]["order"]["counter"], 2);
}

#[test]
fn malformed_original_attachments_and_foreign_projection_identity_refuse() {
    let store = Store::new('1');
    let mut row = store.typed(1);
    row["attachments"] = json!({"corrupt":"descriptor inventory"});
    let receipt = serde_json::from_value(append_agent_event(&store.bus, &row).unwrap()).unwrap();
    assert!(
        append_agent_event(
            &store.bus,
            &store.projection(&row, receipt, MessageKind::Typed)
        )
        .is_err()
    );
    let request = store.publish(2);
    for field in ["owner", "host", "id", "schema"] {
        let mut forged = request.clone();
        match field {
            "owner" => forged["owner"]["provider_session_id"] = json!("foreign-native-session"),
            "host" => forged["message"]["origin"]["source_host_id"] = json!("0".repeat(32)),
            "id" => forged["message"]["origin"]["message_id"] = json!("BAD-ID"),
            _ => forged["schema"] = json!("unknown.schema"),
        }
        assert!(append_agent_event(&store.bus, &forged).is_err());
    }
}

#[test]
fn committed_capture_attachment_cannot_be_omitted_and_retains_anchor() {
    for variant in [
        "committed",
        "uncommitted",
        "foreign",
        "wrong_epoch",
        "other_bus",
    ] {
        let store = Store::new('1');
        let row = store.voice(false);
        let other = Store::new('1');
        let mut stage = json!({"schema":"codescribe.capture-attachment.v1","kind":"capture_attachment","stage_id":"f".repeat(24),
            "session_id":row["session_id"],"capture_epoch":7,"source_sequence":5,"sample_start":32000,"sample_end":64000,
            "audience":row["audience"],"recipients":row["recipients"],"emitted_at":TIME,
            "commit_bus":if variant=="other_bus" {&other.bus} else {&store.bus},
            "attachments":[{"name":"sent.txt","path":"/provenance/only/sent.txt","media_type":"text/plain","bytes":4,"sha256":"a".repeat(64)}]});
        if variant == "foreign" {
            stage["recipients"][0]["lease_id"] = json!("0".repeat(32));
        }
        if variant == "wrong_epoch" {
            stage["capture_epoch"] = json!(8);
        }
        store.append(&stage);
        if variant != "uncommitted" {
            let commit = json!({"schema":"codescribe.capture-attachment-commit.v1","kind":"capture_attachment_commit",
                "stage_id":stage["stage_id"],"session_id":stage["session_id"],"capture_epoch":stage["capture_epoch"],
                "source_sequence":stage["source_sequence"],"commit_bus":stage["commit_bus"],"emitted_at":TIME});
            if variant == "other_bus" {
                other.append(&commit);
            } else {
                store.append(&commit);
            }
        }
        let receipt = store.append(&row);
        let mut request = store.projection(&row, receipt, MessageKind::Voice);
        let eligible = matches!(variant, "committed" | "other_bus");
        if eligible {
            assert!(append_agent_event(&store.bus, &request).is_err());
        } else {
            append_agent_event(&store.bus, &request).unwrap();
            continue;
        }
        request["message"]["attachments"] = json!([{"name":"sent.txt","source_path":"/provenance/only/sent.txt","media_type":"text/plain","bytes":4,"sha256":"a".repeat(64),
            "capture":{"stage_id":"f".repeat(24),"capture_epoch":7,"source_sequence":5,"attached_at_sample":32000}}]);
        let frozen = append_agent_event(&store.bus, &request).unwrap();
        assert_eq!(
            frozen["projection"]["message"]["attachments"],
            request["message"]["attachments"]
        );
    }
}

#[test]
fn stale_binding_host_pair_and_private_receipt_refuse() {
    for field in ["binding", "host", "pair", "mode"] {
        let mut store = Store::new('1');
        let request = store.publish(1);
        match field {
            "binding" => private_write(
                &store
                    .pair
                    .bridge_root
                    .join("vc.agent-audience-binding.v1.json"),
                &json!({"schema":"vc.agent-audience-binding.v1","bindings":{}}),
            ),
            "host" => private_write(
                &store.pair.bridge_root.join("host-instance.json"),
                &json!({"schema":"codescribe.host-instance.v1","host_id":"0".repeat(32)}),
            ),
            "pair" => {
                store.pair.pair_id = "e".repeat(24);
                store.persist_pair();
            }
            _ => fs::set_permissions(
                store.pair.bridge_root.join("host-instance.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap(),
        }
        assert!(append_agent_event(&store.bus, &request).is_err());
    }
}

#[test]
fn foreign_commit_lease_refuses_without_waiting_or_appending() {
    let store = Store::new('1');
    let other = Store::new('2');
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!("{}.generation.lock", other.bus.display()))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) },
        0
    );
    let bus = store.bus.clone();
    let foreign = other.bus.clone();
    let (send, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let shared = shared_bus_file(&bus).unwrap();
        let result = generation::append_durable_many_with(
            &[(bus, shared)],
            || {
                generation::observe_durable_events(&foreign, |_, _| Ok(()))?;
                Ok(vec![Some(generation::encode(
                    &json!({"schema":"fixture.unpublished"}),
                )?)])
            },
            None,
        );
        send.send(result.unwrap_err().kind()).unwrap();
    });
    let result = receive.recv_timeout(Duration::from_secs(2));
    drop(lock_file);
    worker.join().unwrap();
    assert_eq!(result.unwrap(), io::ErrorKind::WouldBlock);
    assert_eq!(store.rows().len(), 1);
}

#[test]
fn binding_lock_revalidates_after_waiting_for_rebind() {
    let store = Store::new('1');
    let request = store.publish(1);
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(
            store
                .pair
                .bridge_root
                .join("vc.agent-audience-binding.v1.lock"),
        )
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) },
        0
    );
    let (send, receive) = mpsc::channel();
    let bus = store.bus.clone();
    let worker = std::thread::spawn(move || {
        send.send(append_agent_event(&bus, &request).is_err())
            .unwrap()
    });
    assert!(receive.recv_timeout(Duration::from_millis(100)).is_err());
    private_write(
        &store
            .pair
            .bridge_root
            .join("vc.agent-audience-binding.v1.json"),
        &json!({"schema":"vc.agent-audience-binding.v1","bindings":{}}),
    );
    drop(lock_file);
    assert!(receive.recv_timeout(Duration::from_secs(5)).unwrap());
    worker.join().unwrap();
}

#[test]
fn reply_order_out_of_order_import_and_late_local_parent_refuse_cycles() {
    let (a, b) = linked();
    let parent = a.publish(1);
    let frozen = append_agent_event(&a.bus, &parent).unwrap();
    let mut row = a.typed(2);
    row["history_reply_to"] = frozen["projection"]["message"]["origin"].clone();
    let receipt = serde_json::from_value(append_agent_event(&a.bus, &row).unwrap()).unwrap();
    let child =
        append_agent_event(&a.bus, &a.projection(&row, receipt, MessageKind::Typed)).unwrap();
    let mut replica = child["projection"].clone();
    replica["projection"] = json!("replica");
    append_agent_event(&b.bus, &replica).unwrap();
    let mut replica = frozen["projection"].clone();
    replica["projection"] = json!("replica");
    append_agent_event(&b.bus, &replica).unwrap();
    let local_parent = b.publish(3);
    let mut orphan = child["projection"].clone();
    orphan["projection"] = json!("replica");
    orphan["message"]["origin"]["message_id"] = json!("e".repeat(24));
    orphan["message"]["order"]["message_id"] = json!("e".repeat(24));
    orphan["message"]["order"]["counter"] = json!(10);
    orphan["message"]["reply_to"] = local_parent["message"]["origin"].clone();
    append_agent_event(&b.bus, &orphan).unwrap();
    assert!(append_agent_event(&b.bus, &local_parent).is_err());
    let mut cycle = parent;
    cycle["message"]["reply_to"] = cycle["message"]["origin"].clone();
    assert!(append_agent_event(&a.bus, &cycle).is_err());
}

#[test]
fn old_generation_needs_canonical_transition_chain() {
    let (a, mut b) = linked();
    let request = a.publish(1);
    let frozen = append_agent_event(&a.bus, &request).unwrap();
    let mut replica = frozen["projection"].clone();
    replica["projection"] = json!("replica");
    let old = b.pair.peer.generation_id.clone();
    b.pair.peer.generation_id = uuid::Uuid::new_v4().to_string();
    b.persist_pair();
    assert!(append_agent_event(&b.bus, &replica).is_err());
    b.pair.transitions.push(GenerationTransition {
        host_id: a.pair.local.host_id.clone(),
        from: old,
        to: b.pair.peer.generation_id.clone(),
        receipt_id: "c".repeat(24),
    });
    b.persist_pair();
    append_agent_event(&b.bus, &replica).unwrap();
}

#[test]
fn original_receipt_survives_daily_rotation_and_chunked_history() {
    let store = Store::new('1');
    let mut row = store.typed(1);
    row["attachments"] = json!([{"opaque_fixture": "x".repeat(600_000)}]);
    let receipt = append_agent_event(&store.bus, &row).unwrap();
    let manifest_path = generation::manifest_path(&store.bus);
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["active"]["day"] = json!("2000_0101");
    private_write(&manifest_path, &manifest);
    assert_eq!(append_agent_event(&store.bus, &row).unwrap(), receipt);
    assert_eq!(
        store
            .rows()
            .iter()
            .filter(|event| event["schema"] == "codescribe.agent-user-message.v1")
            .count(),
        1
    );
    let manifest = generation::view(&store.bus).unwrap().unwrap();
    assert_eq!(manifest.stream_id, receipt["stream_id"]);
}

#[test]
fn corrupted_or_incomplete_stream_cannot_certify_retry() {
    for tail in ["{broken}\n", "{\"unfinished\":"] {
        let store = Store::new('1');
        let row = store.typed(1);
        append_agent_event(&store.bus, &row).unwrap();
        use std::io::Write;
        OpenOptions::new()
            .append(true)
            .open(&store.bus)
            .unwrap()
            .write_all(tail.as_bytes())
            .unwrap();
        assert!(append_agent_event(&store.bus, &row).is_err());
    }
}
