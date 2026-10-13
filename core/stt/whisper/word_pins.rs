//! Measured word pins from Whisper cross-attention.
//!
//! This is the OpenAI `word_timestamps` alignment
//! (`whisper/timing.py` `find_alignment`, model card alignment heads):
//! pre-softmax cross-attention from the checkpoint's alignment heads, softmax,
//! token-axis z-score, width-7 median filter, mean across heads, then DTW.
//! Interior boundaries follow the reference DTW path. Its forced frame-zero
//! first boundary is replaced by the leading edge of the first predictive
//! attention peak; it is not a measured onset.
//! A word boundary is otherwise a jump in that path. Ranges are never split uniformly
//! across characters or tokens.
//!
//! Encoder frames are 20 ms (`SAMPLE_RATE / (HOP_LENGTH * 2) = 50`), the same
//! clock as Whisper's timestamp tokens. After normalization over the full
//! forced sequence, DTW keeps `no_timestamps` through the last text row.
//! These predictive rows supply one boundary more than the text token count;
//! the final jump closes the last word without assigning it trailing silence.

/// Seconds per encoder frame after the stride-2 convolution.
pub const ENCODER_FRAME_SECS: f32 = 0.02;

/// Odd median-filter width used by OpenAI's alignment.
const MEDIAN_WIDTH: usize = 7;

/// Measured bounds of one word on the audio the decoder saw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredSpan {
    pub start_secs: f32,
    pub end_secs: f32,
}

/// One decoded word. `span` is its measured DTW range. `None` means the path
/// gave this word no positive duration: the word stays in the decoded label,
/// explicitly unaligned, and never becomes a pin. One such word does not take
/// the measured pins away from its neighbours.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredWord {
    pub text: String,
    pub span: Option<MeasuredSpan>,
    /// First text token of this word in the decode's filtered token stream
    /// (EOT/timestamp rows removed). Lets the caller re-aggregate per-token
    /// evidence — A6 per-word confidence — after punctuation merges.
    pub token_start: usize,
    /// Number of text tokens the word spans.
    pub token_len: usize,
}

impl MeasuredWord {
    #[cfg(test)]
    pub fn is_aligned(&self) -> bool {
        self.span.is_some()
    }
}

/// A word the decoder emitted but the alignment could not place. It is part
/// of the hypothesis text, not of the pin set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnalignedWord {
    /// Position of the word in the decoded word sequence, pins included.
    pub ordinal: usize,
    pub text: String,
    pub token_start: usize,
    pub token_len: usize,
}

/// Measured pins of one decode plus the words it could not place. `pins`
/// keep decode order; `unaligned` ordinals index the full word sequence, so
/// the decoded label can be rebuilt without losing a word.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignedWordPins {
    pub pins: Vec<crate::pipeline::contracts::TranscriptSegment>,
    pub unaligned: Vec<UnalignedWord>,
}

impl AlignedWordPins {
    pub fn is_partial(&self) -> bool {
        !self.unaligned.is_empty()
    }

    /// Every decoded word in order, pinned or not, joined by one space. The
    /// decoded label never shrinks to the pinned subset.
    pub fn label_words(&self) -> Vec<&str> {
        let total = self.pins.len() + self.unaligned.len();
        let mut pins = self.pins.iter();
        let mut unaligned = self.unaligned.iter().peekable();
        let mut words = Vec::with_capacity(total);
        for ordinal in 0..total {
            if unaligned.peek().is_some_and(|word| word.ordinal == ordinal) {
                words.push(unaligned.next().map_or("", |word| word.text.as_str()));
            } else if let Some(pin) = pins.next() {
                words.push(pin.text.as_str());
            }
        }
        words
    }
}

/// Align text tokens to encoder frames.
///
/// `head_qk[head][token][frame]` is pre-softmax cross-attention for the full
/// forced sequence (`sot…`, `no_timestamps`, text, `eot`). `sot_prefix_len` is
/// `len(sot_sequence)` and does not include `no_timestamps`. `content_frames`
/// trims padded encoder frames (`num_frames // 2` in the reference).
///
/// Returns `None` when the inputs are malformed or no word receives a
/// positive measured span; callers keep the utterance-grain span then. A word
/// whose path jumps give it no positive duration comes back with `span: None`
/// while its neighbours keep their measured pins and its text stays in place.
pub fn align_measured_words(
    head_qk: &[Vec<Vec<f32>>],
    sot_prefix_len: usize,
    word_token_counts: &[usize],
    word_texts: &[&str],
    content_frames: usize,
) -> Option<Vec<MeasuredWord>> {
    if head_qk.is_empty()
        || word_token_counts.is_empty()
        || word_token_counts.len() != word_texts.len()
        || content_frames == 0
    {
        tracing::info!(
            reason = "invalid_alignment_inputs",
            head_count = head_qk.len(),
            word_count = word_token_counts.len(),
            text_count = word_texts.len(),
            content_frames,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }
    let token_rows = head_qk[0].len();
    if token_rows <= sot_prefix_len + 2 || head_qk.iter().any(|head| head.len() != token_rows) {
        tracing::info!(
            reason = "inconsistent_token_rows",
            token_rows,
            sot_prefix_len,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }
    let frame_count = head_qk[0][0].len().min(content_frames);
    if frame_count == 0
        || head_qk
            .iter()
            .any(|head| head.iter().any(|row| row.len() < frame_count))
    {
        tracing::info!(
            reason = "inconsistent_frame_rows",
            frame_count,
            content_frames,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }

    // Reference matrix[len(sot_sequence):-1]: the no_timestamps row predicts
    // the first text token, and the last text row supplies the closing jump.
    // Removing the first predictive row leaves no measured final boundary.
    let row_start = sot_prefix_len;
    let row_end = token_rows - 1;
    if row_end <= row_start {
        tracing::info!(
            reason = "missing_text_rows",
            row_start,
            row_end,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }
    let rows = row_end - row_start;
    let text_tokens: usize = word_token_counts.iter().sum();
    if rows - 1 != text_tokens {
        tracing::info!(
            reason = "token_count_mismatch",
            text_tokens,
            rows,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }

    let mut weights = Vec::with_capacity(head_qk.len());
    let mut onset_weights = vec![0.0_f32; frame_count];
    for head in head_qk {
        // Prefix and EOT participate in token-axis normalization before the
        // DTW slice, just as in find_alignment. Slicing first changes the
        // attention distribution and therefore every measured boundary.
        let mut matrix = vec![vec![0.0_f32; frame_count]; token_rows];
        for (row, src) in matrix.iter_mut().enumerate() {
            src.copy_from_slice(&head[row][..frame_count]);
            softmax_in_place(src);
        }
        // Onset reads this token's distribution over time. Token-axis
        // z-scoring can amplify background where other tokens are absent.
        let mut first = [matrix[row_start].clone()];
        median_filter_rows(&mut first, MEDIAN_WIDTH);
        for (sum, value) in onset_weights.iter_mut().zip(&first[0]) {
            *sum += value / head_qk.len() as f32;
        }
        zscore_token_axis(&mut matrix);
        median_filter_rows(&mut matrix, MEDIAN_WIDTH);
        weights.push(matrix);
    }

    let mut mean = vec![vec![0.0_f32; frame_count]; rows];
    let scale = head_qk.len() as f32;
    for head in &weights {
        for (row_index, row) in head[row_start..row_end].iter().enumerate() {
            for (frame, value) in row.iter().enumerate() {
                mean[row_index][frame] += value / scale;
            }
        }
    }

    let mut cost = vec![vec![0.0_f64; frame_count]; rows];
    for (row_index, row) in mean.iter().enumerate() {
        for (frame, value) in row.iter().enumerate() {
            cost[row_index][frame] = f64::from(-value);
        }
    }
    let (text_indices, time_indices) = dtw(&cost);
    let mut jump_times = jump_times(&text_indices, &time_indices);
    if jump_times.len() < rows {
        tracing::info!(
            reason = "incomplete_dtw_path",
            jump_count = jump_times.len(),
            rows,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }
    // Global DTW necessarily opens row zero at frame zero. Read the first
    // predictive attention peak instead, without moving any interior/tail jump.
    let onset = first_predictive_onset(&onset_weights)?;
    jump_times[0] = onset as f32 * ENCODER_FRAME_SECS;
    let mut boundaries = Vec::with_capacity(word_token_counts.len() + 1);
    boundaries.push(0usize);
    let mut cursor = 0usize;
    for (word_index, count) in word_token_counts.iter().enumerate() {
        if *count == 0 {
            tracing::info!(
                reason = "zero_word_token_count",
                word_index,
                "tail_word_pin_alignment_refused"
            );
            return None;
        }
        cursor += count;
        boundaries.push(cursor);
    }
    if boundaries.last().copied() != Some(text_tokens) {
        tracing::info!(
            reason = "word_boundary_count_mismatch",
            boundary_token_end = ?boundaries.last().copied(),
            text_tokens,
            "tail_word_pin_alignment_refused"
        );
        return None;
    }

    let mut words = Vec::with_capacity(word_texts.len());
    let mut unaligned = 0usize;
    for (index, text) in word_texts.iter().enumerate() {
        let start = jump_times[boundaries[index]];
        let end_index = boundaries[index + 1];
        let end = jump_times[end_index];
        let text = text.trim();
        if text.is_empty() {
            tracing::info!(
                reason = "empty_word_text",
                word_index = index,
                token_start = boundaries[index],
                token_len = end_index - boundaries[index],
                "tail_word_pin_alignment_refused"
            );
            return None;
        }
        // A collapsed jump pair is this word's failure, not the window's.
        // The word keeps its text and loses only its pin.
        let span = if end > start {
            Some(MeasuredSpan {
                start_secs: start,
                end_secs: end,
            })
        } else {
            unaligned += 1;
            tracing::info!(
                reason = "nonpositive_word_duration",
                word_index = index,
                token_start = boundaries[index],
                token_len = end_index - boundaries[index],
                start_secs = start,
                end_secs = end,
                "tail_word_pin_unaligned"
            );
            None
        };
        words.push(MeasuredWord {
            text: text.to_string(),
            span,
            token_start: boundaries[index],
            token_len: end_index - boundaries[index],
        });
    }
    if unaligned == words.len() {
        tracing::info!(
            reason = "no_measured_word",
            word_count = words.len(),
            "tail_word_pin_alignment_refused"
        );
        return None;
    }
    if unaligned > 0 {
        tracing::info!(
            unaligned,
            measured = words.len() - unaligned,
            "tail_word_pin_alignment_partial"
        );
    }
    Some(words)
}

/// Span of a word after punctuation joins it. The word's own span is the
/// authority: a measured piece of punctuation widens it, an unmeasured one
/// leaves it, and a word without a span stays unaligned. Punctuation never
/// lends a word bounds the word itself did not measure.
fn merged_span(
    word: Option<MeasuredSpan>,
    punctuation: Option<MeasuredSpan>,
    punctuation_leads: bool,
) -> Option<MeasuredSpan> {
    let word = word?;
    Some(match punctuation {
        Some(punctuation) if punctuation_leads => MeasuredSpan {
            start_secs: punctuation.start_secs.min(word.start_secs),
            end_secs: word.end_secs,
        },
        Some(punctuation) => MeasuredSpan {
            start_secs: word.start_secs,
            end_secs: punctuation.end_secs.max(word.end_secs),
        },
        None => word,
    })
}

/// Group tokenizer pieces into `(start, len)` spans. A piece that starts
/// with Whisper's space marker (`Ġ`), a space, or a newline opens a word.
/// The caller decodes each span; concatenating raw pieces is not text.
pub fn word_token_spans(pieces: &[String]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    for (index, piece) in pieces.iter().enumerate() {
        if count > 0 && piece_opens_word(piece) {
            spans.push((start, count));
            start = index;
            count = 0;
        }
        if count == 0 {
            start = index;
        }
        count += 1;
    }
    if count > 0 {
        spans.push((start, count));
    }
    spans
}

/// Merge punctuation the way `whisper/timing.py` `merge_punctuations` does,
/// keeping the measured union of the token ranges.
pub fn merge_word_punctuation(words: &mut Vec<MeasuredWord>) {
    const PREPENDED: &[char] = &['"', '\'', '“', '¿', '(', '[', '{', '-'];
    const APPENDED: &[&str] = &[
        "\"", "'", ".", "。", ",", "，", "!", "！", "?", "？", ":", "：", "”", ")", "]", "}", "、",
    ];

    let mut index = words.len().saturating_sub(1);
    while index > 0 {
        let previous = index - 1;
        let lead = words[previous].text.trim();
        let opens =
            words[previous].text.starts_with(' ') && lead.chars().all(|ch| PREPENDED.contains(&ch));
        if opens {
            let taken = words[previous].clone();
            words[index].text = format!("{}{}", taken.text.trim(), words[index].text.trim());
            words[index].span = merged_span(words[index].span, taken.span, true);
            let token_end = words[index].token_start + words[index].token_len;
            words[index].token_start = taken.token_start;
            words[index].token_len = token_end - taken.token_start;
            words[previous].text.clear();
        }
        index -= 1;
    }

    let mut index = 0usize;
    while index + 1 < words.len() {
        let following = words[index + 1].text.trim();
        if !words[index].text.ends_with(' ') && APPENDED.contains(&following) {
            let trail = words[index + 1].span;
            words[index].text = format!("{}{following}", words[index].text.trim());
            words[index].span = merged_span(words[index].span, trail, false);
            words[index].token_len = (words[index + 1].token_start + words[index + 1].token_len)
                - words[index].token_start;
            words[index + 1].text.clear();
        }
        index += 1;
    }
    words.retain(|word| !word.text.trim().is_empty());
}

fn piece_opens_word(piece: &str) -> bool {
    matches!(piece.chars().next(), Some('Ġ' | ' ' | 'Ċ' | '\n'))
}

fn softmax_in_place(row: &mut [f32]) {
    let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !max.is_finite() {
        return;
    }
    let mut sum = 0.0_f32;
    for value in row.iter_mut() {
        *value = (*value - max).exp();
        sum += *value;
    }
    if sum > 0.0 && sum.is_finite() {
        for value in row.iter_mut() {
            *value /= sum;
        }
    }
}

/// Population z-score over the token axis, per frame (`unbiased=False`).
fn zscore_token_axis(matrix: &mut [Vec<f32>]) {
    if matrix.is_empty() {
        return;
    }
    let frames = matrix[0].len();
    let rows = matrix.len() as f32;
    for frame in 0..frames {
        let mut mean = 0.0_f32;
        for row in matrix.iter() {
            mean += row[frame];
        }
        mean /= rows;
        let mut variance = 0.0_f32;
        for row in matrix.iter() {
            let delta = row[frame] - mean;
            variance += delta * delta;
        }
        variance /= rows;
        let std = variance.sqrt().max(1.0e-6);
        for row in matrix.iter_mut() {
            row[frame] = (row[frame] - mean) / std;
        }
    }
}

fn median_filter_rows(matrix: &mut [Vec<f32>], width: usize) {
    if width < 3 || width.is_multiple_of(2) {
        return;
    }
    let pad = width / 2;
    for row in matrix.iter_mut() {
        if row.len() <= pad {
            continue;
        }
        let mut padded = Vec::with_capacity(row.len() + width);
        for index in (1..=pad).rev() {
            padded.push(row[index]);
        }
        padded.extend_from_slice(row);
        for index in 1..=pad {
            padded.push(row[row.len() - 1 - index]);
        }
        let mut filtered = row.clone();
        let mut window = vec![0.0_f32; width];
        for (index, slot) in filtered.iter_mut().enumerate() {
            window.copy_from_slice(&padded[index..index + width]);
            window.sort_by(|left, right| left.total_cmp(right));
            *slot = window[width / 2];
        }
        *row = filtered;
    }
}

/// Leading half-height crossing of the dominant predictive attention peak.
/// Median-filtered time probabilities, averaged across alignment heads.
/// A flat/nonpositive row has no onset evidence.
fn first_predictive_onset(scores: &[f32]) -> Option<usize> {
    let (peak_index, &peak) = scores
        .iter()
        .enumerate()
        .filter(|(_, value)| value.is_finite())
        .max_by(|left, right| left.1.total_cmp(right.1))?;
    if peak <= 0.0 {
        return None;
    }
    let floor = scores
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .min_by(f32::total_cmp)?;
    if peak - floor <= f32::EPSILON {
        return None;
    }
    let threshold = peak * 0.5;
    let mut onset = peak_index;
    while onset > 0 && scores[onset - 1] >= threshold {
        onset -= 1;
    }
    Some(onset)
}

/// OpenAI `dtw_cpu`: border is infinite, ties prefer a time step.
fn dtw(cost: &[Vec<f64>]) -> (Vec<usize>, Vec<usize>) {
    let rows = cost.len();
    let cols = cost.first().map_or(0, Vec::len);
    let mut accum = vec![vec![f64::INFINITY; cols + 1]; rows + 1];
    let mut trace = vec![vec![0_u8; cols + 1]; rows + 1];
    accum[0][0] = 0.0;
    for col in 1..=cols {
        for row in 1..=rows {
            let diagonal = accum[row - 1][col - 1];
            let up = accum[row - 1][col];
            let left = accum[row][col - 1];
            let (step_cost, kind) = if diagonal < up && diagonal < left {
                (diagonal, 0_u8)
            } else if up < diagonal && up < left {
                (up, 1_u8)
            } else {
                (left, 2_u8)
            };
            accum[row][col] = cost[row - 1][col - 1] + step_cost;
            trace[row][col] = kind;
        }
    }
    if let Some(first) = trace.first_mut() {
        for cell in first.iter_mut() {
            *cell = 2;
        }
    }
    for row in &mut trace {
        if let Some(cell) = row.first_mut() {
            *cell = 1;
        }
    }
    let mut row = rows;
    let mut col = cols;
    let mut path = Vec::new();
    while row > 0 || col > 0 {
        path.push((row.saturating_sub(1), col.saturating_sub(1)));
        match trace[row][col] {
            0 => {
                row = row.saturating_sub(1);
                col = col.saturating_sub(1);
            }
            1 => row = row.saturating_sub(1),
            _ => col = col.saturating_sub(1),
        }
    }
    path.reverse();
    let text = path.iter().map(|(text, _)| *text).collect();
    let time = path.iter().map(|(_, time)| *time).collect();
    (text, time)
}

fn jump_times(text_indices: &[usize], time_indices: &[usize]) -> Vec<f32> {
    let mut jumps = Vec::new();
    for (index, time) in time_indices.iter().enumerate() {
        let jumped = index == 0 || text_indices[index] > text_indices[index - 1];
        if jumped {
            jumps.push(*time as f32 * ENCODER_FRAME_SECS);
        }
    }
    jumps
}

#[cfg(test)]
#[path = "word_pins_oracle_tests.rs"]
mod oracle_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn spike(rows: usize, frames: usize, peaks: &[(usize, usize)]) -> Vec<Vec<Vec<f32>>> {
        let mut matrix = vec![vec![0.0_f32; frames]; rows];
        for (row, frame) in peaks {
            for offset in 0..5 {
                let column = frame + offset;
                if column < frames {
                    matrix[*row][column] = 8.0;
                }
            }
        }
        vec![matrix]
    }

    /// Attention peaks, not token counts, place the boundary.
    ///
    /// Two one-token words. The predictive attention jumps open the second
    /// word before the midpoint, rather than at a uniform half-window split.
    #[test]
    fn alignment_follows_attention_peaks_not_a_uniform_split() {
        let frames = 40;
        // Two equal tokens. A uniform split would open the second word at
        // half the frame axis. Attention puts the first word on an early blob,
        // so the boundary follows that blob.
        // Separate the predictive peaks: the former row-1 peak at frame 2
        // gives a zero-duration first word in the pinned OpenAI reference.
        let heads = spike(4, frames, &[(0, 0), (1, 12), (2, 30)]);
        let words = align_measured_words(&heads, 0, &[1, 1], &["raz", "dwa"], frames)
            .expect("measured pins");
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "raz");
        assert_eq!(words[1].text, "dwa");
        let first = words[0].span.expect("first word measured");
        let second = words[1].span.expect("second word measured");
        let uniform_half = (frames as f32 / 2.0) * ENCODER_FRAME_SECS;
        assert!(
            second.start_secs < uniform_half - 0.15,
            "second word started at {}s; a uniform split of two tokens would open it at {uniform_half}s",
            second.start_secs
        );
        assert!(second.end_secs > second.start_secs);
        assert!(first.end_secs <= second.start_secs + f32::EPSILON);
    }

    /// One collapsed jump pair unpins that word only. The neighbour keeps its
    /// measured span and the decoded label keeps both words in order.
    #[test]
    fn zero_duration_word_stays_in_the_label_without_a_pin() {
        // Row-1 peak at frame 2 collapses the first word in the pinned OpenAI
        // reference (boundaries [0, 0, 7]); the second word is still measured.
        let heads = spike(4, 40, &[(0, 0), (1, 2), (2, 30)]);
        let words = align_measured_words(&heads, 0, &[1, 1], &["raz", "dwa"], 40)
            .expect("one measured word keeps the alignment");
        assert_eq!(
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            ["raz", "dwa"]
        );
        assert_eq!((words[0].token_start, words[0].token_len), (0, 1));
        assert_eq!((words[1].token_start, words[1].token_len), (1, 1));
        assert!(
            !words[0].is_aligned(),
            "collapsed word must not carry a pin"
        );
        let second = words[1].span.expect("second word measured");
        assert!(second.end_secs > second.start_secs);
    }

    #[test]
    fn unaligned_word_is_not_a_pin_after_punctuation_merge() {
        let mut words = vec![
            MeasuredWord {
                text: "raz".into(),
                span: None,
                token_start: 0,
                token_len: 1,
            },
            MeasuredWord {
                text: ",".into(),
                span: Some(MeasuredSpan {
                    start_secs: 0.4,
                    end_secs: 0.46,
                }),
                token_start: 1,
                token_len: 1,
            },
            MeasuredWord {
                text: "dwa".into(),
                span: Some(MeasuredSpan {
                    start_secs: 0.5,
                    end_secs: 0.9,
                }),
                token_start: 2,
                token_len: 1,
            },
        ];
        merge_word_punctuation(&mut words);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "raz,");
        assert_eq!(
            words[0].span, None,
            "punctuation lends no bounds to an unaligned word"
        );
        assert_eq!((words[0].token_start, words[0].token_len), (0, 2));
        assert_eq!(words[1].text, "dwa");
        assert!(words[1].is_aligned());
    }

    #[test]
    fn label_words_keep_decode_order_across_pins_and_unaligned_words() {
        let pin =
            |text: &str, start: f32, end: f32| crate::pipeline::contracts::TranscriptSegment {
                text: text.into(),
                start_ts: start,
                end_ts: end,
                confidence: None,
            };
        let aligned = AlignedWordPins {
            pins: vec![pin("raz", 0.0, 0.2), pin("trzy", 0.5, 0.9)],
            unaligned: vec![UnalignedWord {
                ordinal: 1,
                text: "dwa".into(),
                token_start: 1,
                token_len: 1,
            }],
        };
        assert!(aligned.is_partial());
        assert_eq!(aligned.label_words(), ["raz", "dwa", "trzy"]);
        let complete = AlignedWordPins {
            pins: vec![pin("raz", 0.0, 0.2)],
            unaligned: Vec::new(),
        };
        assert!(!complete.is_partial());
        assert_eq!(complete.label_words(), ["raz"]);
    }

    #[test]
    fn space_marker_opens_a_word_and_punctuation_stays_attached() {
        let pieces = vec![
            "Ġraz".to_string(),
            "Ġdwa".to_string(),
            ",".to_string(),
            "Ġtrzy".to_string(),
        ];
        let grouped = word_token_spans(&pieces);
        assert_eq!(grouped, vec![(0, 1), (1, 2), (3, 1)]);
        let mut words = vec![
            MeasuredWord {
                text: "raz".into(),
                span: Some(MeasuredSpan {
                    start_secs: 0.0,
                    end_secs: 0.2,
                }),
                token_start: 0,
                token_len: 1,
            },
            MeasuredWord {
                text: "dwa".into(),
                span: Some(MeasuredSpan {
                    start_secs: 0.2,
                    end_secs: 0.4,
                }),
                token_start: 1,
                token_len: 1,
            },
            MeasuredWord {
                text: ",".into(),
                span: Some(MeasuredSpan {
                    start_secs: 0.4,
                    end_secs: 0.46,
                }),
                token_start: 2,
                token_len: 1,
            },
        ];
        merge_word_punctuation(&mut words);
        assert_eq!(words.len(), 2);
        assert_eq!(words[1].text, "dwa,");
        let merged = words[1].span.expect("merged word keeps its span");
        assert_eq!(merged.start_secs, 0.2);
        assert_eq!(merged.end_secs, 0.46);
        // The punctuation token joins the word's span so per-token evidence
        // (A6 confidence aggregation) covers the merged surface.
        assert_eq!((words[1].token_start, words[1].token_len), (1, 2));
    }

    #[test]
    fn first_predictive_onset_follows_the_peak_after_leading_silence() {
        let scores = [-0.2, 0.1, -0.1, 0.2, 1.2, 2.0, 1.4, -0.3];
        assert_eq!(first_predictive_onset(&scores), Some(4));
        let mut shifted = vec![-0.2; 5];
        shifted.extend(scores);
        assert_eq!(first_predictive_onset(&shifted), Some(9));
        assert_eq!(first_predictive_onset(&[0.0; 20]), None);
        assert_eq!(first_predictive_onset(&[-1.0; 20]), None);
        assert_eq!(first_predictive_onset(&[f32::NAN; 20]), None);
    }

    #[test]
    fn first_word_pin_moves_with_attention_not_the_decode_window_origin() {
        let heads = spike(4, 80, &[(0, 10), (1, 35), (2, 60)]);
        let words = align_measured_words(&heads, 0, &[1, 1], &["Ostatni", "wyraz"], 80).unwrap();
        assert_eq!(words.len(), 2);
        let first = words[0].span.expect("first word measured");
        assert_eq!(first.start_secs, 10.0 * ENCODER_FRAME_SECS);
        assert_eq!(words[0].token_start, 0);
        assert_eq!(words[1].token_start, 1);
        assert!(first.end_secs > first.start_secs);
    }

    #[test]
    fn empty_attention_does_not_invent_ranges() {
        assert!(align_measured_words(&[], 1, &[1], &["raz"], 10).is_none());
    }
}
