//! Whisper timestamp token helpers.
//!
//! Resolves timestamp token ranges directly from the tokenizer and parses
//! decoder token streams into segment-level timestamps.

use tokenizers::Tokenizer;

use crate::pipeline::contracts::TranscriptSegment;

/// Timestamp token range resolved from tokenizer special tokens.
#[derive(Debug, Clone, Copy)]
pub struct TimestampRange {
    /// Token ID for `<|0.00|>`.
    pub begin: u32,
    /// Token ID for `<|30.00|>`.
    pub end_inclusive: u32,
}

impl TimestampRange {
    /// Resolve the timestamp token range from tokenizer special tokens.
    pub fn from_tokenizer(tokenizer: &Tokenizer) -> Option<Self> {
        let begin = tokenizer.token_to_id("<|0.00|>")?;
        let end_inclusive = tokenizer.token_to_id("<|30.00|>")?;
        Some(Self {
            begin,
            end_inclusive,
        })
    }

    /// Returns true when `tok` is a timestamp token.
    pub fn is_timestamp(&self, tok: u32) -> bool {
        tok >= self.begin && tok <= self.end_inclusive
    }

    /// Converts timestamp token ID to seconds (Whisper: 20ms step).
    pub fn to_seconds(&self, tok: u32) -> f32 {
        (tok.saturating_sub(self.begin)) as f32 * 0.02
    }
}

/// Parse decoder token output into final text + segment-level timestamps.
pub fn extract_segments(
    all_tokens: &[u32],
    tokenizer: &Tokenizer,
    ts_range: &TimestampRange,
) -> (String, Vec<TranscriptSegment>) {
    let mut segments = Vec::new();
    let mut current_start: Option<f32> = None;
    let mut current_tokens: Vec<u32> = Vec::new();

    for &tok in all_tokens {
        if ts_range.is_timestamp(tok) {
            let time = ts_range.to_seconds(tok);
            match current_start {
                None => {
                    current_start = Some(time);
                }
                Some(start) => {
                    if !current_tokens.is_empty()
                        && let Ok(text) = tokenizer.decode(&current_tokens, true)
                    {
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            segments.push(TranscriptSegment {
                                text,
                                start_ts: start,
                                end_ts: time,
                            });
                        }
                    }
                    current_tokens.clear();
                    current_start = Some(time);
                }
            }
        } else {
            current_tokens.push(tok);
        }
    }

    // Deliberately ignore trailing tokens without a closing timestamp.
    // We only emit "closed" [start, end] spans coming from native timestamp tokens.

    let text_tokens: Vec<u32> = all_tokens
        .iter()
        .filter(|&&tok| !ts_range.is_timestamp(tok))
        .copied()
        .collect();
    let full_text = tokenizer.decode(&text_tokens, true).unwrap_or_default();

    (full_text, segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::Tokenizer;
    use tokenizers::models::wordlevel::WordLevel;

    fn test_tokenizer() -> Tokenizer {
        let vocab = [
            ("[UNK]".to_string(), 0_u32),
            ("hello".to_string(), 1_u32),
            ("world".to_string(), 2_u32),
            ("again".to_string(), 3_u32),
            ("<|0.00|>".to_string(), 1000_u32),
            ("<|0.02|>".to_string(), 1001_u32),
            ("<|0.04|>".to_string(), 1002_u32),
            ("<|30.00|>".to_string(), 1030_u32),
        ]
        .into_iter()
        .collect();

        let model = WordLevel::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("wordlevel tokenizer");

        Tokenizer::new(model)
    }

    #[test]
    fn timestamp_range_resolves_from_tokenizer() {
        let tokenizer = test_tokenizer();
        let range = TimestampRange::from_tokenizer(&tokenizer).expect("timestamp range");
        assert_eq!(range.begin, 1000);
        assert_eq!(range.end_inclusive, 1030);
        assert!(range.is_timestamp(1005));
        assert!(!range.is_timestamp(12));
    }

    #[test]
    fn extract_segments_parses_closed_spans() {
        let tokenizer = test_tokenizer();
        let range = TimestampRange::from_tokenizer(&tokenizer).expect("timestamp range");
        let tokens = vec![1000, 1, 2, 1002, 3, 1004];

        let (text, segments) = extract_segments(&tokens, &tokenizer, &range);

        assert_eq!(text.trim(), "hello world again");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "hello world");
        assert_eq!(segments[0].start_ts, 0.0);
        assert_eq!(segments[0].end_ts, 0.04);
        assert_eq!(segments[1].text, "again");
        assert_eq!(segments[1].start_ts, 0.04);
        assert_eq!(segments[1].end_ts, 0.08);
    }

    #[test]
    fn extract_segments_ignores_unclosed_trailing_span() {
        let tokenizer = test_tokenizer();
        let range = TimestampRange::from_tokenizer(&tokenizer).expect("timestamp range");
        let tokens = vec![1000, 1, 1002, 2, 3];

        let (text, segments) = extract_segments(&tokens, &tokenizer, &range);

        assert_eq!(text.trim(), "hello world again");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[0].start_ts, 0.0);
        assert_eq!(segments[0].end_ts, 0.04);
    }
}
