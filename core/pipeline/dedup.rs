//! Unified deduplication for the transcription pipeline.
//!
//! Two granularities:
//! - **Chunk overlap** (`dedup_chunk_overlap`): word-level exact+fuzzy dedup at chunk boundaries
//!   (ported from `engine::append_with_overlap_dedup`)
//! - **Suffix overlap** (`strip_suffix_overlap`): character-level suffix/prefix strip between utterances
//!   (ported from `TranscriptionPipeline::strip_overlap`)
//!
//! # Note: batch vs live dedup
//!
//! The **live streaming** path (`pipeline::streaming`) uses these functions.
//! The **batch/file** path (`engine::transcribe_long_streaming`) still uses
//! `engine::append_with_overlap_dedup` — an identical algorithm kept local to
//! the engine module. This is intentional: the batch path is self-contained
//! and does not route through the pipeline.

// ── helpers ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct OverlapParams {
    max_window: usize,
    min_fuzzy_overlap_words: usize,
    fuzzy_error_ratio_denominator: usize,
}

impl OverlapParams {
    #[inline]
    fn bounded_max_window(self) -> usize {
        self.max_window.max(1)
    }

    #[inline]
    fn bounded_min_fuzzy_overlap(self) -> usize {
        self.min_fuzzy_overlap_words.max(1)
    }

    #[inline]
    fn max_fuzzy_errors(self, overlap_words: usize) -> usize {
        (overlap_words / self.fuzzy_error_ratio_denominator.max(1)).max(1)
    }
}

const CHUNK_OVERLAP_PARAMS: OverlapParams = OverlapParams {
    max_window: 30,
    min_fuzzy_overlap_words: 3,
    fuzzy_error_ratio_denominator: 3,
};

const DEFAULT_SUFFIX_OVERLAP_PARAMS: OverlapParams = OverlapParams {
    max_window: 16,
    min_fuzzy_overlap_words: 3,
    fuzzy_error_ratio_denominator: 3,
};

const LIVE_SUFFIX_OVERLAP_PARAMS: OverlapParams = OverlapParams {
    // Live path runs this for every utterance boundary, keep the fuzzy window tighter.
    max_window: 12,
    min_fuzzy_overlap_words: 3,
    fuzzy_error_ratio_denominator: 3,
};

fn normalize_token_for_overlap(token: &str) -> String {
    let mut out = String::new();
    for ch in token.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        }
    }
    if out.is_empty() {
        token.to_lowercase()
    } else {
        out
    }
}

/// Word-level edit distance for short sequences (used by fuzzy overlap).
fn word_edit_distance_bounded(a: &[String], b: &[String], max_dist: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max_dist {
        return None;
    }

    let m = a.len();
    let n = b.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];

    for i in 1..=m {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max_dist {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }

    let dist = prev[n];
    (dist <= max_dist).then_some(dist)
}

/// Find overlap between suffix of `left_words` and prefix of `right_words`.
///
/// Pass 1: exact normalized word match.
/// Pass 2: fuzzy word edit distance for larger windows.
fn detect_word_overlap(left_words: &[&str], right_words: &[&str], params: OverlapParams) -> usize {
    let max_overlap = left_words
        .len()
        .min(right_words.len())
        .min(params.bounded_max_window());
    if max_overlap == 0 {
        return 0;
    }

    let left_slice = &left_words[left_words.len() - max_overlap..];
    let right_slice = &right_words[..max_overlap];

    let left_norm: Vec<String> = left_slice
        .iter()
        .map(|word| normalize_token_for_overlap(word))
        .collect();
    let right_norm: Vec<String> = right_slice
        .iter()
        .map(|word| normalize_token_for_overlap(word))
        .collect();

    // Pass 1: exact match (fast path).
    for k in (1..=max_overlap).rev() {
        if left_norm[max_overlap - k..] == right_norm[..k] {
            return k;
        }
    }

    // Pass 2: fuzzy match.
    let min_fuzzy_overlap = params.bounded_min_fuzzy_overlap();
    if min_fuzzy_overlap <= max_overlap {
        for k in (min_fuzzy_overlap..=max_overlap).rev() {
            let tail = &left_norm[max_overlap - k..];
            let head = &right_norm[..k];
            let max_errors = params.max_fuzzy_errors(k);
            if let Some(dist) = word_edit_distance_bounded(tail, head, max_errors) {
                tracing::debug!(
                    "[FUZZY_DEDUP] matched k={} dist={} max_err={} tail={:?} head={:?}",
                    k,
                    dist,
                    max_errors,
                    &tail[..tail.len().min(5)],
                    &head[..head.len().min(5)]
                );
                return k;
            }
        }
    }

    0
}

// ── public API ───────────────────────────────────────────

/// Append `segment` to `out`, deduplicating overlapping word sequences at the boundary.
///
/// Two-pass approach:
/// 1. Exact match (fast path) — suffix of `out` == prefix of `segment`
/// 2. Fuzzy match (fallback) — allows up to k/3 word-level edits in overlap region.
///    Catches cases where Whisper produces slightly different text for the same audio.
pub fn dedup_chunk_overlap(out: &mut String, segment: &str) {
    let seg = segment.trim();
    if seg.is_empty() {
        return;
    }

    if out.trim().is_empty() {
        out.push_str(seg);
        return;
    }

    let out_trim = out.trim_end();
    let seg_words: Vec<&str> = seg.split_whitespace().collect();
    if seg_words.is_empty() {
        if !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(seg);
        return;
    }

    // Keep only the suffix window needed for overlap checks.
    let max_overlap_window = seg_words
        .len()
        .min(CHUNK_OVERLAP_PARAMS.bounded_max_window());
    let mut out_tail_words: Vec<&str> = out_trim
        .split_whitespace()
        .rev()
        .take(max_overlap_window)
        .collect();
    if out_tail_words.is_empty() {
        if !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(seg);
        return;
    }
    out_tail_words.reverse();

    let overlap = detect_word_overlap(&out_tail_words, &seg_words, CHUNK_OVERLAP_PARAMS);

    if !out.ends_with(' ') {
        out.push(' ');
    }

    if overlap >= seg_words.len() {
        return;
    }
    if overlap > 0 {
        out.push_str(&seg_words[overlap..].join(" "));
    } else {
        out.push_str(seg);
    }
}

/// Strip overlapping prefix from `new_text` that matches a suffix of `last_suffix`.
///
/// Fast path: character-level, case-insensitive suffix/prefix match.
/// Fallback: normalized word overlap (exact + fuzzy) to handle small mutations
/// in streaming re-transcriptions (e.g. punctuation or 1-word typo drift).
pub fn strip_suffix_overlap(last_suffix: &str, new_text: &str) -> String {
    strip_suffix_overlap_with_params(last_suffix, new_text, DEFAULT_SUFFIX_OVERLAP_PARAMS)
}

/// Live-streaming overlap strip with stricter fuzzy bounds for deterministic runtime.
///
/// Order is deterministic:
/// 1. exact char suffix/prefix
/// 2. bounded fuzzy fallback
pub fn strip_suffix_overlap_live(last_suffix: &str, new_text: &str) -> String {
    strip_suffix_overlap_with_params(last_suffix, new_text, LIVE_SUFFIX_OVERLAP_PARAMS)
}

fn strip_suffix_overlap_with_params(
    last_suffix: &str,
    new_text: &str,
    overlap_params: OverlapParams,
) -> String {
    if last_suffix.is_empty() {
        return new_text.to_string();
    }

    if let Some(stripped) = strip_suffix_overlap_exact(last_suffix, new_text) {
        return stripped;
    }

    if let Some(stripped) = strip_suffix_overlap_fuzzy(last_suffix, new_text, overlap_params) {
        return stripped;
    }

    new_text.to_string()
}

fn strip_suffix_overlap_exact(last_suffix: &str, new_text: &str) -> Option<String> {
    // Collect valid byte offsets from char boundaries (longest first).
    let suffix_bounds: Vec<usize> = last_suffix.char_indices().map(|(i, _)| i).collect();
    let text_bounds: Vec<usize> = {
        let mut v: Vec<usize> = new_text.char_indices().map(|(i, _)| i).collect();
        v.push(new_text.len()); // include final boundary
        v
    };

    // Try overlap lengths from longest to shortest (min 3 bytes).
    for &suffix_start in &suffix_bounds {
        let suffix_tail = &last_suffix[suffix_start..];
        let tail_len = suffix_tail.len();
        if tail_len < 3 {
            break;
        }
        // Find the matching char boundary in new_text for this byte length.
        if text_bounds.binary_search(&tail_len).is_ok()
            && suffix_tail.eq_ignore_ascii_case(&new_text[..tail_len])
        {
            let stripped = new_text[tail_len..].trim_start();
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
            return Some(String::new());
        }
    }
    None
}

fn strip_suffix_overlap_fuzzy(
    last_suffix: &str,
    new_text: &str,
    overlap_params: OverlapParams,
) -> Option<String> {
    let trimmed_new = new_text.trim();
    if trimmed_new.is_empty() {
        return None;
    }

    let new_words: Vec<&str> = trimmed_new.split_whitespace().collect();
    if new_words.is_empty() {
        return None;
    }

    let max_overlap_window = new_words.len().min(overlap_params.bounded_max_window());
    let mut suffix_tail_words: Vec<&str> = last_suffix
        .split_whitespace()
        .rev()
        .take(max_overlap_window)
        .collect();
    if suffix_tail_words.is_empty() {
        return None;
    }
    suffix_tail_words.reverse();

    let overlap = detect_word_overlap(&suffix_tail_words, &new_words, overlap_params);
    if overlap == 0 {
        return None;
    }
    if overlap >= new_words.len() {
        return Some(String::new());
    }

    let stripped = new_words[overlap..].join(" ");
    tracing::debug!(
        "[FUZZY_SUFFIX_DEDUP] overlap_words={} suffix_tail={:?} new_head={:?}",
        overlap,
        &suffix_tail_words[suffix_tail_words.len().saturating_sub(overlap)..],
        &new_words[..overlap]
    );
    Some(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── chunk dedup ──────────────────────────────────────

    #[test]
    fn test_chunk_dedup_exact() {
        let mut out = "Hello world this is".to_string();
        dedup_chunk_overlap(&mut out, "this is a test");
        assert_eq!(out, "Hello world this is a test");
    }

    #[test]
    fn test_chunk_dedup_fuzzy() {
        // 1-word edit in a 3-word overlap region → should still dedup
        let mut out = "one two three four".to_string();
        dedup_chunk_overlap(&mut out, "three foor five six");
        // "four" vs "foor" = 1 edit in k=2 region... but fuzzy needs k>=3
        // Let's use a bigger overlap: "two three four" vs "two three foor"
        let mut out2 = "one two three four".to_string();
        dedup_chunk_overlap(&mut out2, "two three foor five six");
        // k=3 overlap: ["two","three","four"] vs ["two","three","foor"] → dist=1, max_err=1 → match
        assert_eq!(out2, "one two three four five six");
    }

    #[test]
    fn test_chunk_dedup_no_overlap() {
        let mut out = "Hello world".to_string();
        dedup_chunk_overlap(&mut out, "completely different");
        assert_eq!(out, "Hello world completely different");
    }

    // ── suffix overlap ───────────────────────────────────

    #[test]
    fn test_suffix_overlap_basic() {
        let result = strip_suffix_overlap("Hello world.", "world. And more.");
        assert_eq!(result, "And more.");
    }

    #[test]
    fn test_suffix_overlap_no_match() {
        let result = strip_suffix_overlap("Hello world.", "Something else.");
        assert_eq!(result, "Something else.");
    }

    #[test]
    fn test_suffix_overlap_empty() {
        let result = strip_suffix_overlap("", "Hello world.");
        assert_eq!(result, "Hello world.");
    }

    #[test]
    fn test_suffix_overlap_polish_diacritics() {
        // "ż" is 2 bytes in UTF-8 — old code would panic slicing mid-char
        let result = strip_suffix_overlap("weterynarzem.", "weterynarzem. Dziękuję.");
        assert_eq!(result, "Dziękuję.");
    }

    #[test]
    fn test_suffix_overlap_emoji() {
        // 🐕 is 4 bytes — stress-test char boundary logic
        let result = strip_suffix_overlap("pies 🐕.", "🐕. Koniec.");
        assert_eq!(result, "Koniec.");
    }

    #[test]
    fn test_suffix_overlap_word_fallback_punctuation_drift() {
        // Exact char suffix fails on "." vs " " boundary, word fallback should dedup.
        let result = strip_suffix_overlap("Thank you.", "Thank you very much.");
        assert_eq!(result, "very much.");
    }

    #[test]
    fn test_suffix_overlap_word_fallback_fuzzy_typo() {
        // "feeling" vs "feelingg" should still dedup in a larger overlap window.
        let result = strip_suffix_overlap(
            "the patient is feeling much better",
            "the patient is feelingg much better today",
        );
        assert_eq!(result, "today");
    }

    #[test]
    fn test_suffix_overlap_live_fuzzy_typo() {
        let result = strip_suffix_overlap_live(
            "the patient is feeling much better",
            "the patient is feelingg much better today",
        );
        assert_eq!(result, "today");
    }

    #[test]
    fn test_suffix_overlap_live_small_windows_do_not_trigger_fuzzy() {
        // 2-word span stays strict-only in live mode (min fuzzy overlap = 3).
        let result = strip_suffix_overlap_live("alpha beta", "alpaa betaa gamma");
        assert_eq!(result, "alpaa betaa gamma");
    }
}
