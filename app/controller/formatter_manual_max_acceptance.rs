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
