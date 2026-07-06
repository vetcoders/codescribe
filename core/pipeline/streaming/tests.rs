use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex as StdMutex};

use anyhow::Result;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, Instant};

use crate::pipeline::contracts::{EngineEvent, EventSink, RawTranscript, TranscriptSegment};
use crate::pipeline::sinks::CollectorEventSink;
use crate::stt::scheduler::{SttLane, SttScheduler, SttTaskHandle};
use crate::vad;

use super::correction::*;
use super::emitter::*;
use super::pipeline::*;
use super::quality_gate::*;
use super::session::*;

fn pending_item(is_final: bool) -> PendingUtteranceWorkItem {
    pending_item_with_marker(is_final, if is_final { 1.0 } else { 0.1 })
}

fn pending_item_with_marker(is_final: bool, marker: f32) -> PendingUtteranceWorkItem {
    PendingUtteranceWorkItem {
        audio: vec![marker; 32],
        inference_audio: vec![marker; 32],
        is_final,
        scheduler_utterance_id: if is_final { 1 } else { 0 },
        max_speech_prob: 0.9,
        speech_vad_samples: 512,
    }
}

#[test]
fn test_postprocess_components() {
    // Hallucination
    assert!(is_hallucination("Thank you", None));
    assert!(is_hallucination("  Dziękuję za uwagę  ", Some("pl")));
    assert!(is_hallucination(
        "Napisy stworzone przez społeczność",
        Some("pl")
    ));
    assert!(!is_hallucination("Tak", Some("pl"))); // Whitelisted
    assert!(!is_hallucination("This is a normal sentence.", Some("en")));

    // Overlap
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "Alice has a cat.".to_string();

    let res = pipeline.strip_overlap("Alice has a cat. And a dog.");
    assert_eq!(res, "And a dog.");

    pipeline.last_suffix = "going to the park".to_string();
    let res = pipeline.strip_overlap("park tomorrow.");
    assert_eq!(res, "tomorrow.");

    let res = pipeline.strip_overlap("Hello world");
    assert_eq!(res, "Hello world");
}

#[test]
fn test_silence_chunk_gate() {
    // 1s at 48kHz = 48000 samples. 16kHz equivalent = 16000 samples.
    let one_sec_48k = 48000usize;

    // Chunk with 0% speech → drop
    assert!(should_drop_silence_chunk(one_sec_48k, 48000, 0, false));

    // Chunk with ~5% speech (800 out of 16000) → drop
    assert!(should_drop_silence_chunk(one_sec_48k, 48000, 800, false));

    // Chunk with ~20% speech (3200 out of 16000) → keep
    assert!(!should_drop_silence_chunk(one_sec_48k, 48000, 3200, false));

    // Chunk with 100% speech → keep
    assert!(!should_drop_silence_chunk(one_sec_48k, 48000, 16000, false));

    // Final emission always passes (user released key)
    assert!(!should_drop_silence_chunk(one_sec_48k, 48000, 0, true));

    // 16kHz input: same domain
    assert!(should_drop_silence_chunk(16000, 16000, 0, false));
    assert!(!should_drop_silence_chunk(16000, 16000, 3200, false));

    // Zero-length audio → never drop (edge case)
    assert!(!should_drop_silence_chunk(0, 48000, 0, false));
}

#[test]
fn test_strip_overlap_word_fallback_handles_punctuation_drift() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "Thank you.".to_string();

    let res = pipeline.strip_overlap("Thank you very much");
    assert_eq!(res, "very much");
}

#[test]
fn test_strip_overlap_word_fallback_handles_polish_diacritic_drift() {
    let mut pipeline = TranscriptionPipeline::new(Some("pl".to_string()));
    pipeline.last_suffix = "pacjent czuje się już dobrze".to_string();

    let res = pipeline.strip_overlap("pacjent czuje się juz dobrze dzisiaj");
    assert_eq!(res, "dzisiaj");
}

#[test]
fn test_postprocess_with_reason_uses_fuzzy_overlap_dedup() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "the patient is feeling much better".to_string();

    let result = pipeline.postprocess_with_reason("the patient is feelingg much better today");
    assert_eq!(
        result.expect("postprocess should keep non-overlap tail"),
        "today"
    );
}

#[test]
fn test_postprocess_prefers_timestamp_overlap_when_segments_exist() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "unrelated suffix".to_string();
    pipeline.last_segment_end_ts = Some(1.0);

    let segments = vec![
        TranscriptSegment {
            text: "already emitted".to_string(),
            start_ts: 0.0,
            end_ts: 0.95,
        },
        TranscriptSegment {
            text: "fresh words".to_string(),
            start_ts: 1.0,
            end_ts: 1.50,
        },
    ];

    let cleaned = pipeline
        .postprocess_with_reason_and_segments("this text should not win", &segments)
        .expect("timestamp-aware strip should keep only fresh segment text");
    assert_eq!(cleaned, "fresh words");
    assert_eq!(pipeline.last_segment_end_ts, Some(1.50));
}

#[test]
fn test_postprocess_with_segments_falls_back_to_text_path() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "hello world".to_string();
    pipeline.last_segment_end_ts = Some(7.0);

    let cleaned = pipeline
        .postprocess_with_reason_and_segments("world again", &[])
        .expect("empty segments should use suffix overlap fallback");
    assert_eq!(cleaned, "again");
    assert_eq!(
        pipeline.last_segment_end_ts,
        Some(7.0),
        "text fallback should not mutate timestamp overlap cursor"
    );
}

#[test]
fn test_short_utterance_gate_requires_low_confidence() {
    let sample_rate = 16_000;
    let short = (0.2 * sample_rate as f32) as usize;
    assert!(should_drop_short_utterance(short, sample_rate, 0.40));
    assert!(!should_drop_short_utterance(short, sample_rate, 0.80));
}

#[test]
fn test_utterance_vad_speech_pct_reports_ratio_in_percent() {
    let sample_rate = 48_000;
    let audio_samples = 48_000;
    let speech_vad_samples = 8_000;

    let speech_pct = utterance_vad_speech_pct(audio_samples, sample_rate, speech_vad_samples)
        .expect("expected speech ratio");

    assert!((speech_pct - 50.0).abs() < f32::EPSILON);
}

#[test]
fn test_enqueue_pending_utterance_preserves_final_boundary_when_full() {
    let mut pending = VecDeque::new();
    pending.push_back(pending_item(false));
    pending.push_back(pending_item(false));

    let outcome = enqueue_pending_utterance(&mut pending, pending_item(true), 2);
    assert!(outcome.enqueued, "final item should be admitted");
    assert_eq!(outcome.dropped, 1, "one older non-final should be evicted");
    assert!(
        !outcome.evicted_final,
        "non-final eviction should be preferred for final boundaries"
    );
    assert_eq!(pending.len(), 2);
    assert!(
        pending.back().is_some_and(|item| item.is_final),
        "latest queued item should be final boundary"
    );
}

#[test]
fn test_enqueue_pending_utterance_drops_non_final_when_full() {
    let mut pending = VecDeque::new();
    pending.push_back(pending_item_with_marker(false, 1.0));
    pending.push_back(pending_item_with_marker(true, 2.0));

    let outcome = enqueue_pending_utterance(&mut pending, pending_item_with_marker(false, 3.0), 2);
    assert!(!outcome.enqueued);
    assert_eq!(outcome.dropped, 1);
    assert_eq!(pending.len(), 2, "queue should stay unchanged");
    let markers: Vec<f32> = pending.iter().map(|item| item.audio[0]).collect();
    assert_eq!(
        markers,
        vec![1.0, 2.0],
        "dropping a non-final under pressure must preserve queued work order"
    );
}

#[test]
fn test_enqueue_pending_utterance_still_admits_final_when_only_finals_queued() {
    let mut pending = VecDeque::new();
    pending.push_back(pending_item(true));
    pending.push_back(pending_item(true));

    let outcome = enqueue_pending_utterance(&mut pending, pending_item(true), 2);
    assert!(outcome.enqueued, "latest final should still be admitted");
    assert_eq!(outcome.dropped, 1, "one older final should be evicted");
    assert!(outcome.evicted_final);
    assert_eq!(pending.len(), 2);
    assert!(pending.back().is_some_and(|item| item.is_final));
}

#[test]
fn test_enqueue_pending_utterance_zero_capacity_drops_all_items() {
    let mut pending = VecDeque::new();

    let non_final = enqueue_pending_utterance(&mut pending, pending_item(false), 0);
    assert!(!non_final.enqueued);
    assert_eq!(non_final.dropped, 1);
    assert!(!non_final.evicted_final);

    let final_item = enqueue_pending_utterance(&mut pending, pending_item(true), 0);
    assert!(!final_item.enqueued);
    assert_eq!(final_item.dropped, 1);
    assert!(!final_item.evicted_final);
    assert!(
        pending.is_empty(),
        "zero-capacity queue should never retain pending work"
    );
}

#[test]
fn test_enqueue_pending_utterance_final_evicts_oldest_non_final_in_mixed_queue() {
    let mut pending = VecDeque::new();
    pending.push_back(pending_item_with_marker(true, 1.0));
    pending.push_back(pending_item_with_marker(false, 2.0));
    pending.push_back(pending_item_with_marker(true, 3.0));

    let outcome = enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 4.0), 3);
    assert!(outcome.enqueued, "incoming final should be admitted");
    assert_eq!(outcome.dropped, 1);
    assert!(
        !outcome.evicted_final,
        "queue policy should evict a non-final before any final boundary"
    );
    let markers: Vec<f32> = pending.iter().map(|item| item.audio[0]).collect();
    assert_eq!(
        markers,
        vec![1.0, 3.0, 4.0],
        "oldest non-final should be removed while preserving final boundaries"
    );
    assert!(pending.iter().all(|item| item.is_final));
}

#[test]
fn test_enqueue_pending_utterance_pressure_sequence_preserves_final_boundaries() {
    let mut pending = VecDeque::new();
    pending.push_back(pending_item_with_marker(false, 1.0));
    pending.push_back(pending_item_with_marker(false, 2.0));
    pending.push_back(pending_item_with_marker(false, 3.0));

    let drop_non_final =
        enqueue_pending_utterance(&mut pending, pending_item_with_marker(false, 4.0), 3);
    assert!(!drop_non_final.enqueued);
    assert_eq!(drop_non_final.dropped, 1);
    assert!(!drop_non_final.evicted_final);
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![1.0, 2.0, 3.0]
    );

    let admit_final_a =
        enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 5.0), 3);
    assert!(admit_final_a.enqueued);
    assert_eq!(admit_final_a.dropped, 1);
    assert!(!admit_final_a.evicted_final);
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![2.0, 3.0, 5.0]
    );

    let admit_final_b =
        enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 6.0), 3);
    assert!(admit_final_b.enqueued);
    assert_eq!(admit_final_b.dropped, 1);
    assert!(!admit_final_b.evicted_final);
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![3.0, 5.0, 6.0]
    );

    let admit_final_c =
        enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 7.0), 3);
    assert!(admit_final_c.enqueued);
    assert_eq!(admit_final_c.dropped, 1);
    assert!(!admit_final_c.evicted_final);
    assert!(pending.iter().all(|item| item.is_final));
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![5.0, 6.0, 7.0]
    );

    let drop_non_final_again =
        enqueue_pending_utterance(&mut pending, pending_item_with_marker(false, 8.0), 3);
    assert!(!drop_non_final_again.enqueued);
    assert_eq!(drop_non_final_again.dropped, 1);
    assert!(!drop_non_final_again.evicted_final);
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![5.0, 6.0, 7.0]
    );

    let admit_final_d =
        enqueue_pending_utterance(&mut pending, pending_item_with_marker(true, 9.0), 3);
    assert!(admit_final_d.enqueued);
    assert_eq!(admit_final_d.dropped, 1);
    assert!(
        admit_final_d.evicted_final,
        "when only finals are queued, oldest final should be evicted"
    );
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![6.0, 7.0, 9.0]
    );
    assert!(pending.iter().all(|item| item.is_final));
}

#[tokio::test]
async fn test_enqueue_pending_utterance_pressure_sequence_under_async_saturated_load() {
    let mut pending = VecDeque::new();
    let mut dropped_total = 0u64;
    let mut dropped_non_finals = 0u64;
    let mut final_evictions = 0u64;

    let (tx, mut rx) = mpsc::channel::<PendingUtteranceWorkItem>(32);
    let producer = tokio::spawn(async move {
        let sequence = [
            (false, 1.0),
            (false, 2.0),
            (false, 3.0),
            (false, 4.0),
            (true, 5.0),
            (false, 6.0),
            (true, 7.0),
            (true, 8.0),
            (true, 9.0),
            (false, 10.0),
            (true, 11.0),
        ];
        for (is_final, marker) in sequence {
            tx.send(pending_item_with_marker(is_final, marker))
                .await
                .expect("async pressure sequence send should succeed");
            tokio::task::yield_now().await;
        }
    });

    while let Some(item) = rx.recv().await {
        // Simulate saturated inference slots by not draining the pending queue.
        let item_is_final = item.is_final;
        let outcome = enqueue_pending_utterance(&mut pending, item, 4);
        dropped_total = dropped_total.saturating_add(outcome.dropped);
        if !item_is_final && !outcome.enqueued {
            dropped_non_finals = dropped_non_finals.saturating_add(outcome.dropped);
        }
        if outcome.evicted_final {
            final_evictions = final_evictions.saturating_add(1);
        }
    }
    producer
        .await
        .expect("async pressure producer should finish");

    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0])
            .collect::<Vec<f32>>(),
        vec![7.0, 8.0, 9.0, 11.0],
        "saturated async ingress should preserve newest final boundaries"
    );
    assert_eq!(pending.len(), 4);
    assert!(pending.iter().all(|item| item.is_final));
    assert_eq!(dropped_total, 7);
    assert_eq!(dropped_non_finals, 2);
    assert_eq!(final_evictions, 1);
}

#[tokio::test]
async fn test_enqueue_pending_utterance_async_backpressure_recovers_after_drain() {
    let mut pending = VecDeque::new();
    let mut outcomes = Vec::new();
    let mut drained_marker = None;

    let (tx, mut rx) = mpsc::channel::<PendingUtteranceWorkItem>(16);
    let producer = tokio::spawn(async move {
        let sequence = [
            (true, 1.0),
            (true, 2.0),
            (true, 3.0),
            (false, 4.0),
            (true, 5.0),
            (false, 6.0),
            (true, 7.0),
        ];
        for (is_final, marker) in sequence {
            tx.send(pending_item_with_marker(is_final, marker))
                .await
                .expect("async queue-recovery sequence send should succeed");
            tokio::task::yield_now().await;
        }
    });

    while let Some(item) = rx.recv().await {
        let marker = item.audio[0] as u32;
        let outcome = enqueue_pending_utterance(&mut pending, item, 3);
        outcomes.push((
            marker,
            outcome.enqueued,
            outcome.dropped,
            outcome.evicted_final,
        ));

        // Simulate one inference slot freeing after the queue saturated with finals.
        if marker == 5 {
            let drained = pending
                .pop_front()
                .expect("simulated inference drain should pop one queued item");
            drained_marker = Some(drained.audio[0] as u32);
        }

        tokio::task::yield_now().await;
    }
    producer
        .await
        .expect("async queue-recovery producer should finish");

    assert_eq!(
        outcomes,
        vec![
            (1, true, 0, false),
            (2, true, 0, false),
            (3, true, 0, false),
            (4, false, 1, false),
            (5, true, 1, true),
            (6, true, 0, false),
            (7, true, 1, false),
        ],
        "backpressure policy should drop non-finals when saturated, recover after drain, and keep final precedence"
    );
    assert_eq!(
        drained_marker,
        Some(2),
        "drain should remove the current oldest final after a final-only eviction cycle"
    );
    assert_eq!(
        pending
            .iter()
            .map(|item| item.audio[0] as u32)
            .collect::<Vec<u32>>(),
        vec![3, 5, 7],
        "final enqueue after recovery should evict queued non-final first"
    );
    assert!(
        pending.iter().all(|item| item.is_final),
        "final boundaries should remain intact at the tail of async pressure+drain sequence"
    );
}

#[test]
fn test_partial_trigger_contract_utterance_path() {
    let now = Instant::now();
    let mut state = PartialPassTriggerState::new(now);

    for _ in 0..PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS.saturating_sub(1) {
        state.observe_speech_event(true, 0);
    }
    assert_eq!(
        classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
        None
    );

    state.observe_speech_event(true, 0);
    assert_eq!(
        classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
        Some(PartialPassTrigger::Utterance),
        "{} UtteranceFinal events should trigger partial pass",
        PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS
    );
}

#[test]
fn test_partial_trigger_contract_silero_speech_path() {
    let now = Instant::now();
    let mut state = PartialPassTriggerState::new(now);
    let samples_per_ms = u64::from(vad::VAD_SAMPLE_RATE) / 1_000;
    assert!(
        samples_per_ms > 0,
        "VAD sample rate must support ms conversion in tests"
    );
    let threshold_samples = PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS
        .saturating_mul(u64::from(vad::VAD_SAMPLE_RATE))
        / 1_000;
    let below_threshold_samples = threshold_samples.saturating_sub(samples_per_ms);

    state.observe_speech_event(false, below_threshold_samples);
    assert_eq!(
        classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
        None
    );

    state.observe_speech_event(false, samples_per_ms);
    assert_eq!(
        classify_partial_trigger(state.evaluate(now + Duration::from_secs(1))),
        Some(PartialPassTrigger::Speech),
        "{}ms of Silero-positive speech should trigger partial pass",
        PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS
    );
}

#[test]
fn test_partial_trigger_contract_timer_path() {
    let now = Instant::now();
    let state = PartialPassTriggerState::new(now);

    assert_eq!(
        classify_partial_trigger(state.evaluate(
            now + Duration::from_millis(PARTIAL_PASS_TRIGGER_TIMER_MS.saturating_sub(1))
        )),
        None
    );
    assert_eq!(
        classify_partial_trigger(
            state.evaluate(now + Duration::from_millis(PARTIAL_PASS_TRIGGER_TIMER_MS))
        ),
        Some(PartialPassTrigger::Timer),
        "{}ms timer should trigger partial pass",
        PARTIAL_PASS_TRIGGER_TIMER_MS
    );
}

#[test]
fn test_partial_trigger_precedence_prefers_speech_over_timer_without_utterance_trigger() {
    let now = Instant::now();
    let mut state = PartialPassTriggerState::new(now);

    state.observe_speech_event(false, u64::from(vad::VAD_SAMPLE_RATE) * 6);
    let flags = state.evaluate(now + Duration::from_millis(PARTIAL_PASS_TRIGGER_TIMER_MS));
    assert!(!flags.utterance_finals);
    assert!(flags.silero_speech);
    assert!(flags.timer);
    assert_eq!(
        classify_partial_trigger(flags),
        Some(PartialPassTrigger::Speech),
        "speech trigger should outrank timer when utterance-count threshold is not met"
    );
}

#[test]
fn test_partial_trigger_precedence_matrix_is_explicit() {
    assert_eq!(
        classify_partial_trigger(PartialPassTriggerFlags {
            utterance_finals: true,
            silero_speech: true,
            timer: true,
        }),
        Some(PartialPassTrigger::Utterance),
        "utterance-count trigger should dominate when multiple trigger paths are true"
    );
    assert_eq!(
        classify_partial_trigger(PartialPassTriggerFlags {
            utterance_finals: false,
            silero_speech: true,
            timer: true,
        }),
        Some(PartialPassTrigger::Speech),
        "speech trigger should outrank timer when utterance threshold is not met"
    );
    assert_eq!(
        classify_partial_trigger(PartialPassTriggerFlags {
            utterance_finals: false,
            silero_speech: false,
            timer: true,
        }),
        Some(PartialPassTrigger::Timer),
        "timer should be selected when it is the only triggered path"
    );
}

#[test]
fn test_partial_trigger_precedence_matrix_covers_all_flag_combinations() {
    let cases = [
        (false, false, false, None),
        (false, false, true, Some(PartialPassTrigger::Timer)),
        (false, true, false, Some(PartialPassTrigger::Speech)),
        (false, true, true, Some(PartialPassTrigger::Speech)),
        (true, false, false, Some(PartialPassTrigger::Utterance)),
        (true, false, true, Some(PartialPassTrigger::Utterance)),
        (true, true, false, Some(PartialPassTrigger::Utterance)),
        (true, true, true, Some(PartialPassTrigger::Utterance)),
    ];

    for (utterance_finals, silero_speech, timer, expected) in cases {
        assert_eq!(
            classify_partial_trigger(PartialPassTriggerFlags {
                utterance_finals,
                silero_speech,
                timer,
            }),
            expected,
            "trigger precedence mismatch for flags: utterance_finals={utterance_finals}, silero_speech={silero_speech}, timer={timer}"
        );
    }
}

#[test]
fn test_partial_trigger_coalesces_and_reset_clears_timer_baseline() {
    let now = Instant::now();
    let mut state = PartialPassTriggerState::new(now);
    let two_seconds = u64::from(vad::VAD_SAMPLE_RATE) * 2;

    for _ in 0..3 {
        state.observe_speech_event(true, two_seconds);
    }
    let due_at = now + Duration::from_millis(12_000);
    let flags = state.evaluate(due_at);
    assert!(flags.utterance_finals);
    assert!(flags.silero_speech);
    assert!(flags.timer);
    assert_eq!(
        classify_partial_trigger(flags),
        Some(PartialPassTrigger::Utterance),
        "simultaneous triggers should coalesce into one deterministic run"
    );

    state.reset_after_success(due_at);
    assert_eq!(
        classify_partial_trigger(state.evaluate(due_at + Duration::from_millis(1))),
        None,
        "successful partial pass must reset timer baseline"
    );
}

#[test]
fn test_partial_trigger_reset_clears_utterance_and_speech_accumulators() {
    let now = Instant::now();
    let mut state = PartialPassTriggerState::new(now);
    let two_seconds = u64::from(vad::VAD_SAMPLE_RATE) * 2;

    for _ in 0..3 {
        state.observe_speech_event(true, two_seconds);
    }
    let due_at = now + Duration::from_millis(PARTIAL_PASS_TRIGGER_TIMER_MS);
    assert_eq!(
        classify_partial_trigger(state.evaluate(due_at)),
        Some(PartialPassTrigger::Utterance)
    );

    state.reset_after_success(due_at);
    assert_eq!(
        state.evaluate(due_at + Duration::from_millis(1)),
        PartialPassTriggerFlags::default(),
        "reset should clear all trigger counters and timer elapsed time"
    );

    for _ in 0..PARTIAL_PASS_TRIGGER_UTTERANCE_FINALS.saturating_sub(1) {
        state.observe_speech_event(true, two_seconds);
    }
    assert_eq!(
        classify_partial_trigger(state.evaluate(due_at + Duration::from_millis(10))),
        None,
        "post-reset counters should require fresh accumulation before triggering again"
    );
}

#[tokio::test]
async fn test_partial_trigger_paths_stay_stable_under_async_interleaving() {
    #[derive(Clone, Copy)]
    enum TriggerStep {
        Observe {
            is_final: bool,
            speech_samples: u64,
            advance_ms: u64,
        },
        Evaluate {
            advance_ms: u64,
            expected: Option<PartialPassTrigger>,
            reset_after_success: bool,
        },
    }

    let one_second = u64::from(vad::VAD_SAMPLE_RATE);
    let sequence = [
        TriggerStep::Observe {
            is_final: true,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: true,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: true,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Evaluate {
            advance_ms: 100,
            expected: Some(PartialPassTrigger::Utterance),
            reset_after_success: true,
        },
        TriggerStep::Observe {
            is_final: false,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: false,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: false,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: false,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: false,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Observe {
            is_final: false,
            speech_samples: one_second,
            advance_ms: 100,
        },
        TriggerStep::Evaluate {
            advance_ms: 100,
            expected: Some(PartialPassTrigger::Speech),
            reset_after_success: true,
        },
        TriggerStep::Evaluate {
            advance_ms: PARTIAL_PASS_TRIGGER_TIMER_MS - 1,
            expected: None,
            reset_after_success: false,
        },
        TriggerStep::Evaluate {
            advance_ms: 1,
            expected: Some(PartialPassTrigger::Timer),
            reset_after_success: true,
        },
        TriggerStep::Evaluate {
            advance_ms: 1,
            expected: None,
            reset_after_success: false,
        },
    ];

    let (tx, mut rx) = mpsc::channel::<TriggerStep>(sequence.len());
    let producer = tokio::spawn(async move {
        for step in sequence {
            tx.send(step)
                .await
                .expect("trigger-step sequence send should succeed");
            tokio::task::yield_now().await;
        }
    });

    let start = Instant::now();
    let mut now = start;
    let mut state = PartialPassTriggerState::new(start);
    let mut telemetry = PartialPassTelemetry::default();

    while let Some(step) = rx.recv().await {
        match step {
            TriggerStep::Observe {
                is_final,
                speech_samples,
                advance_ms,
            } => {
                now += Duration::from_millis(advance_ms);
                state.observe_speech_event(is_final, speech_samples);
            }
            TriggerStep::Evaluate {
                advance_ms,
                expected,
                reset_after_success,
            } => {
                now += Duration::from_millis(advance_ms);
                let observed = classify_partial_trigger(state.evaluate(now));
                assert_eq!(
                    observed, expected,
                    "trigger classification drifted under async interleaving"
                );
                if let Some(trigger) = observed {
                    telemetry.record_run(trigger);
                    if reset_after_success {
                        state.reset_after_success(now);
                    }
                }
            }
        }
        tokio::task::yield_now().await;
    }
    producer
        .await
        .expect("async trigger-step producer should finish");

    assert_eq!(telemetry.runs_total, 3);
    assert_eq!(telemetry.trigger_utterance_count, 1);
    assert_eq!(telemetry.trigger_speech_count, 1);
    assert_eq!(telemetry.trigger_timer_count, 1);
}

#[test]
fn test_word_rate_detection() {
    let sample_rate = 16_000;
    let half_second = (0.5 * sample_rate as f32) as usize;
    let wps = text_words_per_second("raz dwa trzy cztery pięć sześć", half_second, sample_rate)
        .expect("should compute words/s");
    assert!(wps > MAX_WORDS_PER_SEC);

    let normal = text_words_per_second(
        "to jest normalna fraza z kilkoma słowami",
        (sample_rate * 2) as usize,
        sample_rate,
    )
    .expect("should compute words/s");
    assert!(normal < MAX_WORDS_PER_SEC);
}

#[test]
fn test_suffix_preserved_when_postprocess_filters() {
    // Simulates the re-transcription scenario: if postprocess drops content
    // (e.g. hallucination), last_suffix must stay at the pre-snapshot value.
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "original suffix".to_string();

    // "Thank you" is a hallucination — postprocess returns a drop reason.
    let result = pipeline.postprocess_with_reason("Thank you");
    assert!(matches!(result, Err(PostprocessDrop::Hallucination)));
    // last_suffix unchanged (strip_overlap was never reached)
    assert_eq!(pipeline.last_suffix, "original suffix");
}

#[test]
fn test_suffix_updated_after_successful_postprocess() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "old tail".to_string();

    let result = pipeline.postprocess_with_reason("This is a brand new sentence.");
    assert!(result.is_ok());
    // last_suffix should now reflect the new text's suffix
    assert_ne!(pipeline.last_suffix, "old tail");
    assert!(pipeline.last_suffix.contains("sentence"));
}

#[test]
fn test_correction_guard_keeps_wide_rewrites_pending() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let buf = Arc::new(Mutex::new(String::new()));
        let mut emitter = BufferedEmitter::new(buf, None, None);
        emitter.emitted_text = "Hello world, this is a test.".to_string();

        // Completely different text should still be queued as a correction.
        emitter.push_correction("Goodbye universe, nothing alike.".to_string());
        assert_eq!(
            emitter.correction_pending.as_deref(),
            Some("Goodbye universe, nothing alike.")
        );

        // Similar text with minor tail fix should also be accepted.
        emitter.push_correction("Hello world, this is a test!".to_string());
        assert_eq!(
            emitter.correction_pending.as_deref(),
            Some("Hello world, this is a test!")
        );
    });
}

#[test]
fn test_correction_bootstraps_when_no_output_emitted_yet() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let buf = Arc::new(Mutex::new(String::new()));
        let mut emitter = BufferedEmitter::new(buf, None, None);

        emitter.push_segment("draft".to_string());
        emitter.push_correction("draft corrected".to_string());

        assert_eq!(emitter.queue.len(), 2);
        assert_eq!(
            emitter.queue.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["draft", " corrected"]
        );
        assert!(emitter.correction_pending.is_none());
        assert_eq!(emitter.target_text, "draft corrected");
    });
}

#[test]
fn test_correction_delta() {
    let before = "This is a dratf.";
    let after = "This is a draft.";
    let delta = build_redacted_delta(before, after).expect("should produce delta");

    assert!(delta.contains("\u{0008}\u{0008}\u{0008}"));
    assert!(delta.ends_with("ft."));

    let mut target = before.to_string();
    apply_delta_to_string(&mut target, &delta);
    assert_eq!(target, after);
}

#[test]
fn test_correction_delta_polish_diacritics() {
    let before = "chciałbym zostać weterynarzem.";
    let after = "chciałbym zostać weterynarzem!";
    let delta = build_redacted_delta(before, after).expect("should produce delta");

    let mut target = before.to_string();
    apply_delta_to_string(&mut target, &delta);
    assert_eq!(target, after);
}

#[test]
fn test_correction_stale_guard_detects_preview_rev_drift() {
    assert!(correction_is_stale(7, 8, "draft", "draft"));
    assert!(!correction_is_stale(7, 7, "draft", "draft"));
}

#[test]
fn test_correction_stale_guard_detects_text_drift() {
    assert!(correction_is_stale(9, 9, "ala ma", "ala ma kota"));
}

#[test]
fn test_partial_telemetry_counters_accumulate() {
    let mut telemetry = PartialPassTelemetry::default();
    telemetry.record_run(PartialPassTrigger::Utterance);
    telemetry.record_run(PartialPassTrigger::Speech);
    telemetry.record_run(PartialPassTrigger::Timer);
    telemetry.record_stale();
    telemetry.record_coalesced();
    telemetry.record_dropped();

    assert_eq!(telemetry.runs_total, 3);
    assert_eq!(telemetry.trigger_utterance_count, 1);
    assert_eq!(telemetry.trigger_speech_count, 1);
    assert_eq!(telemetry.trigger_timer_count, 1);
    assert_eq!(telemetry.stale_count, 1);
    assert_eq!(telemetry.coalesced_count, 1);
    assert_eq!(telemetry.dropped_count, 1);
}

#[tokio::test]
async fn test_schedule_partial_pass_coalesces_under_async_scheduler_pressure() {
    let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
    let gate = Arc::new((StdMutex::new(false), Condvar::new()));
    let started_ref = Arc::clone(&started);
    let gate_ref = Arc::clone(&gate);

    let infer = Arc::new(
        move |samples: Vec<f32>,
              _sample_rate: u32,
              _language: Option<String>,
              _initial_prompt: Option<String>|
              -> Result<RawTranscript> {
            let id = samples.first().copied().unwrap_or_default() as u32;
            started_ref
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(id);
            if id == 100 {
                let (lock, cvar) = &*gate_ref;
                let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                while !*released {
                    released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                }
            }
            Ok(RawTranscript {
                text: format!("job-{id}"),
                segments: Vec::new(),
                ..Default::default()
            })
        },
    );

    let scheduler = SttScheduler::with_infer_fn(infer);
    let mut blocker = scheduler
        .submit(SttLane::Live, vec![100.0], 16_000, None)
        .expect("submit blocking live request");

    let collector = Arc::new(CollectorEventSink::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let mut correction_in_flight: Option<SttTaskHandle> = None;
    let mut correction_expected_preview_rev: Option<u64> = None;
    let mut correction_expected_text: Option<String> = None;
    let mut correction_suffix_snapshot: Option<String> = None;
    let mut partial_telemetry = PartialPassTelemetry::default();

    let mut first_audio = vec![21.0];
    assert!(schedule_partial_pass(
        &scheduler,
        16_000,
        Some("en".to_string()),
        &mut first_audio,
        &mut correction_in_flight,
        &mut correction_expected_preview_rev,
        &mut correction_expected_text,
        &mut correction_suffix_snapshot,
        "suffix-a",
        7,
        "draft-a",
        PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS,
        PartialPassTrigger::Timer,
        &mut partial_telemetry,
        &event_sink,
    ));
    assert!(
        first_audio.is_empty(),
        "correction audio buffer should be consumed on schedule"
    );
    assert_eq!(
        correction_expected_preview_rev,
        Some(7),
        "tracked preview revision should match first scheduled correction"
    );
    assert_eq!(
        correction_expected_text.as_deref(),
        Some("draft-a"),
        "tracked expected text should match first scheduled correction"
    );
    assert_eq!(
        correction_suffix_snapshot.as_deref(),
        Some("suffix-a"),
        "tracked suffix snapshot should match first scheduled correction"
    );
    let first_id = correction_in_flight
        .as_ref()
        .expect("first correction handle should be tracked")
        .id();

    let mut second_audio = vec![22.0];
    assert!(schedule_partial_pass(
        &scheduler,
        16_000,
        Some("en".to_string()),
        &mut second_audio,
        &mut correction_in_flight,
        &mut correction_expected_preview_rev,
        &mut correction_expected_text,
        &mut correction_suffix_snapshot,
        "suffix-b",
        8,
        "draft-b",
        PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS,
        PartialPassTrigger::Speech,
        &mut partial_telemetry,
        &event_sink,
    ));
    let second_id = correction_in_flight
        .as_ref()
        .expect("latest correction handle should replace old in-flight handle")
        .id();
    assert!(
        second_id > first_id,
        "newly scheduled correction should replace the previous tracked handle"
    );
    assert_eq!(partial_telemetry.runs_total, 2);
    assert_eq!(partial_telemetry.trigger_timer_count, 1);
    assert_eq!(partial_telemetry.trigger_speech_count, 1);
    assert_eq!(partial_telemetry.trigger_utterance_count, 0);
    assert_eq!(partial_telemetry.coalesced_count, 1);
    assert_eq!(partial_telemetry.stale_count, 0);
    assert_eq!(partial_telemetry.dropped_count, 0);
    assert_eq!(
        correction_expected_preview_rev,
        Some(8),
        "new schedule should overwrite tracked preview revision"
    );
    assert_eq!(
        correction_expected_text.as_deref(),
        Some("draft-b"),
        "new schedule should overwrite tracked expected text"
    );
    assert_eq!(
        correction_suffix_snapshot.as_deref(),
        Some("suffix-b"),
        "new schedule should overwrite tracked suffix snapshot"
    );

    {
        let (lock, cvar) = &*gate;
        let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
        *released = true;
        cvar.notify_all();
    }

    let blocking_result = tokio::time::timeout(Duration::from_secs(2), blocker.recv())
        .await
        .expect("blocking live request timed out")
        .expect("blocking live request should finish");
    assert_eq!(blocking_result.text, "job-100");
    assert!(blocking_result.segments.is_empty());

    let mut correction_handle = correction_in_flight
        .take()
        .expect("latest correction handle should remain in-flight");
    let correction_result = tokio::time::timeout(Duration::from_secs(2), correction_handle.recv())
        .await
        .expect("latest correction request timed out")
        .expect("latest correction request should complete");
    assert_eq!(correction_result.text, "job-22");
    assert!(correction_result.segments.is_empty());

    tokio::time::timeout(Duration::from_secs(2), scheduler.shutdown())
        .await
        .expect("scheduler shutdown timed out")
        .expect("scheduler shutdown");

    assert_eq!(
        started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec![100, 22],
        "superseded correction should not execute when scheduler is saturated"
    );
    assert!(
        collector
            .events()
            .iter()
            .all(|event| !matches!(event, EngineEvent::Warning { .. })),
        "successful partial scheduling should not emit warning events"
    );
}

#[tokio::test]
async fn test_schedule_partial_pass_repeated_coalescing_under_async_pressure() {
    let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
    let gate = Arc::new((StdMutex::new(false), Condvar::new()));
    let started_ref = Arc::clone(&started);
    let gate_ref = Arc::clone(&gate);

    let infer = Arc::new(
        move |samples: Vec<f32>,
              _sample_rate: u32,
              _language: Option<String>,
              _initial_prompt: Option<String>|
              -> Result<RawTranscript> {
            let id = samples.first().copied().unwrap_or_default() as u32;
            started_ref
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(id);
            if id == 100 {
                let (lock, cvar) = &*gate_ref;
                let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                while !*released {
                    released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                }
            }
            Ok(RawTranscript {
                text: format!("job-{id}"),
                segments: Vec::new(),
                ..Default::default()
            })
        },
    );

    let scheduler = SttScheduler::with_infer_fn(infer);
    let mut blocker = scheduler
        .submit(SttLane::Live, vec![100.0], 16_000, None)
        .expect("submit blocking live request");

    let collector = Arc::new(CollectorEventSink::new());
    let event_sink: Arc<dyn EventSink> = collector.clone();
    let mut correction_in_flight: Option<SttTaskHandle> = None;
    let mut correction_expected_preview_rev: Option<u64> = None;
    let mut correction_expected_text: Option<String> = None;
    let mut correction_suffix_snapshot: Option<String> = None;
    let mut partial_telemetry = PartialPassTelemetry::default();
    let trigger_sequence = [
        PartialPassTrigger::Utterance,
        PartialPassTrigger::Speech,
        PartialPassTrigger::Timer,
        PartialPassTrigger::Speech,
        PartialPassTrigger::Timer,
    ];
    let first_marker = 31u32;
    let expected_last_id = first_marker + trigger_sequence.len() as u32 - 1;

    for (index, trigger) in trigger_sequence.iter().copied().enumerate() {
        let marker = 31.0 + index as f32;
        let expected_rev = 21 + index as u64;
        let expected_text = format!("draft-{index}");
        let expected_suffix = format!("suffix-{index}");
        let mut audio = vec![marker];

        assert!(schedule_partial_pass(
            &scheduler,
            16_000,
            Some("en".to_string()),
            &mut audio,
            &mut correction_in_flight,
            &mut correction_expected_preview_rev,
            &mut correction_expected_text,
            &mut correction_suffix_snapshot,
            &expected_suffix,
            expected_rev,
            &expected_text,
            PARTIAL_PASS_TRIGGER_SILERO_SPEECH_MS + index as u64,
            trigger,
            &mut partial_telemetry,
            &event_sink,
        ));
        assert!(
            audio.is_empty(),
            "schedule should consume correction audio buffer"
        );
        assert_eq!(correction_expected_preview_rev, Some(expected_rev));
        assert_eq!(
            correction_expected_text.as_deref(),
            Some(expected_text.as_str())
        );
        assert_eq!(
            correction_suffix_snapshot.as_deref(),
            Some(expected_suffix.as_str())
        );
    }

    {
        let (lock, cvar) = &*gate;
        let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
        *released = true;
        cvar.notify_all();
    }

    let blocking_result = tokio::time::timeout(Duration::from_secs(2), blocker.recv())
        .await
        .expect("blocking live request timed out")
        .expect("blocking live request should finish");
    assert_eq!(blocking_result.text, "job-100");

    let mut correction_handle = correction_in_flight
        .take()
        .expect("latest correction handle should remain in-flight");
    let correction_result = tokio::time::timeout(Duration::from_secs(2), correction_handle.recv())
        .await
        .expect("latest correction request timed out")
        .expect("latest correction request should complete");
    assert_eq!(correction_result.text, format!("job-{expected_last_id}"));
    assert!(correction_result.segments.is_empty());

    tokio::time::timeout(Duration::from_secs(2), scheduler.shutdown())
        .await
        .expect("scheduler shutdown timed out")
        .expect("scheduler shutdown");

    assert_eq!(
        started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        vec![100, expected_last_id],
        "coalescing under pressure should execute only the latest correction"
    );
    assert_eq!(partial_telemetry.runs_total, 5);
    assert_eq!(partial_telemetry.trigger_utterance_count, 1);
    assert_eq!(partial_telemetry.trigger_speech_count, 2);
    assert_eq!(partial_telemetry.trigger_timer_count, 2);
    assert_eq!(partial_telemetry.coalesced_count, 4);
    assert_eq!(partial_telemetry.stale_count, 0);
    assert_eq!(partial_telemetry.dropped_count, 0);
    assert!(
        collector
            .events()
            .iter()
            .all(|event| !matches!(event, EngineEvent::Warning { .. })),
        "successful coalescing should not emit warnings"
    );
}

#[tokio::test]
async fn transcription_session_emits_no_speech_and_stats_for_empty_input() {
    let (tx, rx) = mpsc::channel::<Vec<f32>>(1);
    drop(tx);
    let sink = Arc::new(CollectorEventSink::new());
    transcription_session(
        rx,
        sink.clone(),
        SessionConfig {
            sample_rate: 16_000,
            language: Some("pl".to_string()),
            stream_log_path: None,
            utterance_silence_sec: None,
        },
    )
    .await;

    let events = sink.events();

    let no_speech_pos = events
        .iter()
        .position(|event| matches!(event, EngineEvent::NoSpeech { .. }))
        .expect("session should emit NoSpeech for empty input");
    let stats_pos = events
        .iter()
        .position(|event| matches!(event, EngineEvent::Stats { .. }))
        .expect("session should emit Stats for empty input");
    assert!(
        no_speech_pos < stats_pos,
        "NoSpeech should be emitted before final Stats"
    );

    let mut no_speech_reason = None;
    let mut stats_count = 0u32;
    for event in &events {
        match event {
            EngineEvent::NoSpeech { reason } => {
                no_speech_reason = Some(reason.clone());
            }
            EngineEvent::Stats {
                dropped_audio_chunks,
                hallucination_drops,
                semantic_gate_drops,
                filtered_empty_drops,
                corrections_applied,
                total_utterances,
                partial_runs_total,
                trigger_utterance_count,
                trigger_speech_count,
                trigger_timer_count,
                partial_stale_count,
                partial_coalesced_count,
                partial_dropped_count,
            } => {
                stats_count += 1;
                assert_eq!(*dropped_audio_chunks, 0);
                assert_eq!(*hallucination_drops, 0);
                assert_eq!(*semantic_gate_drops, 0);
                assert_eq!(*filtered_empty_drops, 0);
                assert_eq!(*corrections_applied, 0);
                assert_eq!(*total_utterances, 0);
                assert_eq!(*partial_runs_total, 0);
                assert_eq!(*trigger_utterance_count, 0);
                assert_eq!(*trigger_speech_count, 0);
                assert_eq!(*trigger_timer_count, 0);
                assert_eq!(*partial_stale_count, 0);
                assert_eq!(*partial_coalesced_count, 0);
                assert_eq!(*partial_dropped_count, 0);
            }
            _ => {}
        }
    }

    assert_eq!(
        no_speech_reason.as_deref(),
        Some("vad_no_speech_detected"),
        "empty session should report VAD no-speech reason"
    );
    assert_eq!(stats_count, 1, "expected exactly one Stats event");
}

#[test]
fn transcription_events_keep_monotonic_previews_before_final() {
    let sink = CollectorEventSink::new();
    sink.on_event(&EngineEvent::Preview {
        rev: 1,
        text: "ala".to_string(),
    });
    sink.on_event(&EngineEvent::Preview {
        rev: 2,
        text: "ala ma".to_string(),
    });
    sink.on_event(&EngineEvent::UtteranceFinal {
        utterance_id: 1,
        text: "ala ma kota".to_string(),
        raw_text: "ala ma kota".to_string(),
        start_ts: 0.0,
        end_ts: 1.0,
        segments: Vec::new(),
        vad_speech_pct: Some(100.0),
        avg_logprob: None,
        compression_ratio: None,
        quality_gate_dropped: false,
        confidence_flags: Vec::new(),
    });

    let events = sink.events();
    let final_pos = events
        .iter()
        .position(|event| matches!(event, EngineEvent::UtteranceFinal { .. }))
        .expect("synthetic stream should include a final event");
    let preview_revs: Vec<(usize, u64)> = events
        .iter()
        .enumerate()
        .filter_map(|(pos, event)| match event {
            EngineEvent::Preview { rev, .. } => Some((pos, *rev)),
            _ => None,
        })
        .collect();

    assert!(
        !preview_revs.is_empty(),
        "stream should emit at least one preview before final"
    );
    assert!(
        preview_revs.iter().all(|(pos, _)| *pos < final_pos),
        "all previews must be emitted before UtteranceFinal"
    );
    for pair in preview_revs.windows(2) {
        assert!(
            pair[0].1 < pair[1].1,
            "Preview revisions must increase monotonically"
        );
    }
}

#[tokio::test]
async fn transcription_session_silent_callbacks_keep_no_speech_stats_coherent() {
    let (tx, rx) = mpsc::channel::<Vec<f32>>(1);
    let sender = tokio::spawn(async move {
        for i in 0..96usize {
            let len = if i % 2 == 0 { 371 } else { 1024 };
            tx.send(vec![0.0; len])
                .await
                .expect("silent callback send should succeed");
            tokio::task::yield_now().await;
        }
    });

    let sink = Arc::new(CollectorEventSink::new());
    transcription_session(
        rx,
        sink.clone(),
        SessionConfig {
            sample_rate: 48_000,
            language: Some("pl".to_string()),
            stream_log_path: None,
            utterance_silence_sec: None,
        },
    )
    .await;
    sender
        .await
        .expect("silent callback sender task should finish");

    let events = sink.events();
    let no_speech_pos = events
        .iter()
        .position(|event| matches!(event, EngineEvent::NoSpeech { .. }))
        .expect("session should emit NoSpeech for silence-only callbacks");
    let stats_pos = events
        .iter()
        .position(|event| matches!(event, EngineEvent::Stats { .. }))
        .expect("session should emit Stats for silence-only callbacks");
    assert!(
        no_speech_pos < stats_pos,
        "NoSpeech should be emitted before final Stats"
    );

    let mut no_speech_count = 0u32;
    let mut stats_count = 0u32;
    for event in &events {
        match event {
            EngineEvent::NoSpeech { reason } => {
                no_speech_count = no_speech_count.saturating_add(1);
                assert_eq!(reason, "vad_no_speech_detected");
            }
            EngineEvent::Stats {
                dropped_audio_chunks,
                hallucination_drops,
                semantic_gate_drops,
                filtered_empty_drops,
                corrections_applied,
                total_utterances,
                partial_runs_total,
                trigger_utterance_count,
                trigger_speech_count,
                trigger_timer_count,
                partial_stale_count,
                partial_coalesced_count,
                partial_dropped_count,
            } => {
                stats_count = stats_count.saturating_add(1);
                assert_eq!(*dropped_audio_chunks, 0);
                assert_eq!(*hallucination_drops, 0);
                assert_eq!(*semantic_gate_drops, 0);
                assert_eq!(*filtered_empty_drops, 0);
                assert_eq!(*corrections_applied, 0);
                assert_eq!(*total_utterances, 0);
                assert_eq!(*partial_runs_total, 0);
                assert_eq!(*trigger_utterance_count, 0);
                assert_eq!(*trigger_speech_count, 0);
                assert_eq!(*trigger_timer_count, 0);
                assert_eq!(*partial_stale_count, 0);
                assert_eq!(*partial_coalesced_count, 0);
                assert_eq!(*partial_dropped_count, 0);
            }
            _ => {}
        }
    }

    assert_eq!(no_speech_count, 1, "expected exactly one NoSpeech event");
    assert_eq!(stats_count, 1, "expected exactly one Stats event");
}

#[test]
fn test_postprocess_correction_with_snapshot_restores_suffix_on_drop() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "current-tail".to_string();

    let result = postprocess_correction_with_snapshot(&mut pipeline, "Thank you", "snapshot-tail");
    assert!(matches!(result, Err(PostprocessDrop::Hallucination)));
    assert_eq!(pipeline.last_suffix, "current-tail");
}

#[test]
fn test_apply_final_boundary_text_preserves_preview_when_cleaned_final_empty() {
    let mut accumulated = "to jest preview".to_string();
    let has_content = apply_final_boundary_text(&mut accumulated, "");
    assert!(
        has_content,
        "non-empty preview should survive empty final cleanup"
    );
    assert_eq!(accumulated, "to jest preview");
}

#[test]
fn test_apply_final_boundary_text_replaces_preview_with_cleaned_final() {
    let mut accumulated = "stary preview".to_string();
    let has_content = apply_final_boundary_text(&mut accumulated, "  finalny tekst  ");
    assert!(has_content);
    assert_eq!(accumulated, "finalny tekst");
}

#[test]
fn test_apply_final_boundary_text_reports_empty_when_no_preview_and_no_final() {
    let mut accumulated = String::new();
    let has_content = apply_final_boundary_text(&mut accumulated, "   ");
    assert!(!has_content);
    assert!(accumulated.is_empty());
}

#[test]
fn test_postprocess_correction_with_snapshot_updates_suffix_on_success() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "old-tail".to_string();

    let corrected = postprocess_correction_with_snapshot(
        &mut pipeline,
        "to jest poprawny tekst",
        "snapshot-tail",
    )
    .expect("correction should pass");
    assert!(!corrected.is_empty());
    assert_ne!(pipeline.last_suffix, "old-tail");
}

#[test]
fn test_correction_postprocess_remains_text_centric_with_timestamp_state() {
    let mut pipeline = TranscriptionPipeline::new(None);
    pipeline.last_suffix = "alpha beta".to_string();
    pipeline.last_segment_end_ts = Some(42.0);

    let corrected = postprocess_correction_with_snapshot(&mut pipeline, "beta gamma", "alpha beta")
        .expect("text-based correction path should remain active");
    assert_eq!(corrected, "gamma");
    assert_eq!(
        pipeline.last_segment_end_ts,
        Some(42.0),
        "correction flow should not depend on or mutate timestamp overlap state"
    );
}

// ── Fix A contract: FINAL must not inherit corrupted suffix from non-final ──

#[test]
fn test_fix_a_final_uses_boundary_suffix_not_nonfinal_suffix() {
    // Simulate: utterance boundary leaves suffix "abc". Then non-final
    // chunks advance pipeline.last_suffix to "xyz". When FINAL arrives,
    // it should see "abc" (the boundary snapshot), not "xyz".
    let mut pipeline = TranscriptionPipeline::new(None);

    // Initial utterance boundary — suffix is empty (session start).
    let utterance_boundary_suffix = pipeline.last_suffix.clone();
    assert_eq!(utterance_boundary_suffix, "");

    // Non-final chunk processing advances pipeline.last_suffix.
    let _ = pipeline.postprocess_with_reason("hello world");
    assert_ne!(
        pipeline.last_suffix, utterance_boundary_suffix,
        "non-final should advance last_suffix"
    );
    let corrupted_suffix = pipeline.last_suffix.clone();

    // Fix A: Restore boundary suffix before FINAL processing.
    pipeline.last_suffix = utterance_boundary_suffix.clone();
    let result = pipeline.postprocess_with_reason("hello world final version");
    assert!(result.is_ok());

    // Verify FINAL did NOT use the corrupted non-final suffix.
    // The boundary suffix was empty, so no overlap should be stripped.
    let cleaned = result.unwrap();
    assert!(
        cleaned.contains("hello"),
        "FINAL with restored boundary suffix should not aggressively strip: got '{}'",
        cleaned
    );

    // Without Fix A, pipeline.last_suffix would have been "corrupted_suffix"
    // causing strip_overlap to incorrectly remove matching text.
    assert_ne!(
        pipeline.last_suffix, corrupted_suffix,
        "after Fix A, pipeline.last_suffix should be updated from FINAL, not stuck on non-final's suffix"
    );
}

// ── Fix D contract: window-scoped stale guard survives utterance boundaries ──

#[test]
fn test_fix_d_stale_guard_with_window_rev_survives_final() {
    // Before Fix D: schedule_partial_pass stored preview_rev / accumulated_text.
    // After FINAL: accumulated_text.clear() → correction_is_stale could pass
    // when it shouldn't (empty == empty).
    //
    // After Fix D: schedule_partial_pass stores window_rev / window_text.
    // FINAL increments window_rev → correction_is_stale correctly detects staleness.

    let window_rev_at_schedule: u64 = 5;
    let window_text_at_schedule = "cześć jak się masz";

    // Simulate FINAL boundary advancing window state.
    let window_rev_after_final: u64 = 6; // FINAL incremented it
    let window_text_after_final = "cześć jak się masz dobrze";

    assert!(
        correction_is_stale(
            window_rev_at_schedule,
            window_rev_after_final,
            window_text_at_schedule,
            window_text_after_final,
        ),
        "correction scheduled before FINAL should be stale after FINAL"
    );
}

#[test]
fn test_fix_d_stale_guard_passes_when_window_unchanged() {
    // When no FINAL or new text arrives between schedule and correction result,
    // the window state matches and correction should apply.
    let window_rev: u64 = 5;
    let window_text = "cześć jak się masz";

    assert!(
        !correction_is_stale(window_rev, window_rev, window_text, window_text),
        "correction should not be stale when window state unchanged"
    );
}

#[test]
fn test_fix_d_empty_accumulated_text_after_final_detected_by_window_rev() {
    // Edge case: FINAL clears accumulated_text. Before Fix D, stale guard
    // compared "" vs "" → not stale → correction applies to empty text.
    // After Fix D, window_rev incremented by FINAL → stale.

    // Old behavior (broken): accumulated_text scope — expected "hello world"
    // vs current "" (cleared by FINAL). This would pass if revs matched
    // (both based on preview_rev which didn't increment for FINAL).

    // New behavior: window scope
    let window_rev_at_schedule: u64 = 3;
    let window_text_at_schedule = "hello world";
    let window_rev_after_final: u64 = 4; // FINAL bumped it
    let window_text_after_final = "hello world and more"; // FINAL appended

    assert!(
        correction_is_stale(
            window_rev_at_schedule,
            window_rev_after_final,
            window_text_at_schedule,
            window_text_after_final,
        ),
        "window-scoped stale guard must detect FINAL boundary crossing"
    );
}

#[test]
fn test_correction_baseline_prefers_live_accumulated_text() {
    let (baseline, boundary) = correction_baseline_text("draft live", "expected", "window");
    assert_eq!(baseline, "draft live");
    assert!(!boundary);
}

#[test]
fn test_correction_baseline_falls_back_after_boundary() {
    let (from_expected, expected_boundary) = correction_baseline_text("", "expected text", "");
    assert_eq!(from_expected, "expected text");
    assert!(expected_boundary);

    let (from_window, window_boundary) = correction_baseline_text("", "", "window text");
    assert_eq!(from_window, "window text");
    assert!(window_boundary);
}
