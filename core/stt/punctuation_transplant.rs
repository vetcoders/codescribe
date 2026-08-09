//! Punctuation transplant: Whisper's sentence shape onto Apple's committed words.
//!
//! WHY THIS EXISTS (measured 2026-08-09, frozen parity fixture): Apple live
//! commits ~1 period and 0 commas per ~1050 characters of Polish; Whisper over
//! the same audio produces 9 periods and 25 commas (human reference: 6 / 32).
//! Layered transcription carries the shape the user needs — but Smart mode's
//! append-only doctrine rightly forbids rewriting committed words, so the
//! shape never reached the delivery. This module resolves the standoff:
//! **words stay Apple's, byte for byte; only punctuation and capitalization
//! are adopted from the aligned Whisper transcript.**
//!
//! INVARIANT (pinned by tests): stripping punctuation and case from the output
//! yields exactly the committed input's word sequence. The transplant can
//! never change, drop, or introduce a word — a low-confidence alignment
//! returns `None` and the committed text ships untouched.
//!
//! Scope note (operator correction 2026-08-09): the overlay doctrine forbids
//! truncating text and dropping transcripts — it does NOT forbid correcting
//! committed words; that is the live tail patch's job (Layer 1, core element).
//! This stage keeps words invariant not because correction is forbidden, but
//! because shape is the only deficit it is asked to repair — word-level
//! corrections at stop-time without live backspace feedback would surprise the
//! user, where the live patch corrects in plain sight.

/// Committed/punctuated word-count ceiling — LCS is O(n·m); beyond this the
/// transplant declines rather than stalls the stop path (~4000² comparisons
/// ≈ tens of ms worst case; real sessions sit far below).
const MAX_WORDS: usize = 4000;

/// Minimum fraction of committed words that must align with the punctuated
/// transcript for the shape transfer to be trusted. Below this the two
/// transcripts disagree about what was said, and adopting punctuation from a
/// disagreeing transcript would place sentence breaks at foreign positions.
const MIN_ALIGNED_RATIO: f32 = 0.5;

/// Sentence-relevant trailing marks worth adopting from the punctuated side.
const TRAILING_MARKS: &[char] = &['.', ',', '!', '?', ';', ':', '…'];

/// Outcome of a successful transplant, with the evidence a telemetry line needs.
#[derive(Debug, Clone, PartialEq)]
pub struct TransplantOutcome {
    pub text: String,
    /// Trailing punctuation marks adopted onto committed words.
    pub punctuation_added: usize,
    /// Committed words whose capitalization was lifted to match the shape.
    pub capitalization_changes: usize,
    /// Fraction of committed words aligned against the punctuated transcript.
    pub aligned_ratio: f32,
}

/// One word of the punctuated transcript, split into what we match on and
/// what we may adopt.
struct ShapeToken {
    normalized: String,
    trailing: String,
    starts_uppercase: bool,
}

/// Lowercased, punctuation-stripped key both sides are matched on.
fn normalize_word(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn shape_tokens(text: &str) -> Vec<ShapeToken> {
    text.split_whitespace()
        .map(|word| {
            let trailing: String = word
                .chars()
                .rev()
                .take_while(|c| TRAILING_MARKS.contains(c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            ShapeToken {
                normalized: normalize_word(word),
                trailing,
                starts_uppercase: word.chars().next().is_some_and(char::is_uppercase),
            }
        })
        .collect()
}

/// Standard LCS table over normalized words; returns aligned index pairs
/// (committed_idx, punctuated_idx) in order.
fn align(committed: &[String], punctuated: &[ShapeToken]) -> Vec<(usize, usize)> {
    let n = committed.len();
    let m = punctuated.len();
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if committed[i] == punctuated[j].normalized {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if committed[i] == punctuated[j].normalized {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Adopt punctuation and capitalization from `punctuated` onto `committed`.
///
/// `None` when either side is empty, too large, or the alignment is too weak
/// to trust — the caller then delivers the committed text untouched.
pub fn transplant_punctuation(committed: &str, punctuated: &str) -> Option<TransplantOutcome> {
    let committed_words: Vec<&str> = committed.split_whitespace().collect();
    let punctuated_tokens = shape_tokens(punctuated);
    if committed_words.is_empty()
        || punctuated_tokens.is_empty()
        || committed_words.len() > MAX_WORDS
        || punctuated_tokens.len() > MAX_WORDS
    {
        return None;
    }

    let committed_norm: Vec<String> = committed_words
        .iter()
        .map(|word| normalize_word(word))
        .collect();
    let pairs = align(&committed_norm, &punctuated_tokens);
    let aligned_ratio = pairs.len() as f32 / committed_words.len() as f32;
    if aligned_ratio < MIN_ALIGNED_RATIO {
        return None;
    }

    let mut adopted_trailing: Vec<Option<&str>> = vec![None; committed_words.len()];
    let mut adopted_upper: Vec<bool> = vec![false; committed_words.len()];
    for (ci, pi) in &pairs {
        let token = &punctuated_tokens[*pi];
        if !token.trailing.is_empty() {
            adopted_trailing[*ci] = Some(token.trailing.as_str());
        }
        adopted_upper[*ci] = token.starts_uppercase;
    }

    let mut punctuation_added = 0usize;
    let mut capitalization_changes = 0usize;
    let mut out: Vec<String> = Vec::with_capacity(committed_words.len());
    for (idx, word) in committed_words.iter().enumerate() {
        // Strip the committed word's own trailing marks only when the shape
        // side supplies a replacement — never erase punctuation the live
        // stream already earned.
        let (stem, own_trailing) = match word.rfind(|c: char| !TRAILING_MARKS.contains(&c)) {
            Some(pos) => {
                let cut = pos + word[pos..].chars().next().map_or(1, char::len_utf8);
                (&word[..cut], &word[cut..])
            }
            None => (*word, ""),
        };
        let mut rebuilt = stem.to_string();
        // Capitalization is adopted, never revoked: an already-capitalized
        // committed word (proper noun the user saw live) stays capitalized
        // even where the shape side is lowercase.
        if adopted_upper[idx] && !rebuilt.chars().next().is_some_and(char::is_uppercase) {
            let mut chars = rebuilt.chars();
            if let Some(first) = chars.next() {
                rebuilt = first.to_uppercase().chain(chars).collect();
                capitalization_changes += 1;
            }
        }
        match adopted_trailing[idx] {
            Some(trailing) => {
                if own_trailing != trailing {
                    punctuation_added += trailing.chars().count();
                }
                rebuilt.push_str(trailing);
            }
            None => rebuilt.push_str(own_trailing),
        }
        out.push(rebuilt);
    }

    Some(TransplantOutcome {
        text: out.join(" "),
        punctuation_added,
        capitalization_changes,
        aligned_ratio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMITTED: &str =
        "dobra zróbmy tak najpierw sprawdź ten plik potem zobacz testy i nie rozwal scope";
    const PUNCTUATED: &str =
        "Dobra, zróbmy tak: najpierw sprawdź ten plik, potem zobacz testy i nie rozwal scope.";

    #[test]
    fn adopts_punctuation_and_capitalization_from_aligned_shape() {
        let outcome = transplant_punctuation(COMMITTED, PUNCTUATED).expect("aligned transcripts");
        assert_eq!(
            outcome.text,
            "Dobra, zróbmy tak: najpierw sprawdź ten plik, potem zobacz testy i nie rozwal scope."
        );
        assert!(outcome.punctuation_added >= 4);
        assert!(outcome.capitalization_changes >= 1);
        assert!(outcome.aligned_ratio > 0.95);
    }

    #[test]
    fn word_sequence_is_invariant_under_transplant() {
        let outcome = transplant_punctuation(COMMITTED, PUNCTUATED).unwrap();
        let strip = |s: &str| s.split_whitespace().map(normalize_word).collect::<Vec<_>>();
        assert_eq!(strip(&outcome.text), strip(COMMITTED));
    }

    #[test]
    fn whisper_extra_words_never_enter_the_output() {
        // The punctuated side heard an extra clause — its words must not leak.
        let punctuated = "Dobra, zróbmy tak — mówię poważnie: najpierw sprawdź ten plik, potem zobacz testy i nie rozwal scope.";
        let outcome = transplant_punctuation(COMMITTED, punctuated).expect("still aligned");
        assert!(!outcome.text.contains("poważnie"));
        let strip = |s: &str| s.split_whitespace().map(normalize_word).collect::<Vec<_>>();
        assert_eq!(strip(&outcome.text), strip(COMMITTED));
    }

    #[test]
    fn declines_on_disagreeing_transcripts() {
        assert_eq!(
            transplant_punctuation(
                COMMITTED,
                "Zupełnie inna rozmowa o zupełnie innych sprawach."
            ),
            None
        );
    }

    #[test]
    fn keeps_committed_punctuation_where_shape_offers_none() {
        let committed = "spotkanie w środę. przenieśmy resztę na piątek";
        let punctuated = "spotkanie w środę przenieśmy resztę na piątek.";
        let outcome = transplant_punctuation(committed, punctuated).unwrap();
        // "środę." keeps its earned period; "piątek." adopts the new one.
        assert!(outcome.text.contains("środę."));
        assert!(outcome.text.ends_with("piątek."));
    }

    #[test]
    fn never_downgrades_committed_capitalization() {
        let committed = "wyślij to do Moniki jeszcze dziś";
        let punctuated = "wyślij to do moniki jeszcze dziś.";
        let outcome = transplant_punctuation(committed, punctuated).unwrap();
        assert!(outcome.text.contains("Moniki"));
    }

    #[test]
    fn empty_sides_decline() {
        assert_eq!(transplant_punctuation("", PUNCTUATED), None);
        assert_eq!(transplant_punctuation(COMMITTED, "   "), None);
    }
}
