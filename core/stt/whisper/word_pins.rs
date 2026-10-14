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

/// `whisper/timing.py` `merge_punctuations` defaults: marks that open onto
/// the following word.
const PREPENDED_PUNCTUATION: &[char] = &['"', '\'', '“', '¿', '(', '[', '{', '-'];

/// Marks that close onto the preceding word.
const APPENDED_PUNCTUATION: &[&str] = &[
    "\"", "'", ".", "。", ",", "，", "!", "！", "?", "？", ":", "：", "”", ")", "]", "}", "、",
];

/// One measured word. `start_secs` / `end_secs` are on the audio the decoder saw.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredWord {
    pub text: String,
    pub start_secs: f32,
    pub end_secs: f32,
    /// First text token of this word in the decode's filtered token stream
    /// (EOT/timestamp rows removed). Lets the caller re-aggregate per-token
    /// evidence — A6 per-word confidence — after punctuation merges.
    pub token_start: usize,
    /// Number of text tokens the word spans.
    pub token_len: usize,
}

/// Align text tokens to encoder frames.
///
/// `head_qk[head][token][frame]` is pre-softmax cross-attention for the full
/// forced sequence (`sot…`, `no_timestamps`, text, `eot`). `sot_prefix_len` is
/// `len(sot_sequence)` and does not include `no_timestamps`. `content_frames`
/// trims padded encoder frames (`num_frames // 2` in the reference).
///
/// A word with no measured frames (nonpositive duration) keeps its text and
/// tokens inside the nearest measured word: the following one, or the last
/// one for a trailing run. Returns `None` when the inputs are inconsistent or
/// no word has a positive measured range. Callers keep the utterance-grain
/// span in that case.
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

    // A word whose path jumps coincide, or whose first jump is the predictive
    // onset past its end, has no measured frames of its own. Its text and
    // tokens join the following measured word in reading order, under that
    // word's own range: the lexical content survives and no duration is
    // invented. Closing punctuation and a trailing unmeasured run join the
    // previous measured word instead.
    let mut words: Vec<MeasuredWord> = Vec::with_capacity(word_texts.len());
    let mut unmeasured: Vec<MeasuredWord> = Vec::new();
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
        let mut word = MeasuredWord {
            text: text.to_string(),
            start_secs: start,
            end_secs: end,
            token_start: boundaries[index],
            token_len: end_index - boundaries[index],
        };
        if end <= start {
            tracing::info!(
                reason = "nonpositive_word_duration",
                word_index = index,
                token_start = word.token_start,
                token_len = word.token_len,
                start_secs = start,
                end_secs = end,
                "tail_word_pin_merged_into_measured_neighbour"
            );
            if APPENDED_PUNCTUATION.contains(&text)
                && unmeasured.is_empty()
                && let Some(last) = words.last_mut()
            {
                last.text = join_word_text(&last.text, &word.text);
                last.token_len = word.token_start + word.token_len - last.token_start;
            } else {
                unmeasured.push(word);
            }
            continue;
        }
        if let Some(token_start) = unmeasured.first().map(|carried| carried.token_start) {
            let mut lead = String::new();
            for carried in unmeasured.drain(..) {
                lead = join_word_text(&lead, &carried.text);
            }
            word.text = join_word_text(&lead, &word.text);
            word.token_len += word.token_start - token_start;
            word.token_start = token_start;
        }
        words.push(word);
    }
    if !unmeasured.is_empty() {
        let Some(last) = words.last_mut() else {
            tracing::info!(
                reason = "no_measured_word",
                word_count = word_texts.len(),
                "tail_word_pin_alignment_refused"
            );
            return None;
        };
        for carried in unmeasured.drain(..) {
            last.text = join_word_text(&last.text, &carried.text);
            last.token_len = carried.token_start + carried.token_len - last.token_start;
        }
    }
    Some(words)
}

/// Join two adjacent word texts in reading order. Opening punctuation binds
/// to the word after it and closing punctuation to the word before it, as in
/// [`merge_word_punctuation`]; any other pair keeps one separating space.
fn join_word_text(left: &str, right: &str) -> String {
    if left.is_empty() {
        return right.to_string();
    }
    let glued = left.chars().all(|ch| PREPENDED_PUNCTUATION.contains(&ch))
        || APPENDED_PUNCTUATION.contains(&right);
    if glued {
        format!("{left}{right}")
    } else {
        format!("{left} {right}")
    }
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
    let mut index = words.len().saturating_sub(1);
    while index > 0 {
        let previous = index - 1;
        let lead = words[previous].text.trim();
        let opens = words[previous].text.starts_with(' ')
            && lead.chars().all(|ch| PREPENDED_PUNCTUATION.contains(&ch));
        if opens {
            let taken = words[previous].clone();
            words[index].text = format!("{}{}", taken.text.trim(), words[index].text.trim());
            words[index].start_secs = taken.start_secs;
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
        if !words[index].text.ends_with(' ') && APPENDED_PUNCTUATION.contains(&following) {
            let end = words[index + 1].end_secs;
            words[index].text = format!("{}{following}", words[index].text.trim());
            words[index].end_secs = end;
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
        let uniform_half = (frames as f32 / 2.0) * ENCODER_FRAME_SECS;
        assert!(
            words[1].start_secs < uniform_half - 0.15,
            "second word started at {}s; a uniform split of two tokens would open it at {uniform_half}s",
            words[1].start_secs
        );
        assert!(words[1].end_secs > words[1].start_secs);
        assert!(words[0].end_secs <= words[1].start_secs + f32::EPSILON);
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
                start_secs: 0.0,
                end_secs: 0.2,
                token_start: 0,
                token_len: 1,
            },
            MeasuredWord {
                text: "dwa".into(),
                start_secs: 0.2,
                end_secs: 0.4,
                token_start: 1,
                token_len: 1,
            },
            MeasuredWord {
                text: ",".into(),
                start_secs: 0.4,
                end_secs: 0.46,
                token_start: 2,
                token_len: 1,
            },
        ];
        merge_word_punctuation(&mut words);
        assert_eq!(words.len(), 2);
        assert_eq!(words[1].text, "dwa,");
        assert_eq!(words[1].start_secs, 0.2);
        assert_eq!(words[1].end_secs, 0.46);
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
        assert_eq!(words[0].start_secs, 10.0 * ENCODER_FRAME_SECS);
        assert_eq!(words[0].token_start, 0);
        assert_eq!(words[1].token_start, 1);
        assert!(words[0].end_secs > words[0].start_secs);
    }

    #[test]
    fn empty_attention_does_not_invent_ranges() {
        assert!(align_measured_words(&[], 1, &[1], &["raz"], 10).is_none());
    }
}
