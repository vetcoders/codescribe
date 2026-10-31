//! Acceptance tests for manual-only Max consultation and automatic take policy sealing.

use super::*;
use codescribe_core::config::CapturedRuntimeInputs;

#[test]
fn automatic_take_settings_clamps_stored_max_to_smart_without_mutating_source_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = CapturedRuntimeInputs::defaults_at(dir.path().to_path_buf(), 1);
    inputs.user_settings.formatting_level = Some("max".to_string());
    inputs.user_settings.formatter_keywords =
        Some(vec!["Klaudiusz".to_string(), "Codescribe".to_string()]);
    let raw_snapshot = Arc::new(Config::runtime_snapshot_from_captured(inputs));
    assert_eq!(raw_snapshot.formatting_policy(), FormattingPolicy::Max);
    assert_eq!(
        raw_snapshot.automatic_formatting_policy(),
        FormattingPolicy::Smart
    );

    let take_snapshot = automatic_take_settings(Arc::clone(&raw_snapshot));
    assert_eq!(take_snapshot.formatting_policy(), FormattingPolicy::Smart);
    assert_eq!(
        take_snapshot.automatic_formatting_policy(),
        FormattingPolicy::Smart
    );
    assert_eq!(
        take_snapshot.formatter_keywords(),
        &["Klaudiusz".to_string(), "Codescribe".to_string()]
    );
    assert_eq!(
        take_snapshot.formatting_vocabulary_context(),
        raw_snapshot.formatting_vocabulary_context()
    );
    // Explicit Max snapshot remains derivable for manual Format actions while automatic stays Smart.
    let explicit_max = take_snapshot.with_formatting_level(FormattingPolicy::Max);
    assert_eq!(explicit_max.formatting_policy(), FormattingPolicy::Max);
    assert_eq!(
        explicit_max.automatic_formatting_policy(),
        FormattingPolicy::Smart
    );
    assert_eq!(
        explicit_max.formatting_vocabulary_context(),
        take_snapshot.formatting_vocabulary_context()
    );
}

#[tokio::test]
async fn controller_with_stored_max_admits_automatic_smart_for_capture() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut inputs = CapturedRuntimeInputs::defaults_at(dir.path().to_path_buf(), 1);
    inputs.user_settings.formatting_level = Some("max".to_string());
    inputs.user_settings.formatter_keywords = Some(vec!["Vetcoders".to_string()]);
    *controller.runtime_settings.write().await =
        Arc::new(Config::runtime_snapshot_from_captured(inputs));

    let admitted = automatic_take_settings(controller.runtime_settings_arc().await);
    assert_eq!(admitted.formatting_policy(), FormattingPolicy::Smart);
    assert!(controller.max_consultation.lock().await.is_none());
}

/// Fix 45f54de75; kills M3b-01 (startup), M3b-02 (reload), M3b-03 (Smart clamp).
/// Count the deferred consultation's actual preparation entry, then prove the
/// probe is live. It stops before provider construction, tools or network I/O.
#[tokio::test]
#[serial_test::serial]
async fn m3b_persisted_max_never_prepares_until_explicit_consultation_request() {
    use codescribe_core::agent::ThreadDeliveryGateway;
    use codescribe_core::llm::provider::{CustomProvider, WireFamily};
    use codescribe_core::test_isolation::EnvGuard;

    let root = tempfile::tempdir().unwrap();
    let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
    let _config = EnvGuard::set("CODESCRIBE_CONFIG_DIR", root.path());
    let _keychain = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    let controller = RecordingController::new_without_keychain();
    let mut inputs = CapturedRuntimeInputs::defaults_at(root.path().to_path_buf(), 1);
    inputs.user_settings.formatting_level = Some("max".into());
    inputs.user_settings.formatter_keywords = Some(vec!["Vetcoders".into()]);
    inputs
        .user_settings
        .add_custom_provider(
            CustomProvider::new(
                "M3b preparation probe",
                WireFamily::OpenAiResponses,
                "http://127.0.0.1:9/v1/responses",
            )
            .unwrap(),
        )
        .unwrap();
    inputs.user_settings.llm_assistive_provider = Some("custom:m3b-preparation-probe".into());
    inputs.user_settings.llm_assistive_model = Some("m3b-agent".into());
    let settings = Config::runtime_snapshot_from_captured(inputs);
    assert!(crate::agent::max_unavailable_reason(&settings).is_none());

    let preparations = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let count = Arc::clone(&preparations);
    let signal = Arc::clone(&entered);
    let consultation = Arc::new(crate::agent::max_consultation::MaxConsultation::start_deferred(
        "m3b-preparation-probe".into(),
        &settings,
        Box::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            signal.notify_one();
            panic!("M3b preparation probe: stop before provider construction");
        }),
        None,
        ThreadDeliveryGateway::new_in(root.path().join("threads")).unwrap(),
        Arc::new(|_, _, _| {}),
        root.path().join("agent-turn.lease"),
    ));
    *controller.max_consultation.lock().await = Some(Arc::clone(&consultation));
    *controller.runtime_settings.write().await = Arc::new(settings.clone());

    // Startup calls the real controller entrypoint with a usable persisted Max.
    let startup = controller.prepare_max_consultation().await;
    assert_eq!(preparations.load(Ordering::SeqCst), 0, "startup prepared Max");
    assert!(startup.unwrap().is_none());

    {
        let _serial = controller.serial_lock.lock().await;
        controller.install_runtime_settings_generation(settings.clone()).await;
    }
    // The removed reload path spawned a task. A notification catches it even
    // after install returns; the positive probe below rules out a dead counter.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .is_err(),
        "settings installation prepared Max in the background"
    );

    let source = controller.runtime_settings_arc().await;
    let take = automatic_take_settings(Arc::clone(&source));
    assert_eq!(take.formatting_policy(), FormattingPolicy::Smart);
    assert_eq!(source.formatting_policy(), FormattingPolicy::Max);
    assert_eq!(take.formatter_keywords(), &["Vetcoders".to_string()]);
    assert!(controller.selected_max_consultation(&take).await.unwrap().is_none());
    assert_eq!(preparations.load(Ordering::SeqCst), 0);

    // The real archived Format action reaches the same retained owner. The
    // deliberate spawn_blocking panic becomes a failed formatting result,
    // leaving the literal archived source unchanged and making no network call.
    let archive = Config::config_dir().join("transcriptions/2026-10-31/m3b.txt");
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::write(&archive, "Pacjent czuje się dobrze.").unwrap();
    let receipt = controller
        .format_archived_transcript(
            archive.to_string_lossy().into_owned(),
            0,
            Some(FormattingPolicy::Max),
        )
        .await
        .unwrap();
    assert!(matches!(receipt.outcome, ArchivedFormatOutcome::Failed));
    assert!(receipt.revision.is_none());
    assert_eq!(std::fs::read_to_string(archive).unwrap(), "Pacjent czuje się dobrze.");
    assert_eq!(preparations.load(Ordering::SeqCst), 1);
}
