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
//! the exact class of harm the lexicon's stopword gate was added to stop.
//! Hesitation sounds and accepted acoustic event markers are preserved too.
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
/// Order matters: normalize punctuation spacing without removing tokens, then
/// insert conservative clause marks, and capitalise last so the pass sees
/// settled sentence boundaries.
pub fn apply(text: &str) -> String {
    apply_with_left_context("", text)
}

/// Shape a complete span while honouring the left context already committed.
///
/// Progressive presentation runs Light+ **per span** during capture.
/// Casing at the span's first word must see whether the preceding sealed text
/// ended mid-sentence or on a terminal — otherwise a lexicon-corrected word
/// at a true sentence start stays lowercase, and a continuation after a
/// comma wrongly capitalises.
///
/// Returns only the shaped span (not the left context concatenated). When
/// `left_context` is empty this is identical to [`apply`].
pub fn apply_with_left_context(left_context: &str, span: &str) -> String {
    let mut shaped = apply_live_span(left_context, span, false);
    if !shaped.is_empty() && !ends_with_terminal_punctuation(&shaped) {
        shaped.push('.');
    }
    shaped
}

/// Shape a live occurrence. A period belongs to the following PCM boundary,
/// so an open last occurrence never invents a sentence end during capture.
pub fn apply_live_span(left_context: &str, span: &str, sentence_break_before: bool) -> String {
    let trimmed = span.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let tightened = tighten_punctuation(trimmed);
    let punctuated = place_polish_commas(&tightened);
    let tightened = punctuated.trim();
    if tightened.is_empty() {
        return String::new();
    }

    // Open-sentence detection from the left neighbour: a terminal (or empty
    // left) means this span starts a sentence and must capitalise.
    let at_sentence_start = sentence_break_before || left_ends_sentence(left_context);
    let mut shaped = capitalize_span(tightened, at_sentence_start);
    if sentence_break_before && !left_context.is_empty() && !left_ends_sentence(left_context) {
        shaped.insert_str(0, ". ");
    }
    shaped
}

/// True when `left` is empty or its last non-whitespace character is a
/// sentence terminal — so the next span opens a new sentence.
fn left_ends_sentence(left: &str) -> bool {
    let trimmed = left.trim_end();
    if trimmed.is_empty() {
        return true;
    }
    matches!(trimmed.chars().last(), Some('.' | '!' | '?' | '…'))
}

/// Capitalise the span's first letter only when it opens a sentence; still
/// capitalise after any terminal that appears *inside* the span.
fn capitalize_span(text: &str, open_at_start: bool) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    let mut at_sentence_start = open_at_start;
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

/// Place conservative Polish clause commas. This only inserts punctuation;
/// occurrence labels and their word order remain untouched.
fn place_polish_commas(text: &str) -> String {
    const CLAUSE_WORDS: &[&str] = &[
        "że",
        "iż",
        "żeby",
        "aby",
        "bo",
        "ponieważ",
        "gdyż",
        "który",
        "która",
        "które",
        "którego",
        "której",
        "którym",
        "którą",
        "których",
        "którymi",
        "ale",
        "lecz",
        "więc",
        "czyli",
        "gdy",
        "kiedy",
        "jeśli",
        "jeżeli",
        "chociaż",
        "choć",
        "zanim",
        "dopóki",
        "gdyby",
        "jakby",
    ];
    const COMPOUNDS: &[&[&str]] = &[
        &["mimo", "że"],
        &["chyba", "że"],
        &["zwłaszcza", "że"],
        &["tak", "że"],
        &["tylko", "że"],
        &["podczas", "gdy"],
        &["dlatego", "że"],
        &["po", "to", "żeby"],
        &["po", "to", "aby"],
    ];
    const NO_COMMA_AFTER: &[&str] = &["i", "oraz", "lub", "albo", "ani", "czy", "a", "no"];

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let words: Vec<String> = tokens
        .iter()
        .map(|token| {
            token
                .trim_matches(|ch: char| !ch.is_alphabetic())
                .to_lowercase()
        })
        .collect();
    let mut insert = vec![false; tokens.len()];
    let mut inner = vec![false; tokens.len()];
    for index in 0..tokens.len() {
        if let Some(compound) = COMPOUNDS.iter().find(|compound| {
            words[index..].starts_with(
                &compound
                    .iter()
                    .map(|word| (*word).to_string())
                    .collect::<Vec<_>>(),
            )
        }) {
            insert[index] = true;
            for offset in 1..compound.len() {
                inner[index + offset] = true;
            }
        } else if CLAUSE_WORDS.contains(&words[index].as_str()) && !inner[index] {
            insert[index] = true;
        }
    }
    let mut out = String::with_capacity(text.len() + tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            if insert[index]
                && !NO_COMMA_AFTER.contains(&words[index - 1].as_str())
                && !tokens[index - 1].ends_with([',', ';', ':', '.', '!', '?'])
            {
                out.push(',');
            }
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

/// Does the text already close on a mark that makes a trailing period wrong?
///
/// `:` counts — a list header (`Lista:`) is finished, not a fragment.
fn ends_with_terminal_punctuation(text: &str) -> bool {
    matches!(text.chars().last(), Some('.' | '!' | '?' | '…' | ':'))
}

/// Pins the two properties the pass exists for — sentence shape on an
/// unpunctuated stream, and idempotence — plus the "never delete a user's word"
/// rule that separates this from the Python original.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_clauses_gain_commas_without_losing_words() {
        let source = "to jest fajne bo VNC jak stosuję to zazwyczaj nie potrzebuję wiedzieć że program który działa jest otwarty";
        let shaped = apply(source);
        assert_eq!(
            shaped,
            "To jest fajne, bo VNC jak stosuję to zazwyczaj nie potrzebuję wiedzieć, że program, który działa jest otwarty."
        );
        assert_eq!(apply(&shaped), shaped);
        let words = |text: &str| {
            text.split_whitespace()
                .map(|word| {
                    word.trim_matches(|ch: char| ch.is_ascii_punctuation())
                        .to_lowercase()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(words(source), words(&shaped));
    }

    #[test]
    fn compound_conjunctions_are_not_split() {
        assert_eq!(
            apply("robię to mimo że pada i że wieje"),
            "Robię to, mimo że pada i że wieje."
        );
        assert_eq!(
            apply("czekam podczas gdy działa"),
            "Czekam, podczas gdy działa."
        );
    }

    #[test]
    fn live_span_waits_for_pcm_boundary_before_adding_a_period() {
        assert_eq!(
            apply_live_span("", "pierwsze słowa", false),
            "Pierwsze słowa"
        );
        assert_eq!(
            apply_live_span("Pierwsze słowa", "drugie słowa", false),
            "drugie słowa"
        );
        assert_eq!(
            apply_live_span("Pierwsze słowa drugie słowa", "trzecie słowa", true),
            ". Trzecie słowa"
        );
    }

    /// Unpunctuated stream gains a capital start and a closing period.
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

    /// Left context that ends on a terminal opens a new sentence on the span.
    #[test]
    fn apply_with_left_context_capitalises_after_neighbour_terminal() {
        let shaped = apply_with_left_context("Koniec poprzedniego.", "docker w produkcji");
        let first = shaped.chars().find(|c| c.is_alphabetic()).expect("alpha");
        assert!(
            first.is_uppercase(),
            "must capitalise after left terminal: {shaped}"
        );
    }

    /// Left context mid-sentence must NOT capitalise the span's first word.
    #[test]
    fn apply_with_left_context_continues_mid_sentence() {
        let shaped = apply_with_left_context("Zaczynamy od", "drugiego słowa");
        let first = shaped.chars().find(|c| c.is_alphabetic()).expect("alpha");
        assert!(
            first.is_lowercase(),
            "mid-sentence continuation stays lower: {shaped}"
        );
    }

    /// Applying the pass twice yields the same string (re-delivery safe).
    #[test]
    fn is_idempotent() {
        let once = apply("to to jest  test ,bez kropki");
        let twice = apply(&once);
        assert_eq!(once, twice, "a re-delivered transcript must not drift");
    }

    /// Capitalisation runs after every terminal mark, not only at the start.
    #[test]
    fn capitalizes_every_sentence_not_just_the_first() {
        assert_eq!(
            apply("pierwsze zdanie. drugie zdanie! trzecie zdanie?"),
            "Pierwsze zdanie. Drugie zdanie! Trzecie zdanie?"
        );
    }

    /// Collapses punctuation seams; never deletes a word.
    ///
    /// The repeated-word rule this pass used to carry is gone: it decided by
    /// content alone, so it could not tell a concatenation artifact from an
    /// operator saying the same word twice, and it always chose deletion.
    #[test]
    fn collapses_punctuation_seams_without_touching_words() {
        assert_eq!(apply("koniec.. naprawdę??"), "Koniec. Naprawdę?");
        assert_eq!(apply("słowo , potem"), "Słowo, potem.");
    }

    /// The conservation law, at the layer that used to break it hardest: five
    /// spoken occurrences of one name stay five tokens.
    #[test]
    fn intentional_repetition_is_never_deleted_by_content() {
        assert_eq!(apply("Iwo Iwo Iwo Iwo Iwo"), "Iwo Iwo Iwo Iwo Iwo.");
        assert_eq!(apply("to to jest jest test"), "To to jest jest test.");
        // Still idempotent: shaping the shaped text changes nothing.
        let once = apply("Iwo Iwo Iwo Iwo Iwo");
        assert_eq!(apply(&once), once);
    }

    /// Raw keeps the spoken performance, including hesitations and events.
    #[test]
    fn preserves_hesitations_and_acoustic_events() {
        assert_eq!(
            apply("yyy no i eee [śmiech] koniec"),
            "Yyy no i eee [śmiech] koniec."
        );
        // Content words that the Python original deleted must survive.
        for kept in ["tak", "no", "właśnie", "jakby"] {
            let shaped = apply(&format!("{kept} zgadzam się"));
            assert!(
                shaped.to_lowercase().contains(kept),
                "{kept} is a word, not a filler: {shaped}"
            );
        }
    }

    /// Only raises case at sentence starts — acronyms and proper nouns stay.
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

    /// Already-shaped text is stable; only missing finals gain a period.
    #[test]
    fn leaves_already_shaped_text_alone_except_for_the_final_stop() {
        assert_eq!(apply("Gotowe zdanie."), "Gotowe zdanie.");
        assert_eq!(apply("Pytanie?"), "Pytanie?");
        assert_eq!(apply("Lista:"), "Lista:");
    }

    /// Empty and whitespace-only inputs stay empty strings.
    #[test]
    fn empty_and_whitespace_stay_empty() {
        assert_eq!(apply(""), "");
        assert_eq!(apply("   \n  "), "");
    }

    /// Hesitation-only spans have the same shaping contract as other speech.
    #[test]
    fn hesitation_only_spans_survive_and_whitespace_stays_empty() {
        for hesitation in ["yyy", "eee", "hmm"] {
            assert_eq!(
                apply_with_left_context("Zdanie przed.", hesitation),
                format!("{}.", capitalize_span(hesitation, true)),
                "a hesitation is part of Raw"
            );
        }
        assert_eq!(apply_with_left_context("Zdanie przed.", "\n"), "");
    }

    /// Left context that closes on a comma is an unfinished clause: the span
    /// continues it and must not rise to a capital.
    #[test]
    fn a_span_after_an_unclosed_clause_stays_lowercase() {
        let shaped = apply_with_left_context("Zaczynamy od tego,", "że to jest ciag dalszy");
        assert!(
            shaped.starts_with("że"),
            "a continuation after a comma stays lowercase: {shaped}"
        );
        assert!(shaped.ends_with('.'), "the span still closes: {shaped}");
    }

    /// The incremental path is idempotent too: re-shaping an already shaped
    /// span against the same left context yields the same bytes, so a repeated
    /// seal observation cannot make the document drift.
    #[test]
    fn reshaping_a_shaped_span_with_the_same_left_context_is_stable() {
        let left = "Pierwsze zdanie.";
        let once = apply_with_left_context(left, "drugie zdanie bez kropki");
        assert_eq!(once, "Drugie zdanie bez kropki.");
        assert_eq!(apply_with_left_context(left, &once), once);
    }

    /// Spans shaped one at a time, as their occurrences close, join into the
    /// same readable document a single whole-transcript pass would produce.
    /// This is the property the live Light+ floor is built on.
    #[test]
    fn spans_shaped_one_by_one_join_into_a_readable_document() {
        let spoken = ["to jest pierwsze zdanie", "a to jest drugie"];
        let mut document = String::new();
        for span in spoken {
            let shaped = apply_with_left_context(&document, span);
            if !document.is_empty() {
                document.push(' ');
            }
            document.push_str(&shaped);
        }
        assert_eq!(document, "To jest pierwsze zdanie. A to jest drugie.");
        // And the whole-document pass leaves that result alone, so the terminal
        // Light+ gate mints nothing on top of it.
        assert_eq!(apply(&document), document);
    }
    #[test]
    fn w0_falsifier_light_plus_preserves_yyy_in_raw() {
        let shaped = apply("Iwo yyy wraca");
        assert!(
            shaped.split_whitespace().any(|word| word == "yyy"),
            "Raw lost its hesitation"
        );
    }
}
