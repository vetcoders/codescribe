/// Unified deduplication for the transcription pipeline.
///
/// Two granularities:
/// - **Chunk overlap** (`dedup_chunk_overlap`): word-level exact+fuzzy dedup at chunk boundaries
///   (ported from `engine::append_with_overlap_dedup`)
/// - **Suffix overlap** (`strip_suffix_overlap`): character-level suffix/prefix strip between utterances
///   (ported from `TranscriptionPipeline::strip_overlap`)

// ── helpers ──────────────────────────────────────────────

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
fn word_edit_distance(a: &[String], b: &[String]) -> usize {
    let m = a.len();
    let n = b.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];

    for i in 1..=m {
        cur[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev.clone_from(&cur);
    }
    prev[n]
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
    let out_words: Vec<&str> = out_trim.split_whitespace().collect();
    let seg_words: Vec<&str> = seg.split_whitespace().collect();
    if out_words.is_empty() || seg_words.is_empty() {
        if !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(seg);
        return;
    }

    let out_norm: Vec<String> = out_words
        .iter()
        .map(|word| normalize_token_for_overlap(word))
        .collect();
    let seg_norm: Vec<String> = seg_words
        .iter()
        .map(|word| normalize_token_for_overlap(word))
        .collect();

    let max_overlap = out_words.len().min(seg_words.len()).min(30);
    let mut overlap = 0usize;

    // Pass 1: exact match (fast path)
    for k in (1..=max_overlap).rev() {
        if out_norm[out_norm.len() - k..] == seg_norm[..k] {
            overlap = k;
            break;
        }
    }

    // Pass 2: fuzzy match — allow up to k/3 word edits (min 1)
    if overlap == 0 {
        for k in (3..=max_overlap).rev() {
            let tail = &out_norm[out_norm.len() - k..];
            let head = &seg_norm[..k];
            let max_errors = (k / 3).max(1);
            let dist = word_edit_distance(tail, head);
            if dist <= max_errors {
                overlap = k;
                tracing::debug!(
                    "[FUZZY_DEDUP] matched k={} dist={} max_err={} tail={:?} head={:?}",
                    k,
                    dist,
                    max_errors,
                    &tail[..tail.len().min(5)],
                    &head[..head.len().min(5)]
                );
                break;
            }
        }
    }

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
/// Character-level, case-insensitive. Returns the non-overlapping remainder.
pub fn strip_suffix_overlap(last_suffix: &str, new_text: &str) -> String {
    if last_suffix.is_empty() {
        return new_text.to_string();
    }
    for len in (3..=last_suffix.len().min(new_text.len())).rev() {
        if last_suffix[last_suffix.len().saturating_sub(len)..]
            .eq_ignore_ascii_case(&new_text[..len])
        {
            let stripped = new_text[len..].trim_start();
            if !stripped.is_empty() {
                return stripped.to_string();
            }
            return String::new();
        }
    }
    new_text.to_string()
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
}
