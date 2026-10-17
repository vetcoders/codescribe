//! Synthetic cross-attention oracle checks, authored by the integrator.
//! No decoder, PCM capture, ledger, seal or delivery is exercised here.

use super::*;

fn cases() -> serde_json::Value {
    serde_json::from_str(include_str!("word_pins_oracle.json")).expect("fixed oracle fixtures")
}

fn check_oracle(name: &str) {
    let fixtures = cases();
    let case = fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .expect("named fixture");
    let heads: Vec<Vec<Vec<f32>>> = serde_json::from_value(case["heads"].clone()).unwrap();
    let counts: Vec<usize> = serde_json::from_value(case["counts"].clone()).unwrap();
    let words: Vec<String> = serde_json::from_value(case["words"].clone()).unwrap();
    let labels: Vec<&str> = words.iter().map(String::as_str).collect();
    let expected: Vec<[usize; 2]> =
        serde_json::from_value(case["expected_frames"].clone()).unwrap();
    let actual = align_measured_words(
        &heads,
        case["prefix"].as_u64().unwrap() as usize,
        &counts,
        &labels,
        case["content_frames"].as_u64().unwrap() as usize,
    )
    .expect("nondegenerate reference has measured pins");
    assert_eq!(actual.len(), expected.len(), "{name}: occurrence count");
    let mut token_start = 0;
    for (index, (word, [start, end])) in actual.iter().zip(expected).enumerate() {
        assert_eq!(word.text, labels[index], "{name}: labels and order");
        assert_eq!(
            (word.token_start, word.token_len),
            (token_start, counts[index])
        );
        token_start += counts[index];
        // The upstream oracle hard-codes the first DTW entry to frame zero.
        // Every checked-in fixture deliberately opens its first predictive
        // attention blob at frame 6. Interior and closing jumps stay exact.
        let start_secs = if index == 0 {
            6.0 / 50.0
        } else {
            start as f32 / 50.0
        };
        let end_secs = end as f32 / 50.0;
        assert!(
            (word.start_secs - start_secs).abs() < 0.000_01,
            "{name}[{index}]: start {} != oracle {start_secs}",
            word.start_secs
        );
        assert!(
            (word.end_secs - end_secs).abs() < 0.000_01,
            "{name}[{index}]: end {} != oracle {end_secs}",
            word.end_secs
        );
    }
}

#[test]
fn oracle_single_final_word_before_silence() {
    check_oracle("single_final_word_before_silence");
}

#[test]
fn oracle_five_iwo_occurrences_remain_five() {
    check_oracle("five_iwo_occurrences");
}

#[test]
fn oracle_last_word_three_tokens() {
    check_oracle("last_word_three_tokens");
}

#[test]
fn oracle_two_alignment_heads() {
    check_oracle("two_alignment_heads");
}

#[test]
fn oracle_padded_frames_are_excluded() {
    check_oracle("padded_frames_are_excluded");
}

#[test]
fn oracle_long_trailing_silence() {
    check_oracle("long_trailing_silence");
}

#[test]
fn oracle_uniform_attention_does_not_manufacture_single_word_span() {
    let heads = vec![vec![vec![0.0; 120]; 6]];
    assert!(align_measured_words(&heads, 3, &[1], &["rację?"], 120).is_none());
}

#[test]
fn oracle_malformed_shapes_and_word_boundaries_are_refused() {
    let valid = vec![vec![vec![0.0; 20]; 6]];
    let mut inconsistent_heads = valid.clone();
    inconsistent_heads.push(vec![vec![0.0; 20]; 5]);
    let mut short_row = valid.clone();
    short_row[0][4].truncate(2);
    for (heads, prefix, counts, words, frames) in [
        (Vec::new(), 3, vec![1], vec!["raz"], 20),
        (valid.clone(), 3, vec![1], vec!["raz"], 0),
        (vec![Vec::new()], 3, vec![1], vec!["raz"], 20),
        (vec![vec![Vec::new(); 6]], 3, vec![1], vec!["raz"], 20),
        (inconsistent_heads, 3, vec![1], vec!["raz"], 20),
        (short_row, 3, vec![1], vec!["raz"], 20),
        (valid.clone(), 3, vec![2], vec!["raz"], 20),
        (valid.clone(), 3, vec![0, 1], vec!["raz", "dwa"], 20),
        (valid.clone(), 3, vec![1], vec![" "], 20),
        (valid, 3, vec![1], Vec::new(), 20),
    ] {
        assert!(align_measured_words(&heads, prefix, &counts, &words, frames).is_none());
    }
}

#[test]
fn oracle_all_nonfinite_attention_is_refused() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let heads = vec![vec![vec![value; 20]; 6]];
        assert!(align_measured_words(&heads, 3, &[1], &["rację?"], 20).is_none());
    }
}

#[test]
fn oracle_overlapping_predictive_jumps_drop_only_the_zero_duration_word() {
    // Pinned reference produces boundaries [0, 0, 7] on the previous fixture:
    // the first word has no duration. That word cannot become a measured
    // occurrence. Its neighbour, measured over frames 0..7, still can.
    //
    // Counterexample from dragon, 2026-10-13 09:12Z, take 018e2177: the
    // window [0, 9) s decoded "Doprecyzuj jaki session id Juni jest dopięty na
    // konferencji." and word 8 came back with end == start. The whole window
    // was returned phrase-grain, the ledger refused it as
    // overlap_without_word_pins, and "Doprecyzuj jaki" never reached the
    // user, although no other window covered that speech. On dragon 12.9 %
    // of the day's windows (64/64 refusals = nonpositive_word_duration) lost
    // every pin this way; 15.4 % the day before.
    let mut matrix = vec![vec![0.0; 40]; 4];
    for (row, start) in [(0, 0), (1, 2), (2, 30)] {
        matrix[row][start..start + 5].fill(8.0);
    }
    let words = align_measured_words(&[matrix], 0, &[1, 1], &["raz", "dwa"], 40)
        .expect("one zero-length word must not void the measured neighbour");
    assert!(
        words.iter().all(|word| word.end_secs > word.start_secs),
        "no returned word may have a nonpositive duration: {words:?}"
    );
    assert_eq!(
        words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>(),
        ["dwa"],
        "only the measured neighbour owns a word pin; the caller retains the independent raw hypothesis"
    );
    assert_eq!(words[0].token_start, 1);
    assert_eq!(
        words[0].token_len, 1,
        "unaligned tokens cannot inherit measured confidence"
    );
}
