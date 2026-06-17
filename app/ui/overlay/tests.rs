//! Overlay unit + opt-in real-flow E2E tests (decomposed alongside the
//! `mod.rs` split; semantics unchanged).

use std::sync::atomic::Ordering;
use std::time::Instant;

use super::actions::{
    AugmentAction, OverlayActionButtonRole, OverlayButtonAction, SETTINGS_SELECTOR_NAME,
    augment_action_for_state, overlay_button_route,
};
use super::layout::{
    OVERLAY_TEXT_MIN_HEIGHT, OVERLAY_WINDOW_MIN_HEIGHT, compute_overlay_layout_metrics,
};
use super::lifecycle::{append_transcription_delta_impl, should_auto_hide};
use super::preview::{display_text_for_state, overlay_visible_text, stable_overlay_preview_text};
use super::state::{
    AUTO_HIDE_GENERATION, AUTO_HIDE_PENDING, DEFAULT_AUTO_HIDE_DELAY_SECS, FormatPhase,
    MAX_AUTO_HIDE_DELAY_SECS, MIN_AUTO_HIDE_DELAY_SECS, OVERLAY_STATE, action_text_for_contract,
    apply_user_edit_to_state, parse_auto_hide_delay_secs,
};
use super::widgets::{decision_hint_text, overlay_status_label, transcript_text_view_editable};
use super::{
    TranscriptionActionContractMode, TranscriptionOverlayConfig, current_segment_text,
    get_transcription_text, is_transcription_overlay_visible,
};
use crate::presentation::emitter::PresentationEmitter;
use crate::ui::shared::status::status_from_detail;
use codescribe_core::audio::load_audio_file;
use codescribe_core::pipeline::contracts::{DeltaSink, EngineEvent, EventSink, TranscriptDelta};
use codescribe_core::pipeline::streaming::collect_buffered_engine_events;
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

const OVERLAY_REAL_FLOW_OPT_IN_ENV: &str = "CODESCRIBE_E2E_STT";

fn overlay_real_flow_enabled() -> bool {
    std::env::var(OVERLAY_REAL_FLOW_OPT_IN_ENV)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn canonical_data_assets_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".codescribe/data_assets");
    if dir.exists() { Some(dir) } else { None }
}

fn canonical_overlay_cases() -> Vec<(PathBuf, PathBuf)> {
    let Some(dir) = canonical_data_assets_dir() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let wav = entry.path();
        if wav.extension().and_then(|ext| ext.to_str()) != Some("wav") {
            continue;
        }

        let Some(stem) = wav.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let reference = dir.join(format!(
            "{stem}_codescribe_raw_human_transcription_from_wav.txt"
        ));
        if reference.exists() {
            out.push((wav, reference));
        }
    }

    out.sort();
    out
}

fn append_utterance_text(rendered: &mut String, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    if !rendered.is_empty() {
        rendered.push(' ');
    }
    rendered.push_str(trimmed);
}

fn final_transcript_from_events(events: &[EngineEvent]) -> String {
    let mut transcript = String::new();
    for event in events {
        if let EngineEvent::UtteranceFinal { text, .. } = event {
            append_utterance_text(&mut transcript, text);
        }
    }
    transcript
}

fn normalize_overlay_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn human_reference_excerpt(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|text| {
            text.split_whitespace()
                .take(24)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn reset_overlay_state_for_test() {
    // Test fixture: pointers may be invalid (tests don't always wire a
    // real AppKit window), so we intentionally do NOT send `release` for
    // window / tracking_area / action_handler here. Production teardown
    // lives in `hide_transcription_overlay_impl` + `close_window_by_ptr`.
    AUTO_HIDE_PENDING.store(false, Ordering::SeqCst);
    AUTO_HIDE_GENERATION.store(0, Ordering::SeqCst);

    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window = None;
    state.header_label = None;
    state.text_scroll_view = None;
    state.text_view = None;
    state.status_field = None;
    state.auto_hide_label = None;
    state.blur_view = None;
    state.copy_button = None;
    state.augment_button = None;
    state.save_button = None;
    state.commit_button = None;
    state.progress_indicator = None;
    state.tracking_area = None;
    state.decision_mode = false;
    state.hover_active = false;
    state.action_handler = None;
    state.action_contract_mode = TranscriptionActionContractMode::Raw;
    state.format_phase = FormatPhase::Idle;
    state.display_status.clear();
    state.raw_text.clear();
    state.last_pass_text.clear();
    state.accumulated_text.clear();
    state.user_edited = false;
    state.min_height = OVERLAY_WINDOW_MIN_HEIGHT;
    state.max_height = OVERLAY_WINDOW_MIN_HEIGHT;
    state.last_applied_height = OVERLAY_WINDOW_MIN_HEIGHT;
    state.last_layout_resize_at = Instant::now();
    state.pending_layout_resize = false;
}

fn overlay_visible_text_now() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    display_text_for_state(&state)
}

fn has_one_to_three_word_collapse(snapshots: &[String]) -> bool {
    let mut saw_substantial_text = false;

    for snapshot in snapshots {
        let words = snapshot.split_whitespace().count();
        if words >= 6 || snapshot.chars().count() >= 30 {
            saw_substantial_text = true;
        }

        if saw_substantial_text && (1..=3).contains(&words) {
            return true;
        }
    }

    false
}

struct OverlayReplaySink {
    snapshots: Arc<StdMutex<Vec<String>>>,
}

impl DeltaSink for OverlayReplaySink {
    fn apply(&self, delta: &TranscriptDelta) {
        append_transcription_delta_impl(&delta.delta);
        let visible = overlay_visible_text_now();
        self.snapshots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(visible);
    }
}

#[test]
fn test_overlay_button_routes_are_distinct_from_settings_handler() {
    let cases = [
        (
            OverlayActionButtonRole::FormatPaste,
            FormatPhase::Idle,
            OverlayButtonAction::Format,
            "onFormatTranscript:",
        ),
        (
            OverlayActionButtonRole::FormatPaste,
            FormatPhase::Formatted,
            OverlayButtonAction::Copy,
            "onCopyTranscript:",
        ),
        (
            OverlayActionButtonRole::Copy,
            FormatPhase::Idle,
            OverlayButtonAction::Copy,
            "onCopyTranscript:",
        ),
        (
            OverlayActionButtonRole::Copy,
            FormatPhase::Formatted,
            OverlayButtonAction::Agent,
            "onAgentTranscript:",
        ),
        (
            OverlayActionButtonRole::AgentClose,
            FormatPhase::Idle,
            OverlayButtonAction::Agent,
            "onAgentTranscript:",
        ),
        (
            OverlayActionButtonRole::AgentClose,
            FormatPhase::Formatted,
            OverlayButtonAction::Close,
            "onCloseTranscript:",
        ),
        (
            OverlayActionButtonRole::Finish,
            FormatPhase::Idle,
            OverlayButtonAction::Finish,
            "onCommitRecording:",
        ),
    ];

    for (role, phase, expected_action, expected_selector) in cases {
        let route = overlay_button_route(role, phase);
        assert_eq!(route.action, expected_action);
        assert_eq!(route.selector_name, expected_selector);
        assert_ne!(route.selector_name, SETTINGS_SELECTOR_NAME);
    }

    assert_eq!(
        [
            overlay_button_route(OverlayActionButtonRole::FormatPaste, FormatPhase::Idle).action,
            overlay_button_route(OverlayActionButtonRole::Copy, FormatPhase::Idle).action,
            overlay_button_route(OverlayActionButtonRole::AgentClose, FormatPhase::Idle).action,
        ],
        [
            OverlayButtonAction::Format,
            OverlayButtonAction::Copy,
            OverlayButtonAction::Agent
        ]
    );
    assert_eq!(
        [
            overlay_button_route(OverlayActionButtonRole::FormatPaste, FormatPhase::Formatted)
                .action,
            overlay_button_route(OverlayActionButtonRole::Copy, FormatPhase::Formatted).action,
            overlay_button_route(OverlayActionButtonRole::AgentClose, FormatPhase::Formatted)
                .action,
        ],
        [
            OverlayButtonAction::Copy,
            OverlayButtonAction::Agent,
            OverlayButtonAction::Close
        ]
    );
    assert_eq!(
        decision_hint_text(
            TranscriptionActionContractMode::AiFormat,
            FormatPhase::Formatted,
            "",
            true
        ),
        "Dictation overlay | FORMATTED | Copy · Agent · Close"
    );
}

#[test]
fn test_transcript_text_view_editability_policy() {
    assert!(transcript_text_view_editable(true, FormatPhase::Idle));
    assert!(!transcript_text_view_editable(false, FormatPhase::Idle));
    assert!(!transcript_text_view_editable(
        true,
        FormatPhase::Formatting
    ));
    assert!(transcript_text_view_editable(false, FormatPhase::Formatted));
}

#[test]
fn test_transcription_text() {
    // Just verify the function doesn't panic
    let _ = get_transcription_text();
}

/// The dictation overlay is a borderless floating window. A plain borderless
/// `NSWindow` returns `canBecomeKeyWindow = NO`, which silently blocks all
/// keyboard input to the transcript `NSTextView` — so `setEditable: true` would
/// be a visual lie. Verify the overlay window subclass opts into key/main
/// status so the transcript is genuinely editable.
#[test]
fn overlay_window_subclass_is_keyable() {
    use objc::runtime::Sel;
    use objc::{msg_send, sel, sel_impl};

    let class = super::window::overlay_window_class();
    assert!(!class.is_null(), "overlay window class should register");

    let key_sel: Sel = sel!(canBecomeKeyWindow);
    let main_sel: Sel = sel!(canBecomeMainWindow);
    // SAFETY: querying the runtime whether instances respond to a selector.
    let responds_key: bool = unsafe { msg_send![class, instancesRespondToSelector: key_sel] };
    let responds_main: bool = unsafe { msg_send![class, instancesRespondToSelector: main_sel] };
    assert!(
        responds_key,
        "overlay window must override canBecomeKeyWindow"
    );
    assert!(
        responds_main,
        "overlay window must override canBecomeMainWindow"
    );
}

#[test]
#[serial]
fn test_current_segment_text_smoke() {
    // current_segment_text() is the pub accessor used by controller's
    // commit_segment to read overlay action-contract text. This smoke test
    // verifies the lock-acquire + Raw/AiFormat branch path doesn't panic
    // on default state, and that the returned String matches whatever the
    // private action_text_for_contract reads under the same lock.
    let direct = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        action_text_for_contract(&state)
    };
    let via_pub = current_segment_text();
    assert_eq!(direct, via_pub);
}

#[test]
fn test_overlay_config_default() {
    let config = TranscriptionOverlayConfig::default();
    assert_eq!(config.width, 420.0);
    assert_eq!(config.height, 180.0);
}

#[test]
fn test_is_overlay_visible_returns_bool() {
    // Just verify the function returns a bool without panic
    let visible = is_transcription_overlay_visible();
    let _ = visible;
}

#[test]
#[serial]
fn test_auto_hide_generation() {
    // Test that generation counter increments
    let gen1 = AUTO_HIDE_GENERATION.load(Ordering::SeqCst);
    AUTO_HIDE_GENERATION.fetch_add(1, Ordering::SeqCst);
    let gen2 = AUTO_HIDE_GENERATION.load(Ordering::SeqCst);
    assert_eq!(gen2, gen1 + 1);
}

#[test]
fn test_auto_hide_delay_seconds() {
    assert_eq!(
        parse_auto_hide_delay_secs(None),
        DEFAULT_AUTO_HIDE_DELAY_SECS
    );
    assert_eq!(
        parse_auto_hide_delay_secs(Some("2")),
        MIN_AUTO_HIDE_DELAY_SECS
    );
    assert_eq!(
        parse_auto_hide_delay_secs(Some("999")),
        MAX_AUTO_HIDE_DELAY_SECS
    );
    assert_eq!(parse_auto_hide_delay_secs(Some("18")), 18);
}

#[test]
#[serial]
fn test_auto_hide_hover_guard() {
    AUTO_HIDE_GENERATION.store(42, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(true, Ordering::SeqCst);

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = true;
    }
    assert!(!should_auto_hide(42));

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = false;
    }
    assert!(should_auto_hide(42));
}

#[test]
#[serial]
fn test_auto_hide_suppresses_only_in_progress_formatting() {
    reset_overlay_state_for_test();
    AUTO_HIDE_GENERATION.store(7, Ordering::SeqCst);
    AUTO_HIDE_PENDING.store(true, Ordering::SeqCst);

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hover_active = false;
        state.format_phase = FormatPhase::Formatting;
    }
    assert!(!should_auto_hide(7));

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.format_phase = FormatPhase::Formatted;
    }
    assert!(should_auto_hide(7));

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.format_phase = FormatPhase::Idle;
    }
    assert!(should_auto_hide(7));
}

#[test]
fn test_layout_metrics_scroll_transition() {
    let min_height = OVERLAY_WINDOW_MIN_HEIGHT;
    let max_height = min_height + 80.0;

    let compact = compute_overlay_layout_metrics(40.0, min_height, max_height);
    assert!(!compact.needs_scroll);
    assert!(compact.target_height >= min_height);

    let grown = compute_overlay_layout_metrics(120.0, min_height, max_height);
    assert!(!grown.needs_scroll);
    assert!(grown.target_height > compact.target_height);

    let overflow = compute_overlay_layout_metrics(420.0, min_height, max_height);
    assert!((overflow.target_height - max_height).abs() < f64::EPSILON);
    assert!(overflow.needs_scroll);
    assert!(overflow.text_document_height > overflow.text_viewport_height);
}

#[test]
fn test_layout_metrics_mobile_like_compact_window() {
    let min_height = OVERLAY_WINDOW_MIN_HEIGHT;
    let max_height = min_height + 24.0;

    let compact = compute_overlay_layout_metrics(360.0, min_height, max_height);
    assert!((compact.target_height - max_height).abs() < f64::EPSILON);
    assert!(compact.needs_scroll);
    assert!(compact.text_viewport_height >= OVERLAY_TEXT_MIN_HEIGHT);
}

#[test]
fn test_overlay_status_labels_are_canonical() {
    assert_eq!(
        overlay_status_label(status_from_detail("Listening...")),
        "Listening"
    );
    assert_eq!(
        overlay_status_label(status_from_detail("Thinking...")),
        "Thinking"
    );
    assert_eq!(overlay_status_label(status_from_detail("Idle")), "Idle");
    assert_eq!(
        overlay_status_label(status_from_detail("Backend failed")),
        "Error"
    );
    assert_eq!(overlay_status_label(status_from_detail("??")), "Idle");
}

#[test]
#[serial]
fn test_action_text_uses_raw_contract_source_in_raw_mode() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.action_contract_mode = TranscriptionActionContractMode::Raw;
    state.raw_text = "raw transcript".to_string();
    state.accumulated_text = "overlay preview".to_string();
    state.last_pass_text = "final last-pass".to_string();

    let text = action_text_for_contract(&state);
    assert_eq!(text, "raw transcript");
}

#[test]
#[serial]
fn test_augment_decision_mode_hands_off_existing_transcript() {
    reset_overlay_state_for_test();
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.decision_mode = true;
    state.action_contract_mode = TranscriptionActionContractMode::Raw;
    state.raw_text = "saved decision transcript".to_string();

    assert_eq!(
        augment_action_for_state(&state),
        Some(AugmentAction::HandoffDecisionText(
            "saved decision transcript".to_string()
        ))
    );
}

#[test]
#[serial]
fn test_augment_live_recording_commits_current_segment() {
    reset_overlay_state_for_test();
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.decision_mode = false;
    state.action_contract_mode = TranscriptionActionContractMode::Raw;
    state.raw_text = "live segment transcript".to_string();

    assert_eq!(
        augment_action_for_state(&state),
        Some(AugmentAction::CommitLiveSegment)
    );
}

#[test]
#[serial]
fn test_action_text_uses_last_pass_contract_source_in_ai_mode() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
    state.raw_text = "raw transcript".to_string();
    state.accumulated_text = "overlay preview".to_string();
    state.last_pass_text = "final last-pass".to_string();

    let text = action_text_for_contract(&state);
    assert_eq!(text, "final last-pass");
}

#[test]
#[serial]
fn test_action_text_ai_mode_returns_empty_when_last_pass_empty() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
    state.raw_text = "raw transcript".to_string();
    state.accumulated_text = "overlay preview".to_string();
    state.last_pass_text.clear();

    let text = action_text_for_contract(&state);
    assert!(text.is_empty());
}

#[test]
#[serial]
fn test_manual_overlay_edit_updates_raw_contract_and_blocks_deltas() {
    reset_overlay_state_for_test();
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.action_contract_mode = TranscriptionActionContractMode::Raw;
        state.accumulated_text = "server text".to_string();
        state.raw_text = "server text".to_string();
        apply_user_edit_to_state(&mut state, "manual edit".to_string());
    }

    append_transcription_delta_impl(" overwritten");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert!(state.user_edited);
    assert_eq!(state.accumulated_text, "manual edit");
    assert_eq!(state.raw_text, "manual edit");
}

#[test]
#[serial]
fn test_manual_overlay_edit_updates_ai_contract() {
    reset_overlay_state_for_test();
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
    state.last_pass_text = "formatted".to_string();

    apply_user_edit_to_state(&mut state, "formatted manual edit".to_string());

    assert_eq!(state.last_pass_text, "formatted manual edit");
    assert!(state.raw_text.is_empty());
    assert_eq!(action_text_for_contract(&state), "formatted manual edit");
}

#[test]
#[serial]
fn test_display_text_prefers_live_preview_over_action_contract() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.decision_mode = true;
    state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
    state.raw_text = "raw transcript".to_string();
    state.accumulated_text = "overlay preview".to_string();
    state.last_pass_text = "final last-pass".to_string();

    let text = display_text_for_state(&state);
    assert_eq!(text, "overlay preview");
}

#[test]
#[serial]
fn test_display_text_falls_back_to_action_contract_when_preview_empty() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.decision_mode = true;
    state.action_contract_mode = TranscriptionActionContractMode::AiFormat;
    state.raw_text = "raw transcript".to_string();
    state.accumulated_text.clear();
    state.last_pass_text = "final last-pass".to_string();

    let text = display_text_for_state(&state);
    assert_eq!(text, "final last-pass");
}

#[test]
fn test_stable_overlay_preview_text_keeps_complete_tail() {
    let text = "To jest stabilne zdanie.";
    assert_eq!(stable_overlay_preview_text(text), text);
}

#[test]
fn test_stable_overlay_preview_text_trims_partial_tail_word() {
    let text = "To jest stabilne zda";
    assert_eq!(stable_overlay_preview_text(text), "To jest stabilne ");
}

#[test]
fn test_stable_overlay_preview_text_without_boundary_returns_text() {
    assert_eq!(stable_overlay_preview_text("partial"), "partial");
}

/// Scoped env var guard — saves the prior value and restores it on Drop.
///
/// Required because `CODESCRIBE_OVERLAY_STABLE_PREVIEW` is read by
/// `overlay_live_preview_uses_stable_text()` as process-global state, so
/// parallel tests without isolation can observe values left over by siblings.
struct OverlayStablePreviewEnvGuard {
    prev: Option<String>,
}

impl OverlayStablePreviewEnvGuard {
    fn unset() -> Self {
        let prev = std::env::var("CODESCRIBE_OVERLAY_STABLE_PREVIEW").ok();
        // SAFETY: `#[serial]` on every caller enforces single-threaded access to
        // this env var for the duration of the test, and Drop restores the prior
        // value before any other test resumes.
        unsafe { std::env::remove_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW") };
        Self { prev }
    }
}

impl Drop for OverlayStablePreviewEnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: see OverlayStablePreviewEnvGuard::unset — serial test scope.
            Some(v) => unsafe { std::env::set_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW", v) },
            // SAFETY: see OverlayStablePreviewEnvGuard::unset — serial test scope.
            None => unsafe { std::env::remove_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW") },
        }
    }
}

#[test]
#[serial]
fn test_overlay_visible_text_decision_mode_uses_exact_text() {
    let _guard = OverlayStablePreviewEnvGuard::unset();
    let text = "pełny tekst kontraktu bez trimowania";
    assert_eq!(overlay_visible_text(text, true), text);
}

#[test]
#[serial]
fn test_overlay_visible_text_live_mode_defaults_to_exact_text() {
    let _guard = OverlayStablePreviewEnvGuard::unset();
    let text = "To jest stabilne zda";
    assert_eq!(overlay_visible_text(text, false), text);
}

#[tokio::test]
#[serial]
async fn overlay_real_flow_from_canonical_assets_never_collapses_to_1_3_words() {
    if !overlay_real_flow_enabled() {
        eprintln!(
            "Skipping overlay real-flow E2E (set {}=1 to enable)",
            OVERLAY_REAL_FLOW_OPT_IN_ENV
        );
        return;
    }

    if let Err(err) = codescribe_core::stt::whisper::singleton::get_model_path() {
        eprintln!("Skipping overlay real-flow E2E: local Whisper model unavailable: {err}");
        return;
    }

    let cases = canonical_overlay_cases();
    if cases.is_empty() {
        eprintln!("Skipping overlay real-flow E2E: no canonical data assets found");
        return;
    }

    let previous_stable_preview = std::env::var("CODESCRIBE_OVERLAY_STABLE_PREVIEW").ok();
    unsafe {
        std::env::set_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW", "0");
    }

    for (audio_path, reference_path) in cases {
        reset_overlay_state_for_test();

        let (samples, sample_rate) =
            load_audio_file(&audio_path).expect("load canonical audio asset");
        let events = collect_buffered_engine_events(&samples, sample_rate, Some("pl".to_string()))
            .await
            .expect("collect engine events for overlay replay");

        assert!(
            events
                .iter()
                .any(|event| matches!(event, EngineEvent::Preview { .. })),
            "expected Preview events for canonical asset {}",
            audio_path.display()
        );

        let expected_final = final_transcript_from_events(&events);
        assert!(
            !expected_final.trim().is_empty(),
            "expected non-empty final transcript for {}",
            audio_path.display()
        );

        let transcript_buffer = Arc::new(Mutex::new(String::new()));
        let snapshots = Arc::new(StdMutex::new(Vec::<String>::new()));
        let sink: Arc<dyn DeltaSink> = Arc::new(OverlayReplaySink {
            snapshots: Arc::clone(&snapshots),
        });
        let mut emitter = PresentationEmitter::new(transcript_buffer, Some(sink), None);

        for event in &events {
            emitter.on_event(event);
        }
        emitter.finish().await;

        let final_visible = overlay_visible_text_now();
        let snapshot_list = snapshots.lock().unwrap_or_else(|e| e.into_inner()).clone();

        assert!(
            !snapshot_list.is_empty(),
            "expected visible overlay snapshots for {}",
            audio_path.display()
        );
        assert!(
            !has_one_to_three_word_collapse(&snapshot_list),
            "overlay collapsed to 1-3 words for {} (reference excerpt: {})",
            audio_path.display(),
            human_reference_excerpt(&reference_path)
        );
        assert_eq!(
            normalize_overlay_text(&final_visible),
            normalize_overlay_text(&expected_final),
            "overlay final visible transcript diverged for {}",
            audio_path.display()
        );
    }

    match previous_stable_preview {
        Some(value) => unsafe { std::env::set_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW", value) },
        None => unsafe { std::env::remove_var("CODESCRIBE_OVERLAY_STABLE_PREVIEW") },
    }
    reset_overlay_state_for_test();
}
