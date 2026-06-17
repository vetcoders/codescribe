//! Pre/post-inference quality gates: hallucination filtering, silence and
//! short-utterance gating, word-rate anomaly detection, and VAD telemetry.

use std::sync::Arc;

use crate::audio::chunker::SpeechSession;
use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::vad;

// ── Hallucination filter ─────────────────────────────────────────────────────

const WHISPER_HALLUCINATIONS_COMMON: &[&str] = &[
    "thank you",
    "thanks for watching",
    "thanks for listening",
    "dziękuję za uwagę",
    "do zobaczenia",
    "subscribe",
    "like and subscribe",
    ".com",
    "codescribe",
    "www.",
];

const WHISPER_HALLUCINATIONS_PL: &[&str] = &[
    "napisy stworzone przez społeczność",
    "tłumaczenie",
    "transkrypcja",
];

const SHORT_SPEECH_WHITELIST: &[&str] = &[
    "tak", "nie", "co?", "co", "dobra", "dobrze", "ok", "okej", "no", "no?", "mhm", "aha", "jasne",
    "pewnie", "super", "hej", "halo", "cześć", "siema", "dzięki", "proszę",
];

const MIN_UTTERANCE_SEC: f32 = 0.50;
const SHORT_UTTERANCE_LOW_CONFIDENCE: f32 = 0.55;
pub(crate) const MAX_WORDS_PER_SEC: f32 = 5.0;
const WORD_RATE_MIN_WORDS: usize = 6;

/// Minimum fraction of Silero-positive VAD frames required in an interim chunk
/// before sending it to Whisper. Chunks below this ratio are categorically
/// classified as silence and skipped — Silero is SoTA for this binary decision.
/// Only applies to non-final (interim) emissions; final/flush always transcribes.
pub(crate) const MIN_SPEECH_RATIO_FOR_INFERENCE: f32 = 0.15;

fn is_polish_language(language: Option<&str>) -> bool {
    language
        .map(|lang| {
            let normalized = lang.to_ascii_lowercase();
            normalized == "pl" || normalized.starts_with("pl-")
        })
        .unwrap_or(false)
}

pub(crate) fn text_words_per_second(
    text: &str,
    audio_samples: usize,
    sample_rate: u32,
) -> Option<f32> {
    if audio_samples == 0 || sample_rate == 0 {
        return None;
    }
    let words = text.split_whitespace().count();
    if words < WORD_RATE_MIN_WORDS {
        return None;
    }
    let duration_s = audio_samples as f32 / sample_rate as f32;
    if duration_s <= 0.0 {
        return None;
    }
    Some(words as f32 / duration_s)
}

pub(crate) fn emit_vad_warning(event_sink: &Arc<dyn EventSink>, session: &mut SpeechSession) {
    if let Some(stats) = session.take_vad_error_stats() {
        event_sink.on_event(&EngineEvent::Warning {
            code: "vad_degraded".to_string(),
            message: format!(
                "VAD degraded in current batch: predict_errors={} unavailable_frames={} (totals: predict_errors={} unavailable_frames={})",
                stats.predict_errors,
                stats.unavailable_frames,
                stats.total_predict_errors,
                stats.total_unavailable_frames
            ),
        });
    }
}

pub(crate) fn should_drop_short_utterance(
    audio_samples: usize,
    sample_rate: u32,
    speech_prob: f32,
) -> bool {
    let duration_s = audio_samples as f32 / sample_rate as f32;
    duration_s < MIN_UTTERANCE_SEC && speech_prob < SHORT_UTTERANCE_LOW_CONFIDENCE
}

/// Categorical speech-ratio gate: use Silero VAD as a binary classifier.
///
/// Computes the fraction of the chunk that Silero classified as speech
/// (prob >= threshold). If the ratio falls below `MIN_SPEECH_RATIO_FOR_INFERENCE`,
/// the chunk is predominantly silence and should not be sent to Whisper
/// (which would hallucinate on it).
///
/// Returns `true` when the chunk should be **dropped** (too little speech).
pub(crate) fn should_drop_silence_chunk(
    audio_samples: usize,
    sample_rate: u32,
    speech_vad_samples: u64,
    is_final: bool,
) -> bool {
    // Never gate final emissions (user explicitly released key / segment closed).
    if is_final {
        return false;
    }
    // Convert audio length to 16kHz domain to match Silero's sample counting.
    let audio_16k =
        (audio_samples as f64 * f64::from(vad::VAD_SAMPLE_RATE) / f64::from(sample_rate)) as u64;
    if audio_16k == 0 {
        return false;
    }
    let speech_ratio = speech_vad_samples as f32 / audio_16k as f32;
    speech_ratio < MIN_SPEECH_RATIO_FOR_INFERENCE
}

pub(crate) fn silero_vad_samples_to_ms(samples: u64) -> u64 {
    samples.saturating_mul(1_000) / u64::from(vad::VAD_SAMPLE_RATE)
}

pub(crate) fn utterance_vad_speech_pct(
    audio_samples: usize,
    sample_rate: u32,
    speech_vad_samples: u64,
) -> Option<f32> {
    if audio_samples == 0 || sample_rate == 0 {
        return None;
    }

    let audio_16k =
        (audio_samples as f64 * f64::from(vad::VAD_SAMPLE_RATE) / f64::from(sample_rate)) as u64;
    if audio_16k == 0 {
        return None;
    }

    Some(((speech_vad_samples as f32 / audio_16k as f32) * 100.0).min(100.0))
}

pub(crate) fn is_hallucination(text: &str, language: Option<&str>) -> bool {
    let lower = text.trim().to_lowercase();
    if SHORT_SPEECH_WHITELIST.iter().any(|w| lower == *w) {
        return false;
    }
    let is_pl = is_polish_language(language);
    if WHISPER_HALLUCINATIONS_COMMON.iter().any(|h| lower == *h)
        || (is_pl && WHISPER_HALLUCINATIONS_PL.iter().any(|h| lower == *h))
    {
        return true;
    }
    if lower.len() < 30
        && (WHISPER_HALLUCINATIONS_COMMON
            .iter()
            .any(|h| lower.contains(h))
            || (is_pl && WHISPER_HALLUCINATIONS_PL.iter().any(|h| lower.contains(h))))
        && lower.split_whitespace().count() <= 4
    {
        return true;
    }
    if is_repetition_hallucination(&lower) {
        return true;
    }
    false
}

/// Minimum word count before the repetition heuristic engages. Short utterances
/// (e.g. "tak tak tak", "no no no") are legitimate and must never be flagged.
const REPETITION_MIN_WORDS: usize = 8;

/// Minimum fraction of words that must belong to repeated occurrences of the
/// dominant token before text is treated as a repetition hallucination.
const REPETITION_DOMINANCE: f32 = 0.6;

/// Cheap auto-repetition detector for longer hallucinations: Whisper sometimes
/// loops a phrase/word many times (e.g. "do widzenia do widzenia do widzenia
/// ..."). Flags text that is both long enough to be implausible as real speech
/// and dominated by a single repeated word. Conservative by design: it stays
/// silent on short legitimate repeats (gated by `REPETITION_MIN_WORDS`) and only
/// fires when one token covers most of the text (`REPETITION_DOMINANCE`).
fn is_repetition_hallucination(lower: &str) -> bool {
    let words: Vec<&str> = lower.split_whitespace().collect();
    if words.len() < REPETITION_MIN_WORDS {
        return false;
    }

    // Unigram dominance: a single word covering most of the text is the cheapest
    // and most reliable loop signal.
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for w in &words {
        *counts.entry(*w).or_insert(0) += 1;
    }
    let max_count = counts.values().copied().max().unwrap_or(0);
    if max_count as f32 / words.len() as f32 >= REPETITION_DOMINANCE {
        return true;
    }

    // Bigram dominance: catches two-word phrases looped (the unigram split
    // between the two words keeps either below the unigram threshold).
    if words.len() >= 2 * REPETITION_MIN_WORDS {
        let mut bigrams: std::collections::HashMap<(&str, &str), usize> =
            std::collections::HashMap::new();
        for pair in words.windows(2) {
            *bigrams.entry((pair[0], pair[1])).or_insert(0) += 1;
        }
        let max_bigram = bigrams.values().copied().max().unwrap_or(0);
        let total_bigrams = words.len() - 1;
        // Each repeated bigram covers ~2 words; compare covered words to total.
        if (max_bigram * 2) as f32 / words.len() as f32 >= REPETITION_DOMINANCE
            && max_bigram >= total_bigrams / 2
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hallucination_existing_matches_preserved() {
        // Exact-match list entries still flagged.
        assert!(is_hallucination("Thank you", None));
        assert!(is_hallucination("  Dziękuję za uwagę  ", Some("pl")));
        assert!(is_hallucination(
            "Napisy stworzone przez społeczność",
            Some("pl")
        ));
        // Whitelist + normal speech still pass.
        assert!(!is_hallucination("Tak", Some("pl")));
        assert!(!is_hallucination("This is a normal sentence.", Some("en")));
    }

    #[test]
    fn hallucination_repetition_detected() {
        // Single word looped many times (long enough to be implausible speech).
        assert!(is_hallucination(
            "do do do do do do do do do do",
            Some("pl")
        ));
        // Two-word phrase looped.
        assert!(is_hallucination(
            "do widzenia do widzenia do widzenia do widzenia do widzenia do widzenia do widzenia do widzenia",
            Some("pl")
        ));
    }

    #[test]
    fn hallucination_legit_repeat() {
        // Short legitimate repeats must NOT be flagged (below min-words gate).
        assert!(!is_hallucination("tak tak tak", Some("pl")));
        assert!(!is_hallucination("no no no", Some("pl")));
        assert!(!is_hallucination("nie nie nie nie", Some("pl")));
        // Longer but varied real speech (no single-word dominance) passes.
        assert!(!is_hallucination(
            "i wtedy poszedłem do sklepu żeby kupić chleb mleko oraz masło",
            Some("pl")
        ));
        // Repetition heuristic helper: empty / short are inert.
        assert!(!is_repetition_hallucination(""));
        assert!(!is_repetition_hallucination("tak tak tak"));
    }
}
