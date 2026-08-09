//! Deterministic sentence shaping — the "Light+" layer.
//!
//! Measured 2026-08-09 on the frozen `05_apple-live-parity` fixture: NO Apple
//! on-device path produces Polish punctuation. `DictationTranscriber` emits 1
//! period and 0 commas over ~1050 characters; the SYSTEM dictation reference
//! captured beside the clip scores identically; the human transcript of the same
//! audio has 6 periods and 32 commas. `SpeechTranscriber` has no `pl` catalog at
//! all. Punctuation simply is not on offer from the engine.
//!
//! That left the LLM formatter as the product's ONLY producer of sentences —
//! and when it is unavailable (offline, a dead key, a provider outage) the
//! delivered text collapses to an unbroken word stream. This module is the
//! floor under that: an always-on, zero-network pass that gives the transcript
//! sentence shape before anything optional runs.
//!
//! It descends from VistaScribe's Python `apply_light_plus`, with one rule
//! deliberately NOT ported: that version deleted "filler" content words (`no`,
//! `tak`, `właśnie`, `w sumie`). Those carry meaning in Polish — "tak, zgadzam
//! się" is not the same sentence without its "tak" — and deleting user words is
//! the exact class of harm the lexicon's stopword gate was added to stop. Only
//! non-lexical hesitation sounds are removed here.
//!
//! Every rule is idempotent: applying the pass twice yields the same string, so
//! a re-delivered or re-formatted transcript never drifts.

/// Punctuation that closes a clause and may therefore be doubled by a seam.
const COLLAPSIBLE_PUNCT: [char; 6] = ['.', '!', '?', ',', ';', ':'];

/// Apply the deterministic sentence-shaping pass. Idempotent, no allocations
/// beyond the output, no network, no model.
///
/// Deliberately written as explicit passes rather than regex: Rust's `regex`
/// crate has neither backreferences nor lookaround, so "a word repeated" and
/// "the same punctuation mark twice" cannot be expressed as patterns at all.
///
/// Order matters: token work first (it decides which words survive), then the
/// character pass (punctuation spacing depends on final token adjacency), and
/// capitalisation last so it sees settled sentence boundaries.
pub fn apply(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let joined = collapse_tokens(trimmed);
    if joined.is_empty() {
        return String::new();
    }

    let tightened = tighten_punctuation(&joined);
    let tightened = tightened.trim();
    if tightened.is_empty() {
        return String::new();
    }

    let mut shaped = capitalize_sentences(tightened);
    if !ends_with_terminal_punctuation(&shaped) {
        shaped.push('.');
    }
    shaped
}

/// Drop hesitation sounds and immediately repeated words, and normalise every
/// run of whitespace to a single space.
fn collapse_tokens(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for token in text.split_whitespace() {
        if is_hesitation(token) {
            continue;
        }
        if kept
            .last()
            .is_some_and(|previous| is_same_word(previous, token))
        {
            continue;
        }
        kept.push(token);
    }
    kept.join(" ")
}

/// A non-lexical hesitation: a run of one vowel (`yyy`, `eee`), or an `hm`/`uh`
/// /`um` form, once trailing punctuation is set aside. The doubling requirement
/// is what keeps real words — `y`, `a`, `e`, `o` never reach two characters —
/// out of reach.
fn is_hesitation(token: &str) -> bool {
    let core: String = token
        .chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();
    if core.len() < 2 {
        return false;
    }
    let mut chars = core.chars();
    let first = chars.next().expect("non-empty");
    if matches!(first, 'y' | 'e' | 'a' | 'i' | 'u' | 'o') && core.chars().all(|c| c == first) {
        // yy, eee, aaa … a single repeated vowel is never a Polish word.
        return true;
    }
    matches!(
        core.as_str(),
        "hm" | "hmm" | "hmmm" | "mhm" | "mhmm" | "uh" | "uhm" | "um" | "umm"
    )
}

/// Are these the same word, ignoring case and any punctuation hanging off them?
/// Punctuation-only tokens never count as repeats — `. .` is the character
/// pass's problem, and treating them as words would eat real ellipses.
fn is_same_word(a: &str, b: &str) -> bool {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let (left, right) = (normalize(a), normalize(b));
    !left.is_empty() && left == right
}

/// Collapse repeated punctuation and pull marks back onto the preceding word.
fn tighten_punctuation(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut previous_punct: Option<char> = None;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        let is_collapsible = COLLAPSIBLE_PUNCT.contains(&ch);
        if is_collapsible {
            // `word ,` → `word,` and `..` → `.`
            if previous_punct == Some(ch) {
                continue;
            }
            out.push(ch);
            previous_punct = Some(ch);
            pending_space = false;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        previous_punct = None;
        out.push(ch);
    }
    out
}

fn ends_with_terminal_punctuation(text: &str) -> bool {
    matches!(text.chars().last(), Some('.' | '!' | '?' | '…' | ':'))
}

/// Uppercase the first letter of the string and of every sentence that follows a
/// terminal mark. Only ever raises case — an all-caps acronym or a deliberately
/// capitalised term is left alone, and a lowercase word mid-sentence is not
/// touched, so this can never fight the lexicon's canonical spellings.
fn capitalize_sentences(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    // A sentence opens at the start of the string and after `. ! ? …`.
    let mut at_sentence_start = true;
    for ch in text.chars() {
        if at_sentence_start && ch.is_alphabetic() {
            for upper in ch.to_uppercase() {
                out.push(upper);
            }
            at_sentence_start = false;
            continue;
        }
        out.push(ch);
        if matches!(ch, '.' | '!' | '?' | '…') {
            at_sentence_start = true;
        } else if !ch.is_whitespace() {
            at_sentence_start = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gives_an_unpunctuated_stream_sentence_shape() {
        let apple_like = "no więc wygląda to żenująco jeśli chodzi o obecny kształt";
        let shaped = apply(apple_like);
        assert!(
            shaped.starts_with("No więc"),
            "first letter must rise: {shaped}"
        );
        assert!(shaped.ends_with('.'), "a transcript must close: {shaped}");
    }

    #[test]
    fn is_idempotent() {
        let once = apply("to to jest  test ,bez kropki");
        let twice = apply(&once);
        assert_eq!(once, twice, "a re-delivered transcript must not drift");
    }

    #[test]
    fn capitalizes_every_sentence_not_just_the_first() {
        assert_eq!(
            apply("pierwsze zdanie. drugie zdanie! trzecie zdanie?"),
            "Pierwsze zdanie. Drugie zdanie! Trzecie zdanie?"
        );
    }

    #[test]
    fn collapses_seam_artifacts_without_touching_words() {
        assert_eq!(apply("to to jest jest test"), "To jest test.");
        assert_eq!(apply("koniec.. naprawdę??"), "Koniec. Naprawdę?");
        assert_eq!(apply("słowo , potem"), "Słowo, potem.");
    }

    #[test]
    fn removes_hesitation_sounds_only() {
        assert_eq!(apply("yyy no i eee koniec"), "No i koniec.");
        // Content words that the Python original deleted must survive.
        for kept in ["tak", "no", "właśnie", "jakby"] {
            let shaped = apply(&format!("{kept} zgadzam się"));
            assert!(
                shaped.to_lowercase().contains(kept),
                "{kept} is a word, not a filler: {shaped}"
            );
        }
    }

    #[test]
    fn never_lowercases_existing_capitals() {
        let shaped = apply("mamy API oraz MCP w Codescribe");
        assert!(shaped.contains("API"), "acronyms survive: {shaped}");
        assert!(shaped.contains("MCP"), "acronyms survive: {shaped}");
        assert!(
            shaped.contains("Codescribe"),
            "proper nouns survive: {shaped}"
        );
    }

    #[test]
    fn leaves_already_shaped_text_alone_except_for_the_final_stop() {
        assert_eq!(apply("Gotowe zdanie."), "Gotowe zdanie.");
        assert_eq!(apply("Pytanie?"), "Pytanie?");
        assert_eq!(apply("Lista:"), "Lista:");
    }

    #[test]
    fn empty_and_whitespace_stay_empty() {
        assert_eq!(apply(""), "");
        assert_eq!(apply("   \n  "), "");
    }
}
