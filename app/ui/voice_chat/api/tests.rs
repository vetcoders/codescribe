use super::*;
use crate::ui_helpers::{BubbleRole, RenderMode, next_render_mode, streaming_render_mode};
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
        title: None,
        model: None,
        total_tokens: None,
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
fn prompt_history_cursor_walks_previous_and_next() {
    assert_eq!(next_prompt_history_cursor(0, None, true), None);
    assert_eq!(next_prompt_history_cursor(3, None, true), Some(2));
    assert_eq!(next_prompt_history_cursor(3, Some(2), true), Some(1));
    assert_eq!(next_prompt_history_cursor(3, Some(0), true), Some(0));
    assert_eq!(next_prompt_history_cursor(3, None, false), None);
    assert_eq!(next_prompt_history_cursor(3, Some(0), false), Some(1));
    assert_eq!(next_prompt_history_cursor(3, Some(2), false), Some(3));
}

#[test]
fn prompt_history_keeps_recent_unique_sent_prompts() {
    let mut state = VoiceChatOverlayState::default();

    push_prompt_history_locked(&mut state, " first ");
    push_prompt_history_locked(&mut state, "first");
    push_prompt_history_locked(&mut state, "second");
    push_prompt_history_locked(&mut state, "");

    assert_eq!(state.prompt_history, vec!["first", "second"]);
}

#[test]
fn manual_send_reenables_follow_latest_after_prior_scrollback() {
    let mut state = VoiceChatOverlayState {
        scroll_pinned: false,
        ..Default::default()
    };

    follow_latest_after_manual_send_locked(&mut state);

    assert!(should_autoscroll(state.scroll_pinned));
}

#[test]
fn drawer_filter_in_memory_returns_correct_subset_without_disk_io() {
    // P1.3: search-as-you-type must filter the already-loaded in-memory
    // `drawer_entries` snapshot, never re-read ThreadStore from disk per
    // keystroke. This exercises the same `filtered_drawer_entries` path the
    // debounced `filter_drawer` callback renders from, proving the filter is a
    // pure in-memory operation over a fixed `Vec<DrawerEntry>` (no I/O).
    let state = VoiceChatOverlayState {
        drawer_entries: vec![
            sample_drawer_entry(
                "alpha.md",
                "renal panel recap",
                TranscriptionMode::Hold,
                false,
                false,
            ),
            sample_drawer_entry(
                "beta.md",
                "surgical follow-up",
                TranscriptionMode::Assistive,
                false,
                false,
            ),
            sample_drawer_entry(
                "gamma.md",
                "renal recheck tomorrow",
                TranscriptionMode::Toggle,
                false,
                false,
            ),
        ],
        ..Default::default()
    };

    // Two entries contain "renal", original indices preserved for card actions.
    let renal = filtered_drawer_entries(&state, "renal");
    assert_eq!(renal.len(), 2);
    assert_eq!(renal[0].0, 0);
    assert_eq!(renal[1].0, 2);

    // Single match.
    let surgical = filtered_drawer_entries(&state, "surgical");
    assert_eq!(surgical.len(), 1);
    assert_eq!(surgical[0].0, 1);

    // No match → empty (still no I/O).
    assert!(filtered_drawer_entries(&state, "cardiology").is_empty());

    // Empty query → full set unchanged.
    assert_eq!(filtered_drawer_entries(&state, "").len(), 3);
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
            title: Some("Clinical recap".to_string()),
            model: Some("gpt-5".to_string()),
            total_tokens: Some(1536),
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
            "/Users/tester/Library/Application Support/CodeScribe/threads/thread_t_2026-04-21_h15b5t.json",
        ),
        timestamp: SystemTime::now(),
        mode: TranscriptionMode::Assistive,
        title: Some("AI failed output".to_string()),
        model: None,
        total_tokens: None,
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
        title: Some("Missing thread".to_string()),
        model: Some("gpt-5".to_string()),
        total_tokens: Some(2048),
        preview: "summary".to_string(),
        search_corpus: "summary".to_string(),
        is_ai_formatted: true,
        is_favorite: true,
    };

    let subtitle = drawer_entry_subtitle(&entry);
    assert!(subtitle.contains("Shift/Cmd"));
    assert!(subtitle.contains("gpt-5"));
    assert!(subtitle.contains("2.0k tok"));
    assert!(subtitle.contains("★"));
}

#[test]
fn drawer_entry_subtitle_omits_empty_optional_metadata() {
    let subtitle = format_drawer_subtitle("4 h", "Toggle", None, None, false);

    assert_eq!(subtitle, "4 h • Toggle");
}

#[test]
fn drawer_entry_title_prefers_thread_title_then_preview() {
    let mut entry = sample_drawer_entry(
        "thread.md",
        "preview fallback text",
        TranscriptionMode::Assistive,
        true,
        false,
    );
    entry.title = Some("Clinical plan".to_string());

    assert_eq!(drawer_entry_title(&entry), "Clinical plan");

    entry.title = Some("   ".to_string());
    assert_eq!(drawer_entry_title(&entry), "preview fallback text");
}

#[test]
fn drawer_entry_title_compacts_absolute_path_titles() {
    let mut entry = sample_drawer_entry(
        "thread.md",
        "preview fallback text",
        TranscriptionMode::Assistive,
        true,
        false,
    );
    entry.title =
        Some("/Users/tester/Library/Application Support/CodeScribe/thread.json".to_string());

    assert_eq!(drawer_entry_title(&entry), "thread.json");
}

#[test]
fn drawer_row_action_layout_reserves_title_truncation_space() {
    let layout = drawer_row_action_layout(280.0);

    assert!(layout.title_width >= 24.0);
    assert!(
        layout.title_x + layout.title_width < layout.actions_x,
        "title must end before hover action buttons begin"
    );
    assert_eq!(layout.text_column_x, layout.title_x);
    assert!(
        layout.text_column_width > layout.title_width,
        "preview/subtitle should breathe under the title instead of starting under the badge"
    );
    assert_eq!(
        layout.actions_width,
        ui_tokens::DRAWER_ACTION_BUTTON_SIZE * 4.0 + ui_tokens::DRAWER_ACTION_BUTTON_GAP * 3.0
    );
}

#[test]
fn section_for_groups_drawer_entries_by_local_day_boundaries() {
    let today = chrono::Local::now().date_naive();
    let now_local = today
        .and_hms_opt(12, 0, 0)
        .expect("valid noon timestamp")
        .and_local_timezone(chrono::Local)
        .single()
        .expect("local noon must be unambiguous");
    let now = SystemTime::from(now_local);

    let today_early = today
        .and_hms_opt(0, 1, 0)
        .expect("valid today timestamp")
        .and_local_timezone(chrono::Local)
        .single()
        .expect("local today timestamp must be unambiguous");
    let yesterday_late = (today - chrono::Duration::days(1))
        .and_hms_opt(23, 59, 0)
        .expect("valid yesterday timestamp")
        .and_local_timezone(chrono::Local)
        .single()
        .expect("local yesterday timestamp must be unambiguous");
    let six_days = (today - chrono::Duration::days(6))
        .and_hms_opt(12, 0, 0)
        .expect("valid six-day timestamp")
        .and_local_timezone(chrono::Local)
        .single()
        .expect("local six-day timestamp must be unambiguous");
    let eight_days = (today - chrono::Duration::days(8))
        .and_hms_opt(12, 0, 0)
        .expect("valid eight-day timestamp")
        .and_local_timezone(chrono::Local)
        .single()
        .expect("local eight-day timestamp must be unambiguous");

    assert_eq!(
        section_for(SystemTime::from(today_early), now),
        DrawerSection::Today
    );
    assert_eq!(
        section_for(SystemTime::from(yesterday_late), now),
        DrawerSection::Yesterday
    );
    assert_eq!(
        section_for(SystemTime::from(six_days), now),
        DrawerSection::ThisWeek
    );
    assert_eq!(
        section_for(SystemTime::from(eight_days), now),
        DrawerSection::Older
    );
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
        is_pending_followup: false,
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
fn render_mode_defaults_every_bubble_to_plain() {
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
        RenderMode::Plain
    );
    assert_eq!(
        streaming_render_mode(false, BubbleRole::System),
        RenderMode::Plain
    );
}

#[test]
fn next_render_mode_toggles_between_raw_and_rich() {
    assert_eq!(next_render_mode(RenderMode::Plain), RenderMode::Markdown);
    assert_eq!(next_render_mode(RenderMode::Markdown), RenderMode::Plain);
}

#[test]
#[serial]
fn finalize_assistant_message_state_only_preserves_render_mode_override() {
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: "**raw stays selected**".to_string(),
            is_streaming: true,
            is_collapsed: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some("AI".to_string()),
            is_pending_followup: false,
        });
        state.active_assistant_stream_index = Some(0);
        state.message_render_modes.insert(0, RenderMode::Markdown);
    }

    finalize_assistant_message_state_only_impl(false);

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        state.message_render_modes.get(&0).copied(),
        Some(RenderMode::Markdown)
    );
    assert!(!state.messages[0].is_streaming);
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
fn agent_document_height_preserves_bottom_clearance_above_input_bar() {
    let stack_height = 920.0;
    let bottom_inset = 96.0;
    let clearance = AGENT_SCROLL_BOTTOM_CLEARANCE;
    let document_height =
        agent_document_height_for_bottom_clearance(stack_height, bottom_inset, clearance);

    assert_eq!(document_height, stack_height + bottom_inset + clearance);
    assert_eq!(
        agent_stack_height_from_document(
            stack_height + bottom_inset + AGENT_SCROLL_BOTTOM_CLEARANCE,
            bottom_inset,
        ),
        stack_height
    );
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
        is_pending_followup: false,
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

#[test]
#[serial]
fn toggle_callback_appends_finalized_utterances_to_user_draft() {
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }

    append_voice_chat_user_utterance_impl("Pierwsze moje myśli.");
    append_voice_chat_user_utterance_impl("Druga fraza już nie może zastąpić pierwszej.");
    append_voice_chat_user_utterance_impl("Trzecia zostaje na końcu.");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(state.messages.len(), 1);
    assert_eq!(
        state.messages[0].text,
        "Pierwsze moje myśli. Druga fraza już nie może zastąpić pierwszej. Trzecia zostaje na końcu."
    );
    assert!(state.messages[0].is_streaming);
}

#[test]
#[serial]
fn assistive_toggle_vad_end_appends_without_sending_until_explicit_send() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(String::new()));
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }
    {
        let count = Arc::clone(&call_count);
        let observed = Arc::clone(&observed);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |text: String| {
            count.fetch_add(1, Ordering::SeqCst);
            *observed.lock().unwrap_or_else(|e| e.into_inner()) = text;
        }));
    }

    append_voice_chat_user_utterance_impl("Pierwszy segment.");
    append_voice_chat_user_utterance_impl("Drugi segment.");

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        0,
        "VAD-end utterance append must not dispatch the draft to the agent"
    );
    {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].role, ChatRole::User);
        assert_eq!(state.messages[0].text, "Pierwszy segment. Drugi segment.");
        assert!(
            state.messages[0].is_streaming,
            "VAD-end keeps the user bubble open until toggle-stop finalizes/sends"
        );
    }

    assert!(dispatch_voice_chat_send("Pierwszy segment. Drugi segment."));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "toggle-stop/runtime boundary should send the accumulated transcript exactly once"
    );
    assert_eq!(
        observed.lock().unwrap_or_else(|e| e.into_inner()).as_str(),
        "Pierwszy segment. Drugi segment."
    );

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

#[test]
#[serial]
fn explicit_commit_sends_accumulated_draft_and_keeps_next_draft_open() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(String::new()));
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }
    {
        let count = Arc::clone(&call_count);
        let observed = Arc::clone(&observed);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |text: String| {
            count.fetch_add(1, Ordering::SeqCst);
            *observed.lock().unwrap_or_else(|e| e.into_inner()) = text;
        }));
    }

    append_voice_chat_user_utterance_impl("Pierwszy segment.");
    append_voice_chat_user_utterance_impl("Drugi segment.");
    finalize_user_message_state_only_impl();
    commit_last_user_message_impl();

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        observed.lock().unwrap_or_else(|e| e.into_inner()).as_str(),
        "Pierwszy segment. Drugi segment."
    );

    append_voice_chat_user_utterance_impl("Nowy segment po ciszy.");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(state.messages.len(), 2);
    assert!(!state.messages[0].is_streaming);
    assert_eq!(state.messages[1].text, "Nowy segment po ciszy.");
    assert!(state.messages[1].is_streaming);

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

#[test]
#[serial]
fn assistive_followup_busy_capture_waits_for_explicit_send() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: "Already sent prompt".to_string(),
            is_streaming: false,
            is_collapsed: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some("AI".to_string()),
            is_pending_followup: false,
        });
        state.is_sending = true;
    }
    {
        let count = Arc::clone(&call_count);
        let observed = Arc::clone(&observed);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |text: String| {
            count.fetch_add(1, Ordering::SeqCst);
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(text);
        }));
    }

    append_voice_chat_user_delta_impl("Follow-up while busy");
    append_voice_chat_user_delta_impl(" please");

    {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].text, "Already sent prompt");
        assert!(!state.messages[0].is_pending_followup);
        assert_eq!(state.messages[1].role, ChatRole::User);
        assert_eq!(state.messages[1].text, "Follow-up while busy please");
        assert!(state.messages[1].is_pending_followup);
        assert!(state.messages[1].is_streaming);
        assert!(
            message_metadata(&state.messages[1]).contains("Pending follow-up"),
            "metadata should expose the waiting affordance used by the bubble"
        );
    }
    assert_eq!(call_count.load(Ordering::SeqCst), 0);

    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = false;
        state.is_agent_thinking = false;
    }
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        0,
        "captured follow-up must not auto-send when the agent becomes idle"
    );

    commit_pending_followup_message_impl();

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_slice(),
        &["Follow-up while busy please".to_string()]
    );
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(state.messages.len(), 2);
    assert!(!state.messages[1].is_pending_followup);
    assert!(!state.messages[1].is_streaming);

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

#[test]
#[serial]
fn assistive_followup_finalization_keeps_pending_until_explicit_send() {
    let call_count = Arc::new(AtomicUsize::new(0));
    {
        let count = Arc::clone(&call_count);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |_text: String| {
            count.fetch_add(1, Ordering::SeqCst);
        }));
    }
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
        state.is_sending = true;
    }

    append_voice_chat_user_utterance_impl("Follow-up after first send.");
    finalize_user_message_state_only_impl();

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].text, "Follow-up after first send.");
    assert!(
        state.messages[0].is_pending_followup,
        "finalizing a busy follow-up must keep the explicit follow-up affordance"
    );
    assert!(
        !state.messages[0].is_streaming,
        "finalization may close transcription streaming without demoting the follow-up"
    );
    assert_eq!(state.active_user_stream_index, None);

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

#[test]
#[serial]
fn assistive_followup_text_finalization_keeps_pending_until_explicit_send() {
    let call_count = Arc::new(AtomicUsize::new(0));
    {
        let count = Arc::clone(&call_count);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |_text: String| {
            count.fetch_add(1, Ordering::SeqCst);
        }));
    }
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
        state.is_agent_thinking = true;
    }

    append_voice_chat_user_delta_impl("partial follow-up");
    finalize_user_message_impl("Final follow-up transcript");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].text, "Final follow-up transcript");
    assert!(
        state.messages[0].is_pending_followup,
        "text finalization must not demote a busy follow-up into a sent user bubble"
    );
    assert!(!state.messages[0].is_streaming);
    assert_eq!(state.active_user_stream_index, None);

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

#[test]
#[serial]
fn assistive_first_message_double_finalize_renders_single_user_bubble() {
    // Regression (fix/assistive-double-send): the assistive controller finalizes
    // the same first utterance twice — once on the full-rewrite render, once
    // again right before send (`set_voice_chat_user_text` → this impl). A cold
    // overlay has no active streaming index to reuse, so the second call used to
    // push a second identical User bubble, rendering the first message twice.
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
    }

    let text = "Dobra, wez zacznij w schowek i tylko przyjmij do wiadomosci.";
    finalize_user_message_impl(text);
    finalize_user_message_impl(text);

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        state.messages.len(),
        1,
        "two finalizes of the same first utterance must render exactly one user bubble"
    );
    assert_eq!(state.messages[0].role, ChatRole::User);
    assert_eq!(state.messages[0].text, text);
    assert!(!state.messages[0].is_streaming);
}

#[test]
#[serial]
fn finalize_user_message_does_not_merge_across_assistant_boundary() {
    // Guard against over-reach: reuse must never reach back past a non-user
    // message. A prior user turn already answered by the assistant is a closed
    // turn; a fresh utterance with identical text is a new bubble, not a merge.
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: "Repeat that".to_string(),
            is_streaming: false,
            is_collapsed: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some("AI".to_string()),
            is_pending_followup: false,
        });
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: "Sure, here it is.".to_string(),
            is_streaming: false,
            is_collapsed: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some("AI".to_string()),
            is_pending_followup: false,
        });
    }

    finalize_user_message_impl("Repeat that");

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        state.messages.len(),
        3,
        "identical text behind an assistant reply must push a new user bubble"
    );
    assert_eq!(state.messages[2].role, ChatRole::User);
    assert_eq!(state.messages[2].text, "Repeat that");
}

#[test]
#[serial]
fn assistive_followup_edit_moves_pending_text_to_draft_without_send() {
    let call_count = Arc::new(AtomicUsize::new(0));
    {
        let count = Arc::clone(&call_count);
        let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        *cb = Some(Arc::new(move |_text: String| {
            count.fetch_add(1, Ordering::SeqCst);
        }));
    }
    {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        *state = VoiceChatOverlayState::default();
        state.is_agent_thinking = true;
    }

    append_voice_chat_user_delta_impl("Needs correction");
    edit_pending_followup_message_impl();

    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(call_count.load(Ordering::SeqCst), 0);
    assert!(state.messages.is_empty());
    assert_eq!(state.manual_draft, "Needs correction");

    let mut cb = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *cb = None;
}

// ── Grouped Tool Activity ────────────────────────────────────────────────
// State-level accumulation + summary rendering. Exercises the pure grouping
// helpers (`ensure_tool_activity_block` / `refresh_tool_activity_message` /
// `close_tool_activity_turn`) directly on a windowless state, so no AppKit
// view build runs — the assertion is purely on the message model.

fn push_chat_message(
    state: &mut VoiceChatOverlayState,
    role: ChatRole,
    text: &str,
    streaming: bool,
) {
    state.messages.push(ChatMessage {
        role,
        text: text.to_string(),
        is_streaming: streaming,
        is_collapsed: false,
        is_error: false,
        timestamp: SystemTime::now(),
        mode: Some("AI".to_string()),
        is_pending_followup: false,
    });
}

fn feed_tool_running(state: &mut VoiceChatOverlayState, id: &str, raw: &str, display: &str) {
    let idx = ensure_tool_activity_block(state);
    state
        .tool_activity_groups
        .get_mut(&idx)
        .expect("group exists for the open block")
        .mark_running(id, raw, display);
    refresh_tool_activity_message(state, idx);
}

fn feed_tool_result(
    state: &mut VoiceChatOverlayState,
    id: &str,
    raw: &str,
    display: &str,
    summary: &str,
    is_error: bool,
) {
    let idx = ensure_tool_activity_block(state);
    state
        .tool_activity_groups
        .get_mut(&idx)
        .expect("group exists for the open block")
        .mark_result(id, raw, display, summary, is_error);
    refresh_tool_activity_message(state, idx);
}

#[test]
fn turn_with_three_tools_renders_one_grouped_block() {
    // Operator regression sequence. Assistant text chunks (1 & 5) and tool
    // events (2,3,4,6,7,8) interleave on the wire; the model must produce ONE
    // assistant message and ONE tool-activity block — never per-tool cards.
    let mut state = VoiceChatOverlayState::default();
    push_chat_message(&mut state, ChatRole::User, "Co tam?", false);
    // 1. assistant answer starts (single streaming bubble for the whole turn)
    push_chat_message(
        &mut state,
        ChatRole::Assistant,
        "Sprawdzam… Wynik jest taki…",
        true,
    );

    // 2-3. brave search start + result
    feed_tool_running(
        &mut state,
        "c1",
        "mcp__brave-search__brave_web_search",
        "Web search",
    );
    feed_tool_result(
        &mut state,
        "c1",
        "mcp__brave-search__brave_web_search",
        "Web search",
        "10 results",
        false,
    );
    // 4. loctree start (result arrives later, after more assistant text)
    feed_tool_running(
        &mut state,
        "c2",
        "mcp__loctree-mcp__context",
        "Loctree context",
    );
    // 7. aicx start
    feed_tool_running(
        &mut state,
        "c3",
        "mcp__aicx-mcp__aicx_intents",
        "AICX intents",
    );
    // 6. loctree result
    feed_tool_result(
        &mut state,
        "c2",
        "mcp__loctree-mcp__context",
        "Loctree context",
        "",
        false,
    );
    // 8. aicx failed
    feed_tool_result(
        &mut state,
        "c3",
        "mcp__aicx-mcp__aicx_intents",
        "AICX intents",
        "empty index",
        true,
    );

    // Exactly one tool-activity block for the turn.
    let blocks: Vec<&ChatMessage> = state
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::ToolActivity)
        .collect();
    assert_eq!(blocks.len(), 1, "all turn tools collapse into one block");
    // Default primary view is the semantic Evidence Summary: what was checked,
    // which sources, the key result, with failures surfaced as warnings.
    assert_eq!(
        blocks[0].text,
        "What I checked · 3 tools · 1 warning\n\
         - Web search: 10 results.\n\
         - Loctree: scanned code surfaces.\n\
         - AICX: failed — empty index.\n\
         Key finding: AICX check failed: empty index."
    );

    // Expanding the block (a click flips `is_collapsed`) reveals the technical
    // per-tool list as the layer-2 detail view.
    let block_idx = state
        .messages
        .iter()
        .position(|m| m.role == ChatRole::ToolActivity)
        .expect("one tool-activity block");
    state.messages[block_idx].is_collapsed = true;
    refresh_tool_activity_message(&mut state, block_idx);
    assert_eq!(
        state.messages[block_idx].text,
        "Tool activity · 3 calls · 1 failed\n\
         - Web search · completed · 10 results\n\
         - Loctree context · completed\n\
         - AICX intents · failed · empty index"
    );

    // The assistant answer stays a single, uninterrupted message.
    let assistant_count = state
        .messages
        .iter()
        .filter(|m| m.role == ChatRole::Assistant)
        .count();
    assert_eq!(assistant_count, 1, "answer is not split by tool cards");

    // No raw MCP wire name leaks into the primary timeline.
    assert!(
        state.messages.iter().all(|m| !m.text.contains("mcp__")),
        "raw MCP names must never reach the timeline"
    );
}

#[test]
fn block_is_reused_within_a_turn_and_reopened_next_turn() {
    let mut state = VoiceChatOverlayState::default();

    feed_tool_running(
        &mut state,
        "a",
        "mcp__loctree-mcp__find",
        "Loctree occurrences/find",
    );
    let first_idx = state
        .active_tool_activity_index
        .expect("first turn opened a block");
    // Same turn: a second tool reuses the same block index.
    feed_tool_running(&mut state, "b", "read_clipboard", "Clipboard read");
    assert_eq!(state.active_tool_activity_index, Some(first_idx));
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|m| m.role == ChatRole::ToolActivity)
            .count(),
        1
    );

    // Turn boundary: assistant finalized → pointer closes.
    close_tool_activity_turn(&mut state);
    assert_eq!(state.active_tool_activity_index, None);

    // Next turn's first tool opens a brand-new block.
    feed_tool_running(&mut state, "c", "take_screenshot", "Screenshot");
    assert_ne!(state.active_tool_activity_index, Some(first_idx));
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|m| m.role == ChatRole::ToolActivity)
            .count(),
        2,
        "each turn renders its own block"
    );
}

#[test]
fn toggle_switches_between_evidence_summary_and_technical_list() {
    let mut state = VoiceChatOverlayState::default();
    feed_tool_result(
        &mut state,
        "a",
        "mcp__loctree-mcp__context",
        "Loctree context",
        "",
        false,
    );
    let idx = state.active_tool_activity_index.expect("block open");

    // Default: the Evidence Summary is the primary view.
    assert_eq!(
        state.messages[idx].text,
        "What I checked · 1 tool\n- Loctree: scanned code surfaces."
    );

    // Click → technical per-tool list.
    state.messages[idx].is_collapsed = true;
    refresh_tool_activity_message(&mut state, idx);
    assert_eq!(
        state.messages[idx].text,
        "Tool activity · 1 call completed\n- Loctree context · completed"
    );

    // Click again → back to the Evidence Summary.
    state.messages[idx].is_collapsed = false;
    refresh_tool_activity_message(&mut state, idx);
    assert_eq!(
        state.messages[idx].text,
        "What I checked · 1 tool\n- Loctree: scanned code surfaces."
    );
}
