//! Semantic guard for AI formatting — does the formatted text still mean what
//! the speaker said?
//!
//! `ActionQualityProbe` counts characters, so an LLM that rewrote the meaning
//! while keeping similar length sailed through the quality gate. This module
//! gives the gate a meaning axis: MiniLM cosine between the pre-formatting
//! transcript and the formatted output.
//!
//! CALIBRATED, NOT INTUITED (`examples/semantic_guard_calibration.rs`, run
//! 2026-08-09 on the embedded MiniLM):
//!
//! | class                          | cosine |
//! |--------------------------------|--------|
//! | punctuation/caps/fillers only  | 0.950–0.976 |
//! | negation dropped               | 0.726 |
//! | hallucinated content added     | 0.776 |
//! | topic replaced                 | 0.253 |
//! | **instruction inverted**       | **0.945 — NOT separable** |
//!
//! The floor of 0.86 sits midway between the meaning-kept minimum (0.950) and
//! the highest *catchable* betrayal (0.776): zero false alarms on the kept
//! class, three of four betrayal classes caught.
//!
//! KNOWN BLINDSPOT, stated rather than implied: an inversion that reuses the
//! same vocabulary ("usuń stary backup po zweryfikowaniu nowego" → "usuń nowy
//! backup po zweryfikowaniu starego") scores like clean formatting. Sentence
//! embeddings measure lexical-semantic overlap, not argument structure. This
//! guard reduces the risk of "meaning kept" being false; it does not prove the
//! meaning was kept.
//!
//! Fail-open by design: no embedder, short text, or a degenerate embedding
//! yields `None` — the guard never blocks delivery on its own malfunction
//! (same posture as the SemanticGate dedup in stream_postprocess).

/// Below the floor, formatting is treated as having diverged from the spoken
/// meaning and the delivery requires an explicit commit.
pub const FORMATTING_SEMANTIC_FLOOR: f32 = 0.86;

/// Below this many characters (either side) the cosine is noise — a two-word
/// utterance and its formatted twin can score anywhere.
const MIN_CHARS: usize = 40;

/// Cosine between the raw transcript and its AI-formatted output, when it can
/// be measured meaningfully. `None` = no verdict (fail-open), never "diverged".
pub fn formatting_semantic_cosine(raw: &str, formatted: &str) -> Option<f32> {
    let raw = raw.trim();
    let formatted = formatted.trim();
    if raw.chars().count() < MIN_CHARS || formatted.chars().count() < MIN_CHARS {
        return None;
    }
    // Identical after whitespace collapse: nothing to judge, and skipping the
    // embedder keeps the no-op path free.
    if raw.split_whitespace().eq(formatted.split_whitespace()) {
        return None;
    }
    let embeddings = crate::embedder::embed_batch(&[raw, formatted]).ok()?;
    let [a, b] = embeddings.as_slice() else {
        return None;
    };
    let cosine = crate::embedder::similarity(a, b);
    // The embedder self-test guards NaN at load, but a guard that can emit a
    // non-finite verdict into the quality gate is a guard with a second bug.
    cosine.is_finite().then_some(cosine)
}

/// Convenience verdict against the calibrated floor.
pub fn formatting_meaning_diverged(cosine: f32) -> bool {
    cosine < FORMATTING_SEMANTIC_FLOOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_texts_yield_no_verdict() {
        assert_eq!(formatting_semantic_cosine("krótko", "Krótko."), None);
    }

    #[test]
    fn whitespace_identical_texts_yield_no_verdict() {
        let raw = "dobra zróbmy tak najpierw sprawdź ten plik potem zobacz testy i nie rozwal";
        let same = format!("  {}  ", raw.replace(' ', "  "));
        assert_eq!(formatting_semantic_cosine(raw, &same), None);
    }

    #[test]
    fn floor_classifies_calibrated_populations() {
        // Measured populations from the calibration example: the floor must
        // accept every meaning-kept score and reject every catchable betrayal.
        for kept in [0.950_f32, 0.971, 0.976] {
            assert!(!formatting_meaning_diverged(kept));
        }
        for betrayed in [0.253_f32, 0.726, 0.776] {
            assert!(formatting_meaning_diverged(betrayed));
        }
        // The documented blindspot, pinned so nobody "fixes" the floor upward
        // past the meaning-kept minimum while chasing it.
        assert!(!formatting_meaning_diverged(0.945));
    }
}
