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
//! The one word rule is the product name: every spoken form of it becomes
//! "Codescribe" (see [`brand_spelling`]).
//!
//! Every rule is idempotent: applying the pass twice yields the same string, so
//! a re-delivered or re-formatted transcript never drifts.

/// The product name, spelled one way whichever engine heard it.
const BRAND: &str = "Codescribe";

/// How engines hear the product name: Whisper writes "CodeScript" or
/// "code script", cloud engines "Code Scribe", Polish decoders "kod skrajb".
/// The brand is not a user preference, so a missing, reset or poisoned
/// custom dictionary must not let it ship misspelt; the rule lives in this
/// always-on pass that every committed span goes through.
static BRAND_HEARINGS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:code[ -]?scri(?:be|pt)|code[ -]?skrajb|kod[ -]?(?:scribe|skrybe|skraj ?b)|kodeskrajb|kołdskrajb)\b",
    )
    .expect("brand hearing pattern")
});

/// Byte ranges that hold data rather than speech: fenced code, inline code
/// and straight-quoted literals, split the way CommonMark 0.31.2 splits them.
/// A fence is a run of three or more backticks or tildes opening a line (at
/// most three spaces before it); it closes at the first later line holding at
/// most three spaces, a run of the same character at least as long and
/// nothing else, and an unclosed fence protects the rest. Any other run of n
/// backticks, even three mid-line, is an inline span that closes at the next
/// run of exactly n, so ``a `b` c`` is one span; an unclosed inline run
/// protects nothing. Tildes outside a fence are text. A quote closes only when an even number of backslashes
/// precedes it, so JSON escapes stay inside their string. An apostrophe
/// inside a word (`Codescribe'a`, `don't`) does not open a literal.
fn literal_regions(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut regions = Vec::new();
    let mut index = 0;
    while let Some(ch) = text[index..].chars().next() {
        let (end, skip) = match ch {
            '`' | '~' => {
                let marker = ch as u8;
                let run = marker_run(&text[index..], marker);
                let end = if run >= 3 && opens_line(text, index) {
                    Some(closing_fence(text, index, marker, run).unwrap_or(text.len()))
                } else if marker == b'`' {
                    closing_backtick_run(text, index + run, run)
                } else {
                    None
                };
                (end, run)
            }
            '"' if unescaped(text, index) => (closing_quote(text, index + 1, '"'), 1),
            '\'' if unescaped(text, index)
                && text[..index]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !before.is_alphanumeric()) =>
            {
                (closing_quote(text, index + 1, '\''), 1)
            }
            _ => (None, ch.len_utf8()),
        };
        match end {
            Some(end) => {
                regions.push(index..end);
                index = end;
            }
            None => index += skip,
        }
    }
    regions
}

/// Length of the run of `marker` bytes that starts `text`.
fn marker_run(text: &str, marker: u8) -> usize {
    text.bytes().take_while(|&byte| byte == marker).count()
}

/// True when only up to three spaces separate `at` from the line start.
fn opens_line(text: &str, at: usize) -> bool {
    let line_start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let indent = &text[line_start..at];
    indent.len() <= 3 && indent.bytes().all(|byte| byte == b' ')
}

/// End of the line that closes the fence opened by `run` `marker` bytes at
/// `at`: at most three spaces, at least `run` of the same marker, then only
/// whitespace.
fn closing_fence(text: &str, at: usize, marker: u8, run: usize) -> Option<usize> {
    let mut line_start = at + text[at..].find('\n')? + 1;
    loop {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |newline| line_start + newline);
        let line = &text[line_start..line_end];
        let fence = line.trim_start_matches(' ');
        let length = marker_run(fence, marker);
        if line.len() - fence.len() <= 3 && length >= run && fence[length..].trim().is_empty() {
            return Some(line_end);
        }
        if line_end == text.len() {
            return None;
        }
        line_start = line_end + 1;
    }
}

/// End of the first run of exactly `run` backticks at or after `from`.
fn closing_backtick_run(text: &str, from: usize, run: usize) -> Option<usize> {
    let mut at = from;
    while let Some(offset) = text[at..].find('`') {
        let start = at + offset;
        let length = marker_run(&text[start..], b'`');
        if length == run {
            return Some(start + run);
        }
        at = start + length;
    }
    None
}

/// True when the byte at `at` is not escaped: an even number of
/// backslashes precedes it.
fn unescaped(text: &str, at: usize) -> bool {
    text[..at]
        .bytes()
        .rev()
        .take_while(|&byte| byte == b'\\')
        .count()
        % 2
        == 0
}

/// End of the literal opened by `quote` before `from`: the next unescaped
/// `quote`. A single quote also needs a non-alphanumeric character after it,
/// so `'CodeScript'a'` closes at the last quote, not inside a word.
fn closing_quote(text: &str, from: usize, quote: char) -> Option<usize> {
    text[from..]
        .match_indices(quote)
        .map(|(offset, _)| from + offset)
        .find(|&close| {
            unescaped(text, close)
                && (quote == '"'
                    || text[close + 1..]
                        .chars()
                        .next()
                        .is_none_or(|after| !after.is_alphanumeric()))
        })
        .map(|close| close + 1)
}

/// Rewrite spoken forms of the brand to [`BRAND`]. All-lowercase
/// `codescribe` and any form glued to a path, identifier or address stay as
/// written: `~/.codescribe/takes` and `codescribe.transcript.v1` are names,
/// not speech. Code in backticks or fences and straight-quoted literals
/// (`printf '%s' 'CodeScript'`, JSON `"CodeScript"`) are data and stay too;
/// writer's quotes („…”) are prose and are normalised. Lone "code", "script"
/// or "scribe" never match.
///
/// Known gap: the rule sees one span. When the decoder splits the name over
/// two occurrences ("code" | "scribe"), each stays as heard; occurrences are
/// never merged or re-keyed for spelling, because that would change their
/// acoustic identity.
pub fn brand_spelling(text: &str) -> std::borrow::Cow<'_, str> {
    let mut rewritten = String::new();
    let mut copied = 0;
    let mut literals = None;
    for found in BRAND_HEARINGS.find_iter(text) {
        let heard = found.as_str();
        let before = text[..found.start()].chars().next_back();
        let mut after = text[found.end()..].chars();
        let next = after.next();
        let glued = matches!(before, Some('.' | '/' | '\\' | '_' | '-' | '~' | '@'))
            || matches!(next, Some('/' | '\\' | '_' | '@'))
            || (matches!(next, Some('.' | '-' | ':'))
                && after.next().is_some_and(char::is_alphanumeric));
        if glued || heard == BRAND || heard == "codescribe" {
            continue;
        }
        if literals
            .get_or_insert_with(|| literal_regions(text))
            .iter()
            .any(|region| region.start <= found.start() && found.end() <= region.end)
        {
            continue;
        }
        rewritten.push_str(&text[copied..found.start()]);
        rewritten.push_str(BRAND);
        copied = found.end();
    }
    if copied == 0 {
        return std::borrow::Cow::Borrowed(text);
    }
    rewritten.push_str(&text[copied..]);
    std::borrow::Cow::Owned(rewritten)
}

/// Punctuation that closes a clause and may therefore be doubled by a seam.
const COLLAPSIBLE_PUNCT: [char; 6] = ['.', '!', '?', ',', ';', ':'];

/// One deadline from scheduling through admission, including worker queue time.
/// A worker owns candidates only; timing out drops its return channel and
/// cannot grant it authority to publish a late result.
#[derive(Clone)]
pub struct TickBudget {
    scheduled: std::time::Duration,
    wall_started: std::time::Instant,
    clock: std::sync::Arc<dyn Fn() -> std::time::Duration + Send + Sync>,
}

impl Default for TickBudget {
    fn default() -> Self {
        let origin = std::time::Instant::now();
        Self::schedule(std::sync::Arc::new(move || origin.elapsed()))
    }
}

impl TickBudget {
    pub const LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

    pub fn schedule(clock: std::sync::Arc<dyn Fn() -> std::time::Duration + Send + Sync>) -> Self {
        Self {
            scheduled: clock(),
            wall_started: std::time::Instant::now(),
            clock,
        }
    }

    pub fn elapsed(&self) -> std::time::Duration {
        (self.clock)()
            .saturating_sub(self.scheduled)
            .max(self.wall_started.elapsed())
    }

    pub fn expired(&self) -> bool {
        self.elapsed() >= Self::LIMIT
    }

    /// Wait only for the remaining budget. The closure must have no access to
    /// live authority; its output still needs deadline and source admission.
    pub fn prepare<T: Send + 'static>(
        &self,
        work: impl FnOnce(Self) -> T + Send + 'static,
    ) -> Option<T> {
        let remaining = Self::LIMIT.checked_sub(self.elapsed())?;
        if remaining.is_zero() {
            return None;
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let budget = self.clone();
        std::thread::Builder::new()
            .name("light-plus-tick".into())
            .spawn(move || {
                if !budget.expired() {
                    let result = work(budget.clone());
                    if !budget.expired() {
                        let _ = sender.send(result);
                    }
                }
            })
            .ok()?;
        let remaining = Self::LIMIT.checked_sub(self.elapsed())?;
        let result = receiver.recv_timeout(remaining).ok()?;
        (!self.expired()).then_some(result)
    }
}

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

    let branded = brand_spelling(trimmed);
    let tightened = tighten_punctuation(&branded);
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
    fn every_heard_form_of_the_brand_becomes_codescribe() {
        for heard in [
            "CodeScript",
            "Codescript",
            "codescript",
            "code script",
            "Code Script",
            "code-script",
            "CodeScribe",
            "Code Scribe",
            "code scribe",
            "kod scribe",
            "kod-scribe",
            "kod skrajb",
            "kod skraj b",
            "kod skrybe",
            "kodeskrajb",
            "kołdskrajb",
            "code skrajb",
            "Codescribe",
        ] {
            assert_eq!(
                brand_spelling(&format!("pracuję nad {heard} dziś")),
                "pracuję nad Codescribe dziś",
                "{heard}"
            );
        }
        assert_eq!(brand_spelling("CodeScript."), "Codescribe.");
        assert_eq!(brand_spelling("CodeScript'a"), "Codescribe'a");
        assert_eq!(
            apply("otwórz CodeScript i code scribe"),
            "Otwórz Codescribe i Codescribe."
        );
    }

    #[test]
    fn script_code_and_names_stay_as_spoken() {
        for kept in [
            "to jest Python script",
            "TypeScript i JavaScript",
            "ten skrypt i ten script",
            "kod skryptu jest długi",
            "napisz code, potem scribe",
            "zajrzyj do ~/.codescribe/takes",
            "schema codescribe.transcript.v1",
            "plik codescribe-ffi i code_script.py",
            "codescribe z małej litery",
        ] {
            assert_eq!(brand_spelling(kept), kept);
        }
    }

    #[test]
    fn code_and_quoted_literals_keep_their_bytes() {
        for kept in [
            "printf '%s' 'CodeScript'",
            r#"{"name": "CodeScript"}"#,
            "uruchom `code script --help`",
            "```\nCodeScript = 1\n```",
            "```sh\necho code scribe",
            r#"{"text": "call \"CodeScript help\" now"}"#,
            "``CodeScript help``",
            "``a `CodeScript` b``",
            "```\nx ``` y\nCodeScript help\n```",
            "a ```CodeScript``` b",
        ] {
            assert_eq!(brand_spelling(kept), kept);
        }
    }

    #[test]
    fn prose_beside_code_still_gets_the_brand() {
        assert_eq!(
            brand_spelling("CodeScript uruchamia `code script` i „Code Scribe”"),
            "Codescribe uruchamia `code script` i „Codescribe”"
        );
        assert_eq!(
            brand_spelling("```\nCodeScript\n```\npo bloku CodeScript działa"),
            "```\nCodeScript\n```\npo bloku Codescribe działa"
        );
        assert_eq!(brand_spelling("`a` CodeScript `b`"), "`a` Codescribe `b`");
        assert_eq!(
            brand_spelling("```\nCodeScript\n````\npo CodeScript"),
            "```\nCodeScript\n````\npo Codescribe"
        );
        assert_eq!(
            brand_spelling("```\nCodeScript\n   ```\npo CodeScript"),
            "```\nCodeScript\n   ```\npo Codescribe"
        );
        assert_eq!(
            brand_spelling("~~~sh\nCodeScript --help\n~~~\npo CodeScript"),
            "~~~sh\nCodeScript --help\n~~~\npo Codescribe"
        );
        // A backtick run never closes a tilde fence, and tildes mid-line are text.
        assert_eq!(
            brand_spelling("~~~\nCodeScript\n```\nCodeScript"),
            "~~~\nCodeScript\n```\nCodeScript"
        );
        assert_eq!(brand_spelling("a ~~~ CodeScript"), "a ~~~ Codescribe");
        assert_eq!(
            brand_spelling(r#""a\\" CodeScript "b""#),
            r#""a\\" Codescribe "b""#
        );
        assert_eq!(
            brand_spelling("`` CodeScript i code script"),
            "`` Codescribe i Codescribe"
        );
        assert_eq!(
            brand_spelling("nie don't i CodeScript'a"),
            "nie don't i Codescribe'a"
        );
    }

    /// Known gap, pinned: the name split over two occurrences stays as heard.
    #[test]
    fn brand_split_across_occurrences_stays_as_heard() {
        assert_eq!(
            apply_live_span("pracuję nad code", "scribe dziś", false),
            "scribe dziś"
        );
    }

    #[test]
    fn brand_rule_is_idempotent_through_the_live_span() {
        let once = apply_live_span("", "pracuję nad code script", false);
        assert_eq!(once, "Pracuję nad Codescribe");
        assert_eq!(apply_live_span("", &once, false), once);
    }

    #[test]
    fn tick_budget_counts_queue_time_and_refuses_late_candidates() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        let time = Arc::new(AtomicU64::new(0));
        let clock_time = time.clone();
        let clock: Arc<dyn Fn() -> std::time::Duration + Send + Sync> =
            Arc::new(move || std::time::Duration::from_millis(clock_time.load(Ordering::SeqCst)));
        let queued = TickBudget::schedule(clock.clone());
        time.store(2_001, Ordering::SeqCst);
        assert!(
            queued
                .prepare(|_| panic!("expired queued job must not start"))
                .is_none()
        );
        time.store(0, Ordering::SeqCst);
        let late = TickBudget::schedule(clock.clone());
        let worker_time = time.clone();
        assert!(
            late.prepare(move |_| {
                worker_time.store(2_001, Ordering::SeqCst);
                "late candidate"
            })
            .is_none()
        );
        time.store(0, Ordering::SeqCst);
        let timely = TickBudget::schedule(clock);
        assert_eq!(
            timely.prepare(|_| apply("Iwo yyy [laugh]")),
            Some("Iwo yyy [laugh].".into())
        );
    }

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
