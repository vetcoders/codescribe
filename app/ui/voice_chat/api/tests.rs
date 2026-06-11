use super::*;
use crate::ui_helpers::{BubbleRole, RenderMode, streaming_render_mode};
use serial_test::serial;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn sample_drawer_entry(
    path: &str,
    preview: &str,
    mode: TranscriptionMode,
    is_ai_formatted: bool,
    is_favorite: bool,
) -> DrawerEntry {
    let mode_label = match mode {
        TranscriptionMode::Hold => "Ctrl+Hold",
        TranscriptionMode::Assistive => "Shift/Cmd",
        TranscriptionMode::Toggle => "Toggle",
        TranscriptionMode::Conversation => "Moshi",
    };
    let entry_type = if is_ai_formatted { "AI" } else { "Tt" };
    let search_corpus = format!("{entry_type} {mode_label} {path} {preview}").to_ascii_lowercase();
    DrawerEntry {
        source: DrawerEntrySource::LegacyFile,
        path: PathBuf::from(path),
        timestamp: SystemTime::now(),
        mode,
        preview: preview.to_string(),
        search_corpus,
        is_ai_formatted,
        is_favorite,
    }
}

#[test]
fn filtered_drawer_entries_matches_preview_path_and_title_case_insensitively() {
    let mut state = VoiceChatOverlayState {
        drawer_entries: vec![
            sample_drawer_entry(
                "meeting_notes.md",
                "Follow-up from team sync",
                TranscriptionMode::Hold,
                false,
                false,
            ),
            sample_drawer_entry(
                "roadmap.md",
                "Architecture review memo",
                TranscriptionMode::Assistive,
                true,
                true,
            ),
        ],
        ..Default::default()
    };

    assert_eq!(filtered_drawer_entries(&state, "TEAM").len(), 1);
    assert_eq!(filtered_drawer_entries(&state, "MEETING_NOTES").len(), 1);
    assert_eq!(filtered_drawer_entries(&state, "shift/cmd").len(), 1);
    assert_eq!(filtered_drawer_entries(&state, "AI").len(), 1);

    state.drawer_favorites_only = true;
    assert_eq!(filtered_drawer_entries(&state, "").len(), 1);
}

#[test]
fn filtered_drawer_entries_returns_empty_when_query_has_no_match() {
    let state = VoiceChatOverlayState {
        drawer_entries: vec![
            sample_drawer_entry(
                "draft-a.md",
                "First transcript snippet",
                TranscriptionMode::Hold,
                false,
                false,
            ),
            sample_drawer_entry(
                "draft-b.md",
                "Second transcript snippet",
                TranscriptionMode::Toggle,
                false,
                false,
            ),
        ],
        ..Default::default()
    };

    assert!(filtered_drawer_entries(&state, "missing phrase").is_empty());
}

#[test]
fn filtered_drawer_entries_clear_query_restores_full_list() {
    let state = VoiceChatOverlayState {
        drawer_entries: vec![
            sample_drawer_entry("first.md", "alpha", TranscriptionMode::Hold, false, false),
            sample_drawer_entry(
                "second.md",
                "beta",
                TranscriptionMode::Assistive,
                false,
                true,
            ),
        ],
        ..Default::default()
    };

    assert_eq!(filtered_drawer_entries(&state, "alpha").len(), 1);
    assert_eq!(filtered_drawer_entries(&state, "").len(), 2);
    assert_eq!(filtered_drawer_entries(&state, "   ").len(), 2);
}

#[test]
fn filtered_drawer_entries_keeps_original_indices_for_card_actions() {
    let state = VoiceChatOverlayState {
        drawer_entries: vec![
            sample_drawer_entry("first.md", "alpha", TranscriptionMode::Hold, false, false),
            sample_drawer_entry(
                "second.md",
                "alpha",
                TranscriptionMode::Assistive,
                false,
                false,
            ),
            sample_drawer_entry("third.md", "alpha", TranscriptionMode::Toggle, false, false),
        ],
        ..Default::default()
    };

    let visible = filtered_drawer_entries(&state, "third");
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].0, 2);
}

#[test]
fn filtered_drawer_entries_matches_thread_message_and_note_corpus() {
    let state = VoiceChatOverlayState {
        drawer_entries: vec![DrawerEntry {
            source: DrawerEntrySource::Thread {
                id: "t_2026-02-23_abc123".to_string(),
            },
            path: PathBuf::from("thread_t_2026-02-23_abc123.json"),
            timestamp: SystemTime::now(),
            mode: TranscriptionMode::Assistive,
            preview: "clinical recap".to_string(),
            search_corpus: "renal values improved call owner tomorrow".to_string(),
            is_ai_formatted: true,
            is_favorite: false,
        }],
        ..Default::default()
    };

    assert_eq!(filtered_drawer_entries(&state, "renal values").len(), 1);
    assert_eq!(filtered_drawer_entries(&state, "call owner").len(), 1);
    assert_eq!(filtered_drawer_entries(&state, "missing phrase").len(), 0);
}

#[test]
fn drawer_unavailable_placeholder_entry_has_expected_metadata() {
    let entry = thread_history_unavailable_drawer_entry();

    assert!(matches!(entry.source, DrawerEntrySource::LegacyFile));
    assert!(entry.path.as_os_str().is_empty());
    assert_eq!(entry.preview, "Thread history unavailable — storage error");
    assert!(!entry.is_ai_formatted);
    assert!(entry.search_corpus.contains("unavailable"));
    assert!(entry.search_corpus.contains("error"));
    assert!(is_drawer_unavailable_placeholder(&entry));
    assert!(drawer_entry_matches_query(&entry, "unavailable"));
    assert!(drawer_entry_matches_query(&entry, "error"));
}

#[test]
fn drawer_entry_matches_query_does_not_leak_absolute_path() {
    // Operator regression 2026-05-24: search field "codescribe" should NOT
    // match a ThreadStore entry whose preview/corpus do not contain
    // "codescribe", even though the entry's absolute path lives under
    // `~/.codescribe/threads/...`. Path pollution must not bypass the filter.
    let entry = DrawerEntry {
        source: DrawerEntrySource::Thread {
            id: "t_2026-04-21_h15b5t".to_string(),
        },
        path: PathBuf::from(
            "/Users/maciejgad/Library/Application Support/CodeScribe/threads/thread_t_2026-04-21_h15b5t.json",
        ),
        timestamp: SystemTime::now(),
        mode: TranscriptionMode::Assistive,
        preview: "ai failed output_text".to_string(),
        search_corpus:
            "threadstore source:thread assistive thread:t_2026-04-21_h15b5t ai failed output_text"
                .to_string(),
        is_ai_formatted: false,
        is_favorite: false,
    };
    // Negative cases — these strings exist ONLY in the leaked absolute path,
    // not in any legitimate search vector.
    assert!(
        !drawer_entry_matches_query(&entry, "codescribe"),
        "absolute path component leaked into haystack — filter would match all threads",
    );
    assert!(
        !drawer_entry_matches_query(&entry, "library"),
        "absolute path component leaked into haystack",
    );
    assert!(
        !drawer_entry_matches_query(&entry, "application support"),
        "absolute path component leaked into haystack",
    );
    // Positive cases — legitimate vectors still match.
    assert!(drawer_entry_matches_query(&entry, "ai failed"));
    assert!(drawer_entry_matches_query(&entry, "t_2026-04-21"));
    assert!(drawer_entry_matches_query(&entry, "thread"));
    assert!(drawer_entry_matches_query(&entry, "assistive"));
    assert!(drawer_entry_matches_query(&entry, "threadstore"));
}

#[test]
#[serial]
fn runtime_degraded_status_persists_across_status_updates() {
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }

    set_voice_chat_runtime_degraded_impl(
        true,
        Some("Legacy formatter fallback is active.".to_string()),
    );
    update_voice_chat_status_impl("Sending...");

    {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(state.runtime_degraded);
        assert!(state.is_agent_degraded);
        assert_eq!(state.status_base_text, "Sending...");
        assert_eq!(state.status_kind, UiStatus::Error);
        assert!(state.status_text.contains("Runtime degraded"));
    }

    set_voice_chat_runtime_degraded_impl(false, None);

    {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!state.runtime_degraded);
        assert!(!state.is_agent_degraded);
        assert_eq!(state.status_text, "Sending...");
        assert_eq!(state.status_kind, UiStatus::Processing);
    }

    update_voice_chat_status_impl("AI Response:");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(state.status_base_text, "AI Response:");
    assert_eq!(state.status_text, "AI Response:");
    assert_eq!(state.status_kind, UiStatus::Idle);
}

#[test]
#[serial]
fn clear_agent_thinking_state_restores_ready_status() {
    let mut state = VoiceChatOverlayState {
        is_agent_thinking: true,
        status_base_text: "Thinking…".to_string(),
        status_text: "Thinking…".to_string(),
        status_kind: UiStatus::Processing,
        ..VoiceChatOverlayState::default()
    };

    clear_agent_thinking_state(&mut state);

    assert!(!state.is_agent_thinking);
    assert_eq!(state.status_base_text, "Ready");
    assert_eq!(state.status_text, "Ready");
    assert_eq!(state.status_kind, UiStatus::Idle);
}

/// Regression: `set_voice_chat_agent_thinking` holds `OVERLAY_STATE` while
/// refreshing status. The old body called `update_voice_chat_status_impl`
/// there, which re-locked the same non-reentrant mutex on the same thread
/// and froze the main thread in `__psynch_mutexwait`. `apply_agent_thinking`
/// must mutate the held guard without re-locking. Run on a worker thread
/// with a watchdog so a regression surfaces as a timeout instead of hanging
/// the whole test run.
#[test]
#[serial]
fn apply_agent_thinking_does_not_relock_overlay_state() {
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        apply_agent_thinking(&mut state, true);
        let snapshot = (state.is_agent_thinking, state.status_base_text.clone());
        drop(state);
        let _ = tx.send(snapshot);
    });

    let (is_thinking, base_text) = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("apply_agent_thinking re-locked OVERLAY_STATE (re-entrant deadlock)");
    worker.join().unwrap();

    assert!(is_thinking);
    assert_eq!(base_text, "Thinking…");
}

#[test]
fn drawer_entry_subtitle_marks_threadstore_index_only_when_path_missing() {
    let entry = DrawerEntry {
        source: DrawerEntrySource::Thread {
            id: "t_2026-02-23_missing".to_string(),
        },
        path: PathBuf::from("__missing_thread_guardrail_test__.json"),
        timestamp: SystemTime::now(),
        mode: TranscriptionMode::Assistive,
        preview: "summary".to_string(),
        search_corpus: "summary".to_string(),
        is_ai_formatted: true,
        is_favorite: false,
    };

    let subtitle = drawer_entry_subtitle(&entry);
    assert!(subtitle.contains("ThreadStore (index-only)"));
    assert!(subtitle.contains("thread:t_2026-02-23_missing"));
}

#[test]
fn thread_messages_for_restore_maps_thread_roles_and_text() {
    let now = chrono::Utc::now();
    let thread = Thread {
        id: "t_2026-06-02_restore".to_string(),
        created_at: now,
        updated_at: now,
        title: "Clinical thread".to_string(),
        mode: "assistive".to_string(),
        tags: Vec::new(),
        notes: Vec::new(),
        messages: vec![
            codescribe_core::agent::ThreadMessage {
                role: "user".to_string(),
                content: vec![serde_json::json!({
                    "type": "input_text",
                    "text": "Summarize labs"
                })],
                timestamp: now,
                metadata: None,
            },
            codescribe_core::agent::ThreadMessage {
                role: "assistant".to_string(),
                content: vec![serde_json::json!({
                    "type": "output_text",
                    "text": "WBC improved."
                })],
                timestamp: now,
                metadata: None,
            },
        ],
        summary: None,
        total_tokens: None,
        provider: "openai-responses".to_string(),
        model: "gpt-5".to_string(),
    };

    let messages = thread_messages_for_restore(&thread);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, ChatRole::User);
    assert_eq!(messages[0].text, "Summarize labs");
    assert_eq!(messages[0].mode.as_deref(), Some("Shift/Cmd"));
    assert_eq!(messages[1].role, ChatRole::Assistant);
    assert_eq!(messages[1].text, "WBC improved.");
    assert!(!messages[1].is_streaming);
}

#[test]
fn display_text_for_message_handles_streaming() {
    let streaming_empty = ChatMessage {
        role: ChatRole::Assistant,
        text: String::new(),
        is_streaming: true,
        is_collapsed: false,
        is_error: false,
        timestamp: SystemTime::now(),
        mode: None,
    };
    assert_eq!(display_text_for_message(&streaming_empty), "• • •");

    let streaming = ChatMessage {
        text: "hello".to_string(),
        ..streaming_empty
    };
    assert_eq!(display_text_for_message(&streaming), "hello …");

    let finished = ChatMessage {
        is_streaming: false,
        is_collapsed: false,
        ..streaming
    };
    assert_eq!(display_text_for_message(&finished), "hello");
}

#[test]
fn should_autoscroll_follows_pinned_state() {
    assert!(should_autoscroll(true));
    assert!(!should_autoscroll(false));
    assert!(VoiceChatOverlayState::default().scroll_pinned);
}

#[test]
fn render_mode_keeps_streaming_plain_and_final_assistant_system_markdown() {
    assert_eq!(
        streaming_render_mode(true, BubbleRole::User),
        RenderMode::Plain
    );
    assert_eq!(
        streaming_render_mode(true, BubbleRole::Assistant),
        RenderMode::Plain
    );
    assert_eq!(
        streaming_render_mode(true, BubbleRole::System),
        RenderMode::Plain
    );
    assert_eq!(
        streaming_render_mode(false, BubbleRole::User),
        RenderMode::Plain
    );
    assert_eq!(
        streaming_render_mode(false, BubbleRole::Assistant),
        RenderMode::Markdown
    );
    assert_eq!(
        streaming_render_mode(false, BubbleRole::System),
        RenderMode::Markdown
    );
}

#[test]
fn update_cached_stack_height_applies_last_bubble_delta() {
    assert_eq!(
        update_cached_stack_height(40.0, 75.0, Some(200.0)),
        Some(235.0)
    );
    assert_eq!(update_cached_stack_height(40.0, 75.0, None), None);
    assert_eq!(
        update_cached_stack_height(120.0, 10.0, Some(50.0)),
        Some(1.0)
    );
}

#[test]
fn scrolled_to_bottom_math_uses_visible_max_y_threshold() {
    assert!(scrolled_to_bottom_math(476.0, 300.0, 800.0, 24.0));
    assert!(scrolled_to_bottom_math(500.0, 300.0, 800.0, 24.0));
    assert!(!scrolled_to_bottom_math(450.0, 300.0, 800.0, 24.0));
    assert!(scrolled_to_bottom_math(0.0, 500.0, 300.0, 24.0));
}

#[test]
fn streaming_reasoning_collapses_when_finalized() {
    let mut state = VoiceChatOverlayState::default();
    state.messages.push(ChatMessage {
        role: ChatRole::Reasoning,
        text: "checking patient context".to_string(),
        is_streaming: true,
        is_collapsed: false,
        is_error: false,
        timestamp: SystemTime::now(),
        mode: Some("AI".to_string()),
    });
    state.active_reasoning_stream_index = Some(0);

    finalize_streaming_reasoning(&mut state);

    let message = &state.messages[0];
    assert!(!message.is_streaming);
    assert!(message.is_collapsed);
    assert_eq!(state.active_reasoning_stream_index, None);
    assert!(display_text_for_message(message).starts_with("Reasoning · "));
}

#[test]
fn drawer_top_scroll_y_matches_flippedness() {
    assert_eq!(drawer_top_scroll_y(900.0, 300.0, false), 600.0);
    assert_eq!(drawer_top_scroll_y(900.0, 300.0, true), 0.0);
    assert_eq!(drawer_top_scroll_y(200.0, 300.0, false), 0.0);
}

#[test]
fn update_active_tab_switches_between_drawer_and_agent() {
    let mut state = VoiceChatOverlayState::default();
    update_active_tab_locked(&mut state, Tab::Agent);
    assert_eq!(state.active_tab, Tab::Agent);

    update_active_tab_locked(&mut state, Tab::Drawer);
    assert_eq!(state.active_tab, Tab::Drawer);
}

#[test]
#[serial]
fn handoff_transcript_to_chat_adds_user_message_without_callback() {
    {
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = None;
    }
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }

    handoff_transcript_to_chat_impl("transcript payload");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].role, ChatRole::User);
    assert_eq!(state.messages[0].text, "transcript payload");
    assert!(
        !state.is_sending,
        "without callback, handoff must not stay in sending state"
    );
}

#[test]
#[serial]
fn handoff_transcript_to_chat_invokes_callback() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(String::new()));
    {
        let count = Arc::clone(&call_count);
        let observed = Arc::clone(&observed);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |text: String| {
            count.fetch_add(1, Ordering::SeqCst);
            let mut guard = observed.lock().unwrap_or_else(|e| e.into_inner());
            *guard = text;
        }));
    }
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }

    handoff_transcript_to_chat_impl("augment this");

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    let payload = observed.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(payload, "augment this");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(state.messages.len(), 1);
    assert!(state.is_sending);

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

#[test]
#[serial]
fn dispatch_voice_chat_send_returns_false_without_callback() {
    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
    drop(cb);

    assert!(!dispatch_voice_chat_send("payload"));
    assert!(!dispatch_voice_chat_send("   "));
}

#[test]
#[serial]
fn dispatch_voice_chat_send_invokes_callback() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(String::new()));
    {
        let count = Arc::clone(&call_count);
        let observed = Arc::clone(&observed);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |text: String| {
            count.fetch_add(1, Ordering::SeqCst);
            let mut guard = observed.lock().unwrap_or_else(|e| e.into_inner());
            *guard = text;
        }));
    }

    assert!(dispatch_voice_chat_send("runtime payload"));
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    let payload = observed.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(payload, "runtime payload");

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}
