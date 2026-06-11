//! Transcription preview timing model and preview panel refresh.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PreviewTimingModel {
    pub(super) overlay_enabled: bool,
    pub(super) buffer_delay_ms: u64,
    pub(super) typing_cps: f32,
    pub(super) emit_words_max: usize,
    pub(super) requested_interim_sec: f32,
    pub(super) effective_interim_sec: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreviewTimingStep {
    publish_ms: u64,
    pub(super) visible_at_ms: u64,
    pub(super) target_text: String,
    pub(super) visible_text: String,
}

pub(super) fn preview_effective_interim_sec(
    overlay_enabled: bool,
    requested_interim_sec: f32,
) -> f32 {
    let requested = requested_interim_sec.clamp(1.0, 30.0);
    if overlay_enabled {
        requested
    } else {
        requested.max(PREVIEW_NO_OVERLAY_MIN_INTERIM_SEC)
    }
}

pub(super) fn current_preview_timing_model() -> PreviewTimingModel {
    let config = Config::load();
    let settings = UserSettings::load();
    let requested_interim_sec = settings.buffered_interim_sec.unwrap_or(1.2);

    PreviewTimingModel {
        overlay_enabled: config.transcription_overlay_enabled,
        buffer_delay_ms: settings.buffer_delay_ms.unwrap_or(280),
        typing_cps: settings.typing_cps.unwrap_or(90.0).max(5.0),
        emit_words_max: settings.emit_words_max.unwrap_or(2).clamp(1, 10) as usize,
        requested_interim_sec,
        effective_interim_sec: preview_effective_interim_sec(
            config.transcription_overlay_enabled,
            requested_interim_sec,
        ),
    }
}

pub(super) fn preview_tokenize_for_emit(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut cursor = 0usize;

    while cursor < chars.len() {
        let mut token = String::new();
        if chars[cursor].is_whitespace() {
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                token.push(chars[cursor]);
                cursor += 1;
            }
        } else {
            while cursor < chars.len() && !chars[cursor].is_whitespace() {
                token.push(chars[cursor]);
                cursor += 1;
            }
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                token.push(chars[cursor]);
                cursor += 1;
            }
        }
        tokens.push(token);
    }
    if tokens.is_empty() && !text.is_empty() {
        tokens.push(text.to_string());
    }
    tokens
}

pub(super) fn preview_emit_chunks(text: &str, emit_words_max: usize) -> Vec<String> {
    let tokens = preview_tokenize_for_emit(text);
    let mut chunks = Vec::new();
    let mut current_index = 0usize;

    while current_index < tokens.len() {
        let mut chunk = String::new();
        let mut words = 0usize;
        while current_index < tokens.len() {
            let token = &tokens[current_index];
            chunk.push_str(token);
            if token.chars().any(|c| !c.is_whitespace()) {
                words += 1;
            }
            current_index += 1;

            if words >= emit_words_max {
                if current_index < tokens.len() {
                    let next = &tokens[current_index];
                    if next.chars().all(|c| c.is_whitespace()) {
                        chunk.push_str(next);
                        current_index += 1;
                    }
                }
                break;
            }
        }

        if !chunk.is_empty() {
            chunks.push(chunk);
        }
    }

    chunks
}

pub(super) fn preview_partial_targets(sample: &str, interim_sec: f32) -> Vec<(u64, String)> {
    let words: Vec<&str> = sample.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let total_duration = PREVIEW_SAMPLE_UTTERANCE_SEC.max(interim_sec);
    let total_words = words.len();
    let mut targets = Vec::new();
    let mut reveal_cursor = 0usize;
    let mut t = interim_sec;

    while t < total_duration {
        let progress = (t / total_duration).clamp(0.0, 1.0);
        let reveal_words = ((total_words as f32 * progress).ceil() as usize)
            .clamp(1, total_words)
            .max((reveal_cursor + 1).min(total_words));
        if reveal_words > reveal_cursor {
            reveal_cursor = reveal_words;
            targets.push((
                (t * 1000.0).round() as u64,
                words[..reveal_cursor].join(" "),
            ));
        }
        t += interim_sec;
    }

    if reveal_cursor < total_words {
        targets.push((
            (total_duration * 1000.0).round() as u64,
            words[..total_words].join(" "),
        ));
    }

    targets
}

pub(super) fn preview_timing_steps(model: PreviewTimingModel) -> Vec<PreviewTimingStep> {
    let partial_targets = preview_partial_targets(PREVIEW_SAMPLE_TEXT, model.effective_interim_sec);
    let tick_ms = ((1000.0 / model.typing_cps as f64).round() as u64).max(1);
    let mut visible_text = String::new();
    let mut previous_target = String::new();
    let mut last_emit_done_ms = 0u64;
    let mut steps = Vec::new();

    for (index, (publish_ms, target_text)) in partial_targets.into_iter().enumerate() {
        let suffix = if target_text.starts_with(&previous_target) {
            target_text[previous_target.len()..].to_string()
        } else {
            target_text.clone()
        };

        if suffix.is_empty() {
            previous_target = target_text;
            continue;
        }

        let start_ms = if index == 0 {
            publish_ms
        } else {
            publish_ms
                .saturating_add(model.buffer_delay_ms)
                .max(last_emit_done_ms)
        };

        let mut current_ms = start_ms;
        for chunk in preview_emit_chunks(&suffix, model.emit_words_max) {
            visible_text.push_str(&chunk);
            steps.push(PreviewTimingStep {
                publish_ms,
                visible_at_ms: current_ms,
                target_text: target_text.clone(),
                visible_text: visible_text.clone(),
            });
            current_ms = current_ms.saturating_add(tick_ms);
        }

        last_emit_done_ms = current_ms;
        previous_target = target_text;
    }

    steps
}

pub(super) fn preview_timing_summary_text(model: PreviewTimingModel) -> String {
    let tick_ms = ((1000.0 / model.typing_cps as f64).round() as u64).max(1);
    if model.overlay_enabled {
        format!(
            "Overlay ON • partial target every {:.1}s • first output immediate • later growth +{}ms • {} words/tick • {}ms between ticks",
            model.effective_interim_sec, model.buffer_delay_ms, model.emit_words_max, tick_ms
        )
    } else {
        format!(
            "Overlay OFF • runtime hides floating preview • cadence clamped to {:.1}s (requested {:.1}s) • if shown: +{}ms buffer • {} words/tick • {}ms ticks",
            model.effective_interim_sec,
            model.requested_interim_sec,
            model.buffer_delay_ms,
            model.emit_words_max,
            tick_ms
        )
    }
}

pub(super) fn preview_timing_report_text(model: PreviewTimingModel) -> String {
    let partial_targets = preview_partial_targets(PREVIEW_SAMPLE_TEXT, model.effective_interim_sec);
    let steps = preview_timing_steps(model);
    let mut lines = Vec::new();
    lines.push(format!("Sample: {PREVIEW_SAMPLE_TEXT}"));
    lines.push(String::new());
    lines.push("Chunker partial targets".to_string());
    for (publish_ms, target_text) in partial_targets {
        lines.push(format!(
            "[{:.1}s] {}",
            publish_ms as f32 / 1000.0,
            target_text
        ));
    }
    lines.push(String::new());
    lines.push(if model.overlay_enabled {
        "Overlay-visible text".to_string()
    } else {
        "Overlay-visible text (would look like this if overlay was enabled)".to_string()
    });
    for step in steps {
        lines.push(format!(
            "[publish {:.1}s -> visible {:.2}s] {}",
            step.publish_ms as f32 / 1000.0,
            step.visible_at_ms as f32 / 1000.0,
            step.visible_text
        ));
    }
    lines.join("\n")
}

pub(super) fn refresh_transcription_preview_panel() {
    let model = current_preview_timing_model();
    let preview_text = preview_timing_report_text(model);
    let summary_text = preview_timing_summary_text(model);
    let (
        buffer_delay_label,
        typing_cps_label,
        emit_words_label,
        interim_label,
        summary_label,
        preview_text_view,
    ) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.preview_buffer_delay_value_label,
            state.preview_typing_cps_value_label,
            state.preview_emit_words_max_value_label,
            state.preview_interim_sec_value_label,
            state.preview_timing_summary_label,
            state.preview_timing_text_view,
        )
    };

    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        if let Some(ptr) = buffer_delay_label {
            set_text_field_string(ptr as Id, &format!("{} ms", model.buffer_delay_ms));
        }
        if let Some(ptr) = typing_cps_label {
            set_text_field_string(ptr as Id, &format!("{:.1} cps", model.typing_cps));
        }
        if let Some(ptr) = emit_words_label {
            set_text_field_string(ptr as Id, &format!("{} words", model.emit_words_max));
        }
        if let Some(ptr) = interim_label {
            let label = if (model.effective_interim_sec - model.requested_interim_sec).abs()
                > f32::EPSILON
            {
                format!(
                    "{:.1} s -> {:.1} s effective",
                    model.requested_interim_sec, model.effective_interim_sec
                )
            } else {
                format!("{:.1} s", model.effective_interim_sec)
            };
            set_text_field_string(ptr as Id, &label);
        }
        if let Some(ptr) = summary_label {
            set_text_field_string(ptr as Id, &summary_text);
        }
        if let Some(ptr) = preview_text_view {
            set_text_view_string(ptr as Id, &preview_text);
        }
    }
}
