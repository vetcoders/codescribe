// Included inside agent_channel::tests; all execution belongs to the integrator.
mod co_audience_registry_contract {
    use super::*;

    async fn channel_fixture() -> (tempfile::TempDir, RecordingController, OpenAgentChannel) {
        let dir = tempfile::tempdir().unwrap();
        let controller = RecordingController::new_without_keychain();
        let (binding, bus) = write_dedicated_binding(dir.path());
        controller
            .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
            .await
            .unwrap();
        let open = controller.agent_channels.lock().await.remove(&3).unwrap();
        (dir, controller, open)
    }

    #[tokio::test]
    #[serial]
    async fn co_audience_registry_derivation_uses_only_live_capture_identity_and_sorts_digits() {
        let (_dir, _controller, original) = channel_fixture().await;
        let mut live = original.clone();
        live.mode = ChannelOpenMode::Live;
        live.session_id = Some("capture-old".into());
        let mut other = live.clone();
        other.audience = "new-owner".into();
        other.session_id = Some("capture-new".into());
        other.provider_session_id = Some("new-agent-thread".into());
        let mut missing_capture = live.clone();
        missing_capture.session_id = None;
        let mut channels = std::collections::HashMap::new();
        channels.insert(9, live);
        channels.insert(1, other);
        channels.insert(2, original);
        channels.insert(4, missing_capture);
        let members = cohort_members(&channels);
        assert_eq!(
            members.iter().map(|entry| entry.digit).collect::<Vec<_>>(),
            vec![1, 9]
        );
        assert_eq!(members[0].audience, "new-owner");
        assert_eq!(members[0].session_id, "capture-new");
        assert_eq!(
            members[0].provider_session_id.as_deref(),
            Some("new-agent-thread")
        );
    }

    #[tokio::test]
    #[serial]
    async fn co_audience_projection_is_controller_local_and_changes_as_one_snapshot() {
        let (_dir, controller, mut original) = channel_fixture().await;
        let independent = RecordingController::new_without_keychain();
        original.mode = ChannelOpenMode::Live;
        original.session_id = Some("capture-fixture".into());
        let mut channels = controller.agent_channels.lock().await;
        channels.insert(3, original.clone());
        channels.insert(9, original);
        controller.publish_cohort(&channels);
        let before = Arc::clone(&controller.agent_channel_cohort.read().unwrap());
        channels.remove(&9);
        controller.publish_cohort(&channels);
        let after = Arc::clone(&controller.agent_channel_cohort.read().unwrap());
        assert_eq!(
            before.iter().map(|entry| entry.digit).collect::<Vec<_>>(),
            vec![3, 9]
        );
        assert_eq!(
            after.iter().map(|entry| entry.digit).collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(
            before.len(),
            2,
            "published immutable snapshot remains intact"
        );
        assert!(independent.agent_channel_cohort.read().unwrap().is_empty());
        channels.clear();
        controller.publish_cohort(&channels);
        assert!(controller.agent_channel_cohort.read().unwrap().is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn co_audience_failed_unbound_open_does_not_join_or_replace_the_existing_snapshot() {
        let (dir, controller, mut original) = channel_fixture().await;
        original.mode = ChannelOpenMode::Live;
        original.session_id = Some("capture-existing".into());
        {
            let mut channels = controller.agent_channels.lock().await;
            channels.insert(3, original);
            controller.publish_cohort(&channels);
        }
        let before = Arc::clone(&controller.agent_channel_cohort.read().unwrap());
        let (binding, bus) = write_dedicated_binding(dir.path());
        assert!(
            controller
                .dispatch_agent_channel(7, &binding, &bus, ChannelOpenMode::AttachedOnly)
                .await
                .is_err()
        );
        let after = Arc::clone(&controller.agent_channel_cohort.read().unwrap());
        assert!(
            Arc::ptr_eq(&before, &after),
            "failed admission publishes no roster transition"
        );
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].digit, 3);
        assert_eq!(after[0].session_id, "capture-existing");
    }
}
