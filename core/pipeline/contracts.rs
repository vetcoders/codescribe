//! Pipeline contracts — shared data types for the transcription pipeline.
//!
//! These types define the boundaries between pipeline stages:
//!   AudioChunk → SpeechUtterance → RawTranscript → PostprocessResult → TranscriptDelta → DeltaSink
//!
//! Vibecrafted with AI Agents by VetCoders (c)2026 VetCoders

use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════
// Audio stage
// ═══════════════════════════════════════════════════════════

/// A chunk of raw audio samples from the recorder.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    /// Start time relative to recording session (seconds).
    pub start_ts: f32,
    /// End time relative to recording session (seconds).
    pub end_ts: f32,
}

/// A complete speech utterance (after VAD gating / silence detection).
#[derive(Debug, Clone)]
pub struct SpeechUtterance {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub start_ts: f32,
    pub end_ts: f32,
}

impl SpeechUtterance {
    /// Duration in seconds.
    pub fn duration(&self) -> f32 {
        self.end_ts - self.start_ts
    }
}

// ═══════════════════════════════════════════════════════════
// STT stage
// ═══════════════════════════════════════════════════════════

/// Raw output from a speech-to-text engine (Whisper or future providers).
#[derive(Debug, Clone, Default)]
pub struct RawTranscript {
    /// The transcribed text (untouched by postprocessing).
    pub text: String,
    /// Per-segment breakdown, if the engine provides it.
    pub segments: Vec<TranscriptSegment>,
}

/// A single segment from the STT engine (optional granularity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub text: String,
    pub start_ts: f32,
    pub end_ts: f32,
}

// ═══════════════════════════════════════════════════════════
// Postprocess stage
// ═══════════════════════════════════════════════════════════

/// Result of postprocessing a raw transcript (lexicon + semantic gate + cleanup).
#[derive(Debug, Clone)]
pub struct PostprocessResult {
    /// Cleaned text ready for user-facing output.
    pub text: String,
    /// Whether the semantic gate dropped this chunk (text will be empty).
    pub dropped: bool,
}

// ═══════════════════════════════════════════════════════════
// Delta / presentation stage
// ═══════════════════════════════════════════════════════════

/// Backspace character used in delta encoding.
pub const BACKSPACE: char = '\u{0008}';

/// An incremental update to the transcript buffer.
///
/// # Hard contract (append + backspace only)
///
/// - The payload is a UTF-8 char stream interpreted left-to-right.
/// - Any non-`\u{0008}` char means append that char to the tail.
/// - `\u{0008}` means remove one char from the current tail (if any).
/// - Backspace underflow is a no-op (never panic, never index by byte).
/// - Producers must emit deltas in-order; consumers must apply in the same order.
///
/// This contract keeps live correction cheap and deterministic without resending
/// full buffers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptDelta {
    pub delta: String,
}

impl TranscriptDelta {
    /// Wrap a raw delta string (may contain backspace chars) without modification.
    pub fn from_raw(delta: impl Into<String>) -> Self {
        Self {
            delta: delta.into(),
        }
    }

    /// Create a simple append-only delta (no backspaces).
    pub fn append(text: impl Into<String>) -> Self {
        Self { delta: text.into() }
    }

    /// Create a delta that replaces the last `delete_count` characters, then appends `new_text`.
    pub fn replace(delete_count: usize, new_text: &str) -> Self {
        let mut delta = String::with_capacity(delete_count + new_text.len());
        for _ in 0..delete_count {
            delta.push(BACKSPACE);
        }
        delta.push_str(new_text);
        Self { delta }
    }

    /// Build a minimal delta from "before" and "after" full texts.
    ///
    /// Finds the common prefix, emits backspaces for the removed tail of `before`,
    /// then appends the new tail of `after`. Returns `None` if texts are identical.
    ///
    /// Output always follows the hard contract: only append chars + backspaces.
    pub fn from_diff(before: &str, after: &str) -> Option<Self> {
        if before == after {
            return None;
        }

        let common_prefix_len = before
            .chars()
            .zip(after.chars())
            .take_while(|(a, b)| a == b)
            .count();

        let delete_count = before.chars().count() - common_prefix_len;
        let new_tail: String = after.chars().skip(common_prefix_len).collect();

        Some(Self::replace(delete_count, &new_tail))
    }

    /// Apply this delta to a mutable string buffer (the inverse of `from_diff`).
    ///
    /// Backspace underflow is intentionally ignored, keeping this operation
    /// idempotent and safe for partial buffers.
    pub fn apply(&self, target: &mut String) {
        for ch in self.delta.chars() {
            if ch == BACKSPACE {
                target.pop();
            } else {
                target.push(ch);
            }
        }
    }

    /// Returns `true` if this delta contains only backspace characters (pure deletion).
    pub fn is_delete_only(&self) -> bool {
        !self.delta.is_empty() && self.delta.chars().all(|c| c == BACKSPACE)
    }

    /// Returns `true` if this delta contains no backspaces (pure append).
    pub fn is_append_only(&self) -> bool {
        !self.delta.is_empty() && self.delta.chars().all(|c| c != BACKSPACE)
    }
}

impl std::fmt::Display for TranscriptDelta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.delta)
    }
}

// ═══════════════════════════════════════════════════════════
// Traits (adapter boundaries)
// ═══════════════════════════════════════════════════════════

/// Adapter for speech-to-text engines.
///
/// Implementations: `LocalWhisperEngine` (current), future cloud STT providers.
pub trait TranscriptionAdapter: Send + Sync {
    fn transcribe(
        &self,
        utterance: &SpeechUtterance,
        language: Option<&str>,
    ) -> anyhow::Result<RawTranscript>;
}

/// Post-processor for raw transcripts.
///
/// Implementations: `StreamPostProcessor` (semantic gate), `LexiconPostProcessor`.
pub trait PostProcessor: Send {
    fn process(&mut self, raw: &RawTranscript) -> Option<PostprocessResult>;
}

/// Sink for transcript deltas (UI, IPC, clipboard, etc).
///
/// This decouples the streaming pipeline from presentation concerns.
pub trait DeltaSink: Send + Sync {
    fn apply(&self, delta: &TranscriptDelta);
}

// ═══════════════════════════════════════════════════════════
// Engine events (intent layer)
// ═══════════════════════════════════════════════════════════

/// Events emitted by the transcription engine.
///
/// These are semantic events — the engine communicates what happened
/// and why, not how to display it. UI decides presentation.
///
/// Data flow: AudioChunk → VAD → Whisper → PostProcess → EngineEvent
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// VAD detected speech start.
    VadStart { speech_prob: f32, ts_ms: u64 },
    /// VAD detected speech end.
    VadEnd { speech_prob: f32, ts_ms: u64 },
    /// Session or utterance completed without usable speech content.
    ///
    /// Emitted when VAD sees no speech at all, or when speech-like segments are
    /// fully rejected by quality gates/hallucination filters.
    NoSpeech { reason: String },

    /// Interim preview — latest transcription of the current utterance.
    ///
    /// # Semantics (contract)
    ///
    /// - `text` is **utterance-local**: it contains the full post-processed text for
    ///   the current utterance only, NOT the accumulated session text.
    /// - On each new Whisper decode, `text` replaces the previous Preview for this
    ///   utterance (not appended). `rev` increments monotonically.
    /// - After `UtteranceFinal`, `text` resets to empty for the next utterance.
    ///
    /// # Sink responsibilities
    ///
    /// - Sinks that need incremental deltas (e.g. overlay append) must track
    ///   `last_preview` and compute diffs themselves (see `TranscriptDelta::from_diff`).
    /// - Sinks that need session-accumulated text must concatenate across utterances.
    /// - On `UtteranceFinal`, sinks must reset their `last_preview` state.
    Preview { rev: u64, text: String },

    /// Correction — re-transcription of accumulated audio improved previous output.
    ///
    /// # Semantics (contract)
    ///
    /// - `text` is the full corrected utterance-local text (replaces, not appends).
    /// - `previous_text` is what was shown before correction and acts as a baseline
    ///   for stale-correction guards in sinks.
    /// - Sinks should apply this as a replacement (delta diff or full overwrite)
    ///   and update their `last_preview` to `text`.
    /// - Must NOT finalize streaming state (keep `is_streaming = true` in UI).
    Correction {
        rev: u64,
        text: String,
        previous_text: String,
    },

    /// Complete utterance (VAD-bounded or flush).
    ///
    /// # Semantics (contract)
    ///
    /// - Emitted once per VAD-bounded speech segment (or on session flush).
    /// - `text` is the final post-processed utterance text.
    /// - After this event, the engine clears its internal accumulated_text.
    /// - Sinks must reset `last_preview` to empty (next Preview starts fresh).
    /// - In toggle mode, the utterance callback processes this text (AI/clipboard).
    ///   The commit path should NOT re-write to the user bubble if Preview already
    ///   streamed into it (see `skip_user_bubble`).
    UtteranceFinal {
        utterance_id: u64,
        text: String,
        raw_text: String,
        start_ts: f32,
        end_ts: f32,
        segments: Vec<TranscriptSegment>,
    },

    /// Content dropped by engine intelligence.
    Drop {
        kind: DropKind,
        text: String,
        reason: String,
    },

    /// Session-level statistics (emitted on stop/flush).
    Stats {
        dropped_audio_chunks: u64,
        hallucination_drops: u64,
        semantic_gate_drops: u64,
        filtered_empty_drops: u64,
        corrections_applied: u64,
        total_utterances: u64,
        /// Number of partial-pass refine runs attempted in this session.
        partial_runs_total: u64,
        /// Partial-pass runs triggered by utterance-count threshold.
        trigger_utterance_count: u64,
        /// Partial-pass runs triggered by speech-duration threshold.
        trigger_speech_count: u64,
        /// Partial-pass runs triggered by watchdog fallback.
        trigger_watchdog_count: u64,
        /// Refinement results suppressed by stale-guard checks.
        partial_stale_count: u64,
        /// Tracked refine runs superseded by newer partial-pass requests.
        partial_coalesced_count: u64,
        /// Partial-pass runs dropped (submit/queue/shutdown paths).
        partial_dropped_count: u64,
    },

    /// Recoverable error — engine continues.
    Warning { code: String, message: String },
}

/// Why the engine dropped content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropKind {
    /// Whisper hallucination pattern detected (e.g. "thank you", "subscribe").
    Hallucination,
    /// Semantic gate: chunk too similar to previous output (streaming path only).
    SemanticGate,
    /// Overlap dedup produced empty result.
    OverlapEmpty,
    /// Text was empty after lexicon + cleanup processing (utterance path).
    /// Distinct from `SemanticGate` — no embedding comparison was involved.
    FilteredEmpty,
}

impl std::fmt::Display for DropKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DropKind::Hallucination => write!(f, "Hallucination"),
            DropKind::SemanticGate => write!(f, "SemanticGate"),
            DropKind::OverlapEmpty => write!(f, "OverlapEmpty"),
            DropKind::FilteredEmpty => write!(f, "FilteredEmpty"),
        }
    }
}

/// Sink for engine events. Replaces DeltaSink for the unified pipeline.
///
/// Implementations decide how to present events — typing animation,
/// overlay updates, clipboard paste, IPC streaming, etc.
pub trait EventSink: Send + Sync {
    fn on_event(&self, event: &EngineEvent);
}

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── TranscriptDelta roundtrip ──

    #[test]
    fn delta_append_roundtrip() {
        let mut buf = String::from("Hello");
        let delta = TranscriptDelta::append(" world");
        delta.apply(&mut buf);
        assert_eq!(buf, "Hello world");
    }

    #[test]
    fn delta_replace_roundtrip() {
        let mut buf = String::from("Hello worl");
        // Fix typo: delete "worl" (4 chars), append "world!"
        let delta = TranscriptDelta::replace(4, "world!");
        delta.apply(&mut buf);
        assert_eq!(buf, "Hello world!");
    }

    #[test]
    fn delta_from_diff_roundtrip() {
        let before = "Cześć, jestem lekarzem";
        let after = "Cześć, jestem weterynarzem";

        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    #[test]
    fn delta_from_diff_identical_returns_none() {
        assert!(TranscriptDelta::from_diff("same", "same").is_none());
    }

    #[test]
    fn delta_from_diff_complete_replacement() {
        let before = "abc";
        let after = "xyz";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    #[test]
    fn delta_from_diff_empty_to_text() {
        let delta = TranscriptDelta::from_diff("", "hello").unwrap();
        let mut buf = String::new();
        delta.apply(&mut buf);
        assert_eq!(buf, "hello");
        assert!(delta.is_append_only());
    }

    #[test]
    fn delta_from_diff_text_to_empty() {
        let delta = TranscriptDelta::from_diff("hello", "").unwrap();
        let mut buf = String::from("hello");
        delta.apply(&mut buf);
        assert_eq!(buf, "");
        assert!(delta.is_delete_only());
    }

    #[test]
    fn delta_apply_backspace_underflow_is_noop() {
        let mut buf = String::new();
        TranscriptDelta::from_raw("\u{0008}\u{0008}abc").apply(&mut buf);
        assert_eq!(buf, "abc");
    }

    #[test]
    fn delta_from_diff_unicode_polish() {
        let before = "Zółty pies";
        let after = "Żółty pies";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    #[test]
    fn delta_backspace_sequence() {
        // Simulate Whisper correcting "transkryp" → "transkrypcja"
        let mut buf = String::from("transkryp");
        let delta = TranscriptDelta::append("cja");
        delta.apply(&mut buf);
        assert_eq!(buf, "transkrypcja");
    }

    #[test]
    fn delta_multi_step_simulation() {
        // Simulate streaming: chunk1 → chunk2 (with correction) → chunk3
        let mut buf = String::new();

        // Chunk 1: "Witaj "
        TranscriptDelta::append("Witaj ").apply(&mut buf);
        assert_eq!(buf, "Witaj ");

        // Chunk 2: Whisper corrects to "Witaj, " (backspace space, add ", ")
        TranscriptDelta::replace(1, ", ").apply(&mut buf);
        assert_eq!(buf, "Witaj, ");

        // Chunk 3: append "świecie!"
        TranscriptDelta::append("świecie!").apply(&mut buf);
        assert_eq!(buf, "Witaj, świecie!");
    }

    #[test]
    fn delta_from_diff_multibyte_cjk() {
        let before = "日本語テスト";
        let after = "日本語テスト結果";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
        assert!(delta.is_append_only());
    }

    #[test]
    fn delta_from_diff_emoji_replacement() {
        let before = "Hello 🌍";
        let after = "Hello 🌎";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    #[test]
    fn delta_from_diff_mixed_replace_contains_backspace_and_append() {
        let before = "alpha beta";
        let after = "alpha gamma";
        let delta = TranscriptDelta::from_diff(before, after).unwrap();
        assert!(delta.delta.contains(BACKSPACE));
        assert!(delta.delta.ends_with("gamma"));

        let mut buf = before.to_string();
        delta.apply(&mut buf);
        assert_eq!(buf, after);
    }

    // ── SpeechUtterance ──

    #[test]
    fn utterance_duration() {
        let u = SpeechUtterance {
            samples: vec![0.0; 16000],
            sample_rate: 16000,
            start_ts: 1.5,
            end_ts: 2.5,
        };
        assert!((u.duration() - 1.0).abs() < f32::EPSILON);
    }

    // ── RawTranscript ──

    #[test]
    fn raw_transcript_default_has_no_segments() {
        let rt = RawTranscript::default();
        assert!(rt.text.is_empty());
        assert!(rt.segments.is_empty());
    }

    // ── PostprocessResult ──

    #[test]
    fn postprocess_result_dropped() {
        let r = PostprocessResult {
            text: String::new(),
            dropped: true,
        };
        assert!(r.dropped);
        assert!(r.text.is_empty());
    }

    // ── EngineEvent ──

    #[test]
    fn engine_event_preview_clone() {
        let event = EngineEvent::Preview {
            rev: 1,
            text: "Hello world".to_string(),
        };
        let cloned = event.clone();
        if let EngineEvent::Preview { rev, text } = cloned {
            assert_eq!(rev, 1);
            assert_eq!(text, "Hello world");
        } else {
            panic!("Expected Preview variant");
        }
    }

    #[test]
    fn engine_event_no_speech_clone() {
        let event = EngineEvent::NoSpeech {
            reason: "vad_no_speech_detected".to_string(),
        };
        let cloned = event.clone();
        if let EngineEvent::NoSpeech { reason } = cloned {
            assert_eq!(reason, "vad_no_speech_detected");
        } else {
            panic!("Expected NoSpeech variant");
        }
    }

    #[test]
    fn engine_event_drop_kind_display() {
        assert_eq!(DropKind::Hallucination.to_string(), "Hallucination");
        assert_eq!(DropKind::SemanticGate.to_string(), "SemanticGate");
        assert_eq!(DropKind::OverlapEmpty.to_string(), "OverlapEmpty");
        assert_eq!(DropKind::FilteredEmpty.to_string(), "FilteredEmpty");
    }

    #[test]
    fn engine_event_stats_fields() {
        let event = EngineEvent::Stats {
            dropped_audio_chunks: 2,
            hallucination_drops: 3,
            semantic_gate_drops: 1,
            filtered_empty_drops: 0,
            corrections_applied: 4,
            total_utterances: 10,
            partial_runs_total: 7,
            trigger_utterance_count: 4,
            trigger_speech_count: 2,
            trigger_watchdog_count: 1,
            partial_stale_count: 3,
            partial_coalesced_count: 2,
            partial_dropped_count: 1,
        };
        if let EngineEvent::Stats {
            total_utterances,
            hallucination_drops,
            partial_runs_total,
            trigger_watchdog_count,
            ..
        } = event
        {
            assert_eq!(total_utterances, 10);
            assert_eq!(hallucination_drops, 3);
            assert_eq!(partial_runs_total, 7);
            assert_eq!(trigger_watchdog_count, 1);
        } else {
            panic!("Expected Stats variant");
        }
    }

    #[test]
    fn engine_event_utterance_final_roundtrip() {
        let event = EngineEvent::UtteranceFinal {
            utterance_id: 42,
            text: "cleaned text".to_string(),
            raw_text: "raw text from whisper".to_string(),
            start_ts: 1.5,
            end_ts: 3.2,
            segments: vec![TranscriptSegment {
                text: "cleaned".to_string(),
                start_ts: 1.5,
                end_ts: 3.2,
            }],
        };
        if let EngineEvent::UtteranceFinal {
            utterance_id,
            text,
            raw_text,
            start_ts,
            end_ts,
            segments,
        } = event
        {
            assert_eq!(utterance_id, 42);
            assert_eq!(text, "cleaned text");
            assert_eq!(raw_text, "raw text from whisper");
            assert!((start_ts - 1.5).abs() < f32::EPSILON);
            assert!((end_ts - 3.2).abs() < f32::EPSILON);
            assert_eq!(segments.len(), 1);
        } else {
            panic!("Expected UtteranceFinal variant");
        }
    }
}
