//! Measured word pins from Whisper cross-attention.
//!
//! This is the OpenAI `word_timestamps` alignment
//! (`whisper/timing.py` `find_alignment`, model card alignment heads):
//! pre-softmax cross-attention from the checkpoint's alignment heads, softmax,
//! token-axis z-score, width-7 median filter, mean across heads, then DTW.
//! A word boundary is a jump in that path. Ranges are never split uniformly
//! across characters or tokens.
//!
//! Encoder frames are 20 ms (`SAMPLE_RATE / (HOP_LENGTH * 2) = 50`), the same
//! clock as Whisper's timestamp tokens. DTW runs on the text tokens: the
//! `sot` prefix, the `no_timestamps` row, and the closing end-of-text row are
//! not word pins.

/// Seconds per encoder frame after the stride-2 convolution.
pub const ENCODER_FRAME_SECS: f32 = 0.02;

/// Odd median-filter width used by OpenAI's alignment.
const MEDIAN_WIDTH: usize = 7;

/// One measured word. `start_secs` / `end_secs` are on the audio the decoder saw.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredWord {
    pub text: String,
    pub start_secs: f32,
    pub end_secs: f32,
}

/// Align text tokens to encoder frames.
///
/// `head_qk[head][token][frame]` is pre-softmax cross-attention for the full
/// forced sequence (`sot…`, `no_timestamps`, text, `eot`). `sot_prefix_len` is
/// `len(sot_sequence)` and does not include `no_timestamps`. `content_frames`
/// trims padded encoder frames (`num_frames // 2` in the reference).
///
/// Returns `None` when the path does not yield a strictly increasing pin per
/// word. Callers keep the utterance-grain span in that case.
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
        return None;
    }
    let token_rows = head_qk[0].len();
    if token_rows <= sot_prefix_len + 2 || head_qk.iter().any(|head| head.len() != token_rows) {
        return None;
    }
    let frame_count = head_qk[0][0].len().min(content_frames);
    if frame_count == 0
        || head_qk
            .iter()
            .any(|head| head.iter().any(|row| row.len() < frame_count))
    {
        return None;
    }

    // Drop the sot prefix, the `no_timestamps` row that follows it, and the
    // final eot row. DTW then runs on the text tokens alone, so a word boundary
    // is the first encoder frame of the next word's tokens.
    let row_start = sot_prefix_len + 1;
    let row_end = token_rows - 1;
    if row_end <= row_start {
        return None;
    }
    let rows = row_end - row_start;
    let text_tokens: usize = word_token_counts.iter().sum();
    if text_tokens != rows {
        return None;
    }

    let mut weights = Vec::with_capacity(head_qk.len());
    for head in head_qk {
        let mut matrix = vec![vec![0.0_f32; frame_count]; rows];
        for (row, src) in matrix.iter_mut().enumerate() {
            src.copy_from_slice(&head[row_start + row][..frame_count]);
            softmax_in_place(src);
        }
        zscore_token_axis(&mut matrix);
        median_filter_rows(&mut matrix, MEDIAN_WIDTH);
        weights.push(matrix);
    }

    let mut mean = vec![vec![0.0_f32; frame_count]; rows];
    let scale = head_qk.len() as f32;
    for head in &weights {
        for (row_index, row) in head.iter().enumerate() {
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
    let jump_times = jump_times(&text_indices, &time_indices);
    if jump_times.len() < rows {
        return None;
    }
    let path_end =
        time_indices.last().copied().unwrap_or(0).saturating_add(1) as f32 * ENCODER_FRAME_SECS;

    let mut boundaries = Vec::with_capacity(word_token_counts.len() + 1);
    boundaries.push(0usize);
    let mut cursor = 0usize;
    for count in word_token_counts {
        if *count == 0 {
            return None;
        }
        cursor += count;
        boundaries.push(cursor);
    }
    if boundaries.last().copied() != Some(text_tokens) {
        return None;
    }

    let mut words = Vec::with_capacity(word_texts.len());
    for (index, text) in word_texts.iter().enumerate() {
        let start = jump_times[boundaries[index]];
        let end_index = boundaries[index + 1];
        let end = if end_index < jump_times.len() {
            jump_times[end_index]
        } else {
            path_end
        };
        if end <= start {
            return None;
        }
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        words.push(MeasuredWord {
            text: text.to_string(),
            start_secs: start,
            end_secs: end,
        });
    }
    Some(words)
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
            words[index].start_secs = taken.start_secs;
            words[previous].text.clear();
        }
        index -= 1;
    }

    let mut index = 0usize;
    while index + 1 < words.len() {
        let following = words[index + 1].text.trim();
        if !words[index].text.ends_with(' ') && APPENDED.contains(&following) {
            let end = words[index + 1].end_secs;
            words[index].text = format!("{}{following}", words[index].text.trim());
            words[index].end_secs = end;
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
    /// Two words: one token then five. A uniform token split would open the
    /// second word at 1/6 of the frame axis. The alignment heads put that
    /// word on the late blob, so the pin starts after the midpoint.
    #[test]
    fn alignment_follows_attention_peaks_not_a_uniform_split() {
        let frames = 40;
        // Two equal tokens. A uniform split would open the second word at
        // half the frame axis. Attention puts the first word on an early blob,
        // so the boundary follows that blob.
        let heads = spike(4, frames, &[(0, 0), (1, 2), (2, 30)]);
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
            },
            MeasuredWord {
                text: "dwa".into(),
                start_secs: 0.2,
                end_secs: 0.4,
            },
            MeasuredWord {
                text: ",".into(),
                start_secs: 0.4,
                end_secs: 0.46,
            },
        ];
        merge_word_punctuation(&mut words);
        assert_eq!(words.len(), 2);
        assert_eq!(words[1].text, "dwa,");
        assert_eq!(words[1].start_secs, 0.2);
        assert_eq!(words[1].end_secs, 0.46);
    }

    #[test]
    fn empty_attention_does_not_invent_ranges() {
        assert!(align_measured_words(&[], 1, &[1], &["raz"], 10).is_none());
    }
}
