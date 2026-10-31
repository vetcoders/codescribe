// Authored by the co-audience integrator. Included inside transcript_bus::tests
// so the real committed-ledger fixture and durable reader remain the oracle.
mod co_audience_contract {
    use super::*;

    fn member(digit: u8, name: &str, identity: &str) -> ChannelCohortMember {
        ChannelCohortMember {
            digit,
            channel: digit.to_string(),
            audience: name.into(),
            session_id: format!("capture-{identity}"),
            provider: Some("codex".into()),
            provider_session_id: Some(format!("thread-{identity}")),
        }
    }

    fn cohort(members: Vec<ChannelCohortMember>) -> ChannelCohort {
        Arc::new(std::sync::RwLock::new(Arc::new(members)))
    }

    fn bus(dir: &Path, digit: u8) -> Arc<TranscriptBus> {
        let id = format!("agent-channel-{digit}-co-audience");
        let mut take = session(&id);
        take.audience = Some(format!("owner-{digit}"));
        take.badge_only = true;
        let bus = Arc::new(
            TranscriptBus::open_at(take, dir.join(format!("channel-{digit}.jsonl")), None).unwrap(),
        );
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-recipients.v1", "session_id": id,
            "channel": digit.to_string(), "recipients": [{"lease_id": format!("owner-{digit}")}],
        }));
        bus
    }

    fn emit(bus: &TranscriptBus, marker: u64) -> serde_json::Value {
        bus.record_channel_receipt(&serde_json::json!({
            "schema": "codescribe.channel-session.v1", "session_id": bus.session.session_id,
            "state": "open", "marker": marker,
        }));
        crate::durable_bus_oracle::logical_text(&bus.path)
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|row| row["marker"] == marker)
            .unwrap()
    }

    #[test]
    fn co_audience_all_live_writers_observe_one_cohort_and_exclude_only_their_own_digit() {
        let dir = tempfile::tempdir().unwrap();
        let names = [
            (1, "klaudiusz"),
            (2, "lena"),
            (3, "roman"),
            (4, "marek"),
            (5, "tomek"),
            (8, "as"),
            (9, "olga-div0"),
        ];
        let members: Vec<_> = names
            .iter()
            .map(|(digit, name)| member(*digit, name, name))
            .collect();
        let shared = cohort(members.clone());
        for (digit, _) in names {
            let bus = bus(dir.path(), digit);
            bus.attach_co_audience(Arc::clone(&shared), digit);
            let row = emit(&bus, 1);
            let expected: Vec<_> = members
                .iter()
                .filter(|entry| entry.digit != digit)
                .cloned()
                .collect();
            assert_eq!(row["co_audience"], serde_json::to_value(expected).unwrap());
            assert_eq!(row["co_audience_basis"], "emission");
            assert_eq!(
                row["recipients"],
                serde_json::json!([{"lease_id": format!("owner-{digit}")}]),
                "co-audience never expands the frozen delivery owners"
            );
        }
    }

    #[test]
    fn co_audience_late_open_close_and_digit_reuse_are_read_at_each_emission() {
        let dir = tempfile::tempdir().unwrap();
        let own = member(3, "roman", "roman");
        let shared = cohort(vec![own.clone()]);
        let bus = bus(dir.path(), 3);
        bus.attach_co_audience(Arc::clone(&shared), 3);
        let single = emit(&bus, 1);
        assert!(single.get("co_audience").is_none());
        assert!(single.get("co_audience_basis").is_none());
        *shared.write().unwrap() = Arc::new(vec![member(1, "old", "old"), own.clone()]);
        assert_eq!(
            emit(&bus, 2)["co_audience"][0]["provider_session_id"],
            "thread-old"
        );
        *shared.write().unwrap() = Arc::new(vec![own.clone()]);
        assert!(emit(&bus, 3).get("co_audience").is_none());
        *shared.write().unwrap() = Arc::new(vec![member(1, "new", "new"), own]);
        let reused = emit(&bus, 4);
        assert_eq!(reused["co_audience"][0]["audience"], "new");
        assert_eq!(reused["co_audience"][0]["session_id"], "capture-new");
        assert_eq!(
            reused["co_audience"][0]["provider_session_id"],
            "thread-new"
        );
        assert!(!reused.to_string().contains("thread-old"));
    }

    #[test]
    fn co_audience_closing_cohort_survives_delay_and_the_live_cohort_becoming_empty() {
        let dir = tempfile::tempdir().unwrap();
        let members = vec![
            member(1, "one", "one"),
            member(3, "roman", "roman"),
            member(9, "nine", "nine"),
        ];
        let shared = cohort(members.clone());
        let first = bus(dir.path(), 1);
        let third = bus(dir.path(), 3);
        let ninth = bus(dir.path(), 9);
        for (digit, bus) in [(1, &first), (3, &third), (9, &ninth)] {
            bus.attach_co_audience(Arc::clone(&shared), digit);
        }
        let closing = Arc::clone(&shared.read().unwrap());
        first.freeze_co_audience(Arc::clone(&closing));
        third.freeze_co_audience(Arc::clone(&closing));
        *shared.write().unwrap() = Arc::new(vec![members[2].clone()]);
        assert!(emit(&ninth, 1).get("co_audience").is_none());
        *shared.write().unwrap() = Arc::new(Vec::new());
        for (digit, bus) in [(1, first), (3, third)] {
            let row = emit(&bus, 2);
            let expected: Vec<_> = members
                .iter()
                .filter(|entry| entry.digit != digit)
                .cloned()
                .collect();
            assert_eq!(row["co_audience"], serde_json::to_value(expected).unwrap());
            assert_eq!(row["co_audience_basis"], "capture_close");
        }
    }

    #[test]
    fn co_audience_snapshot_update_does_not_wait_for_or_copy_into_each_bus_writer() {
        let dir = tempfile::tempdir().unwrap();
        let own = member(3, "roman", "roman");
        let shared = cohort(vec![own.clone(), member(1, "old", "old")]);
        let bus = bus(dir.path(), 3);
        bus.attach_co_audience(Arc::clone(&shared), 3);
        let held_writer = bus.writer.lock().unwrap();
        let (ready, wait) = std::sync::mpsc::channel();
        let emitting = Arc::clone(&bus);
        let thread = std::thread::spawn(move || {
            ready.send(()).unwrap();
            emit(&emitting, 1)
        });
        wait.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        // The emission is blocked on its writer while the controller can publish
        // the replacement without touching that writer. Its eventual read must
        // observe the new membership, not an old per-writer cached copy.
        *shared.write().unwrap() = Arc::new(vec![own, member(9, "new", "new")]);
        drop(held_writer);
        let row = thread.join().unwrap();
        assert_eq!(row["co_audience"][0]["channel"], "9");
        assert_eq!(row["co_audience"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn co_audience_close_twins_preserve_words_revision_and_frozen_recipients() {
        let dir = tempfile::tempdir().unwrap();
        let bus = bus(dir.path(), 3);
        let shared = cohort(vec![member(1, "one", "one"), member(3, "roman", "roman")]);
        bus.attach_co_audience(Arc::clone(&shared), 3);
        let id = bus.session.session_id.clone();
        bus.observe_capture_opened(&id, 7);
        let (ledger, _, revision) = committed_fixture(&id);
        bus.publish_revision(&revision, &ledger);
        bus.freeze_co_audience(Arc::clone(&shared.read().unwrap()));
        *shared.write().unwrap() = Arc::new(Vec::new());
        bus.observe_channel_publication_evidence(&ledger);
        bus.publish_channel_closed(0, None).unwrap();
        let text = crate::durable_bus_oracle::logical_text(&bus.path);
        let twins: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|row| row["status"] == "transcript_sealed")
            .collect();
        assert_eq!(
            twins.len(),
            2,
            "real evidence and clean closing rows: {text}"
        );
        for row in &twins {
            assert_eq!(row["co_audience_basis"], "capture_close");
            assert_eq!(row["co_audience"][0]["channel"], "1");
            assert_eq!(
                row["recipients"],
                serde_json::json!([{"lease_id": "owner-3"}])
            );
            assert_eq!(row["rendered_text"], revision.rendered_text);
            assert_eq!(row["reducer_revision"], twins[0]["reducer_revision"]);
        }
        assert_eq!(twins[1]["text"], twins[0]["rendered_text"]);
    }

    #[test]
    fn co_audience_broadcast_bus_is_not_implicitly_attached_to_local_live_membership() {
        let dir = tempfile::tempdir().unwrap();
        let broadcast = bus(dir.path(), 0);
        let row = emit(&broadcast, 1);
        assert!(row.get("co_audience").is_none());
        assert!(row.get("co_audience_basis").is_none());
    }
}
