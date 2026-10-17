//! Speech islands: the accounting unit between word clamps.
//!
//! An island is Silero speech joined across pauses shorter than
//! [`SPEECH_ISLAND_GAP_MS`]. Its clamps are its first and last speech sample,
//! taken from the acoustic speech set the seal coverage reads (Silero ranges
//! padded by `ACOUSTIC_SPEECH_PAD_SECS`). Inside the clamps this counts the
//! word starts heard in the capture energy (syllable nuclei) and the pauses;
//! outside them it classes the silence. The take-truth observer adds the words
//! written over each island, so the two columns can be compared.
//!
//! Everything lives on the capture sample axis (`capture_epoch` + capture
//! samples) and nothing here reads transcript text. A nucleus is a lower bound
//! on syllables, not a word count: fast short words can share one.
//!
//! The nucleus detector is the voice-lab take report's
//! (`reports/take_report.py`) on the capture energy hops: centred mean energy
//! over about 50 ms, floor at the 10th percentile of hops that are not digital
//! zero, bursts 10 dB over that floor, and nuclei 6 dB over the burst
//! threshold that stand 4 dB over the dip to every kept nucleus and lie at
//! least 120 ms apart.

use serde::{Deserialize, Serialize};

use super::acoustic_ledger::SilenceUnderSpan;
use crate::audio::capture_receipt::{DIGITAL_ZERO_ABS, EnergyHopLevel};

/// Pauses shorter than this join speech into one island. Same G as the div0
/// census of 17 X 2026.
pub const SPEECH_ISLAND_GAP_MS: u64 = 400;

const SMOOTH_MS: f64 = 50.0;
const SPEECH_OVER_FLOOR_DB: f64 = 10.0;
const BURST_GAP_MS: f64 = 120.0;
const BURST_MIN_MS: f64 = 60.0;
const NUCLEUS_OVER_THRESHOLD_DB: f64 = 6.0;
const NUCLEUS_PROMINENCE_DB: f64 = 4.0;
const NUCLEUS_SEPARATION_MS: f64 = 120.0;
/// One 16-bit step squared: the take report's energy floor.
const ENERGY_FLOOR: f64 = 1.0 / (32_768.0 * 32_768.0);

/// Silence beside an island: its length and what the energy holds there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandSilence {
    pub samples: u64,
    pub silence: SilenceUnderSpan,
}

/// One island of speech between its clamps, without words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechIsland {
    /// Opening clamp: first speech sample.
    pub sample_start: u64,
    /// Closing clamp: one past the last speech sample.
    pub sample_end: u64,
    /// Syllable nuclei whose hop starts inside the clamps.
    pub onsets: u32,
    /// Pauses between the speech ranges this island joined.
    pub inner_pauses: u32,
    /// Silence from the previous island (or capture start) to this one.
    pub silence_before: Option<IslandSilence>,
    /// Silence from this island to the next one (or the observed end).
    pub silence_after: Option<IslandSilence>,
}

/// The speech side of one capture's island accounting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechIslandEvidence {
    pub session_id: String,
    pub capture_epoch: u64,
    pub sample_rate_hz: u32,
    pub observed_samples: u64,
    pub gap_ms: u64,
    /// Smoothed level a burst must reach; `None` when the energy clock could
    /// not be read, and then no island has onsets.
    pub onset_threshold_dbfs: Option<f32>,
    /// Level at or above which silence counts as sound without speech: the
    /// calibrated existence floor, else the onset threshold.
    pub sound_floor_dbfs: Option<f32>,
    pub islands: Vec<SpeechIsland>,
}

/// Everything [`measure_speech_islands`] reads.
pub struct SpeechIslandInput<'a> {
    pub session_id: &'a str,
    pub capture_epoch: u64,
    pub sample_rate: u32,
    pub observed_samples: u64,
    /// Silero speech ranges on the capture axis.
    pub speech: &'a [(u64, u64)],
    /// The capture's energy hops, `None` when the clock cannot speak.
    pub hops: Option<&'a [EnergyHopLevel]>,
    /// Calibrated existence floor (linear RMS), when the device has one.
    pub existence_floor_rms: Option<f32>,
}

/// Join Silero speech into islands and count what lies between the clamps.
pub fn measure_speech_islands(input: SpeechIslandInput<'_>) -> SpeechIslandEvidence {
    let gap_samples = u64::from(input.sample_rate).saturating_mul(SPEECH_ISLAND_GAP_MS) / 1_000;
    let mut speech: Vec<(u64, u64)> = input
        .speech
        .iter()
        .map(|&(start, end)| (start, end.min(input.observed_samples)))
        .filter(|(start, end)| end > start)
        .collect();
    speech.sort_unstable();

    // (start, end, inner pauses)
    let mut spans: Vec<(u64, u64, u32)> = Vec::new();
    for (start, end) in speech {
        match spans.last_mut() {
            Some(last) if start < last.1.saturating_add(gap_samples) => {
                if start > last.1 {
                    last.2 += 1;
                }
                last.1 = last.1.max(end);
            }
            _ => spans.push((start, end, 0)),
        }
    }

    let nuclei = input
        .hops
        .and_then(|hops| syllable_nuclei(hops, input.sample_rate));
    let onset_threshold_dbfs = nuclei.as_ref().map(|(_, threshold)| *threshold as f32);
    let sound_floor_rms = input
        .existence_floor_rms
        .filter(|floor| floor.is_finite() && *floor > 0.0)
        .or_else(|| onset_threshold_dbfs.map(|db| 10_f32.powf(db / 20.0)));
    let silence = |start: u64, end: u64| -> Option<IslandSilence> {
        (end > start).then(|| IslandSilence {
            samples: end - start,
            silence: silence_class(input.hops, start, end, sound_floor_rms),
        })
    };

    let islands = spans
        .iter()
        .enumerate()
        .map(|(index, &(start, end, inner_pauses))| {
            let before_start = index.checked_sub(1).map_or(0, |prev| spans[prev].1);
            let after_end = spans
                .get(index + 1)
                .map_or(input.observed_samples, |next| next.0);
            let onsets = nuclei.as_ref().map_or(0, |(positions, _)| {
                positions
                    .iter()
                    .filter(|&&sample| sample >= start && sample < end)
                    .count()
            });
            SpeechIsland {
                sample_start: start,
                sample_end: end,
                onsets: u32::try_from(onsets).unwrap_or(u32::MAX),
                inner_pauses,
                silence_before: silence(before_start, start),
                silence_after: silence(end, after_end),
            }
        })
        .collect();

    SpeechIslandEvidence {
        session_id: input.session_id.to_string(),
        capture_epoch: input.capture_epoch,
        sample_rate_hz: input.sample_rate,
        observed_samples: input.observed_samples,
        gap_ms: SPEECH_ISLAND_GAP_MS,
        onset_threshold_dbfs,
        sound_floor_dbfs: sound_floor_rms.map(|rms| 20.0 * rms.log10()),
        islands,
    }
}

/// Words written over one island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IslandWords {
    pub words: u32,
    pub first_word_start: Option<u64>,
    pub last_word_end: Option<u64>,
}

/// Place each word on the island it overlaps most. Returns one entry per
/// island and the number of words that overlap no island.
pub fn words_per_island(
    islands: &[SpeechIsland],
    words: &[(u64, u64)],
) -> (Vec<IslandWords>, usize) {
    let mut placed = vec![IslandWords::default(); islands.len()];
    let mut outside = 0;
    for &(start, end) in words {
        let best = islands
            .iter()
            .enumerate()
            .map(|(index, island)| {
                let overlap = end
                    .min(island.sample_end)
                    .saturating_sub(start.max(island.sample_start));
                (overlap, index)
            })
            .filter(|(overlap, _)| *overlap > 0)
            .max_by_key(|(overlap, index)| (*overlap, std::cmp::Reverse(*index)));
        let Some((_, index)) = best else {
            outside += 1;
            continue;
        };
        let slot = &mut placed[index];
        slot.words += 1;
        slot.first_word_start = Some(
            slot.first_word_start
                .map_or(start, |first| first.min(start)),
        );
        slot.last_word_end = Some(slot.last_word_end.map_or(end, |last| last.max(end)));
    }
    (placed, outside)
}

fn silence_class(
    hops: Option<&[EnergyHopLevel]>,
    start: u64,
    end: u64,
    sound_floor_rms: Option<f32>,
) -> SilenceUnderSpan {
    let Some(hops) = hops else {
        return SilenceUnderSpan::EnergyUnmeasured;
    };
    let mut inside = hops
        .iter()
        .filter(|hop| hop.sample_end > start && hop.sample_start < end)
        .peekable();
    if inside.peek().is_none() {
        return SilenceUnderSpan::EnergyUnmeasured;
    }
    let loudest = inside.map(|hop| hop.rms).fold(0.0_f32, f32::max);
    if loudest <= DIGITAL_ZERO_ABS {
        return SilenceUnderSpan::DigitalZero;
    }
    match sound_floor_rms {
        Some(floor) if loudest >= floor => SilenceUnderSpan::SoundWithoutSpeech,
        Some(_) => SilenceUnderSpan::BelowFloor,
        None => SilenceUnderSpan::EnergyUnmeasured,
    }
}

/// Syllable nuclei (as hop start samples) and the burst threshold in dBFS.
/// `None` when no hop rises above digital zero.
fn syllable_nuclei(hops: &[EnergyHopLevel], sample_rate: u32) -> Option<(Vec<u64>, f64)> {
    let first = hops.first()?;
    let hop_ms = first.sample_end.saturating_sub(first.sample_start) as f64 * 1_000.0
        / f64::from(sample_rate.max(1));
    if hop_ms <= 0.0 {
        return None;
    }
    let in_hops = |ms: f64| (ms / hop_ms).round() as usize;
    let half = in_hops(SMOOTH_MS) / 2;
    let squares: Vec<f64> = hops.iter().map(|hop| f64::from(hop.rms).powi(2)).collect();
    let mut prefix = Vec::with_capacity(squares.len() + 1);
    prefix.push(0.0);
    for square in &squares {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + square);
    }
    let smooth: Vec<f64> = (0..squares.len())
        .map(|index| {
            let low = index.saturating_sub(half);
            let high = (index + half + 1).min(squares.len());
            let energy = (prefix[high] - prefix[low]) / (high - low) as f64;
            10.0 * energy.max(ENERGY_FLOOR).log10()
        })
        .collect();

    let audible: Vec<f64> = hops
        .iter()
        .zip(&smooth)
        .filter(|(hop, _)| hop.rms > DIGITAL_ZERO_ABS)
        .map(|(_, level)| *level)
        .collect();
    let threshold = percentile(audible, 0.10)? + SPEECH_OVER_FLOOR_DB;

    let mut bursts: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < smooth.len() {
        if smooth[index] < threshold {
            index += 1;
            continue;
        }
        let start = index;
        while index < smooth.len() && smooth[index] >= threshold {
            index += 1;
        }
        match bursts.last_mut() {
            Some(last) if start - last.1 < in_hops(BURST_GAP_MS) => last.1 = index,
            _ => bursts.push((start, index)),
        }
    }
    let shortest = in_hops(BURST_MIN_MS);
    let separation = in_hops(NUCLEUS_SEPARATION_MS).max(1);
    let level = threshold + NUCLEUS_OVER_THRESHOLD_DB;

    let mut nuclei = Vec::new();
    for &(start, end) in bursts.iter().filter(|(start, end)| end - start >= shortest) {
        let mut candidates: Vec<usize> = (start..end)
            .filter(|&i| {
                smooth[i] >= level
                    && (i == start || smooth[i] >= smooth[i - 1])
                    && (i + 1 == end || smooth[i] > smooth[i + 1])
            })
            .collect();
        candidates.sort_by(|a, b| smooth[*b].total_cmp(&smooth[*a]));
        let mut kept: Vec<usize> = Vec::new();
        for candidate in candidates {
            let stands = kept.iter().all(|&other| {
                let (low, high) = (candidate.min(other), candidate.max(other));
                let dip = smooth[low..=high]
                    .iter()
                    .copied()
                    .fold(f64::INFINITY, f64::min);
                high - low >= separation && smooth[candidate] - dip >= NUCLEUS_PROMINENCE_DB
            });
            if stands {
                kept.push(candidate);
            }
        }
        nuclei.extend(kept.into_iter().map(|i| hops[i].sample_start));
    }
    nuclei.sort_unstable();
    Some((nuclei, threshold))
}

/// Linear-interpolated percentile, as the take report computes it.
fn percentile(mut values: Vec<f64>, q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let k = (values.len() - 1) as f64 * q;
    let low = k.floor() as usize;
    let high = (low + 1).min(values.len() - 1);
    Some(values[low] + (values[high] - values[low]) * (k - low as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;
    const HOP: u64 = 320; // 20 ms

    /// One hop per level, tiling from sample 0.
    fn hops(levels: &[f32]) -> Vec<EnergyHopLevel> {
        levels
            .iter()
            .enumerate()
            .map(|(i, &rms)| EnergyHopLevel {
                sample_start: i as u64 * HOP,
                sample_end: (i as u64 + 1) * HOP,
                rms,
            })
            .collect()
    }

    fn input<'a>(
        speech: &'a [(u64, u64)],
        hops: Option<&'a [EnergyHopLevel]>,
        observed: u64,
    ) -> SpeechIslandInput<'a> {
        SpeechIslandInput {
            session_id: "islands",
            capture_epoch: 1,
            sample_rate: RATE,
            observed_samples: observed,
            speech,
            hops,
            existence_floor_rms: None,
        }
    }

    /// Room noise at 0.001 with three syllables at 0.3, each 4 hops wide,
    /// separated by 8 quiet hops: three nuclei 240 ms apart.
    fn three_syllables() -> Vec<f32> {
        let mut levels = vec![0.001_f32; 10];
        for _ in 0..3 {
            levels.extend([0.3_f32; 4]);
            levels.extend([0.001_f32; 8]);
        }
        levels.extend([0.001_f32; 10]);
        levels
    }

    #[test]
    fn pauses_shorter_than_the_gap_join_and_are_counted_inside_the_island() {
        let gap = u64::from(RATE) * SPEECH_ISLAND_GAP_MS / 1_000;
        let speech = [
            (1_000, 2_000),
            (2_000 + gap - 1, 9_000),
            (9_000 + gap, 20_000),
        ];
        let evidence = measure_speech_islands(input(&speech, None, 30_000));
        let spans: Vec<_> = evidence
            .islands
            .iter()
            .map(|island| (island.sample_start, island.sample_end, island.inner_pauses))
            .collect();
        assert_eq!(
            spans,
            vec![(1_000, 9_000, 1), (9_000 + gap, 20_000, 0)],
            "a pause one sample short of the gap joins; a pause of exactly the gap splits"
        );
        assert_eq!(evidence.gap_ms, SPEECH_ISLAND_GAP_MS);
    }

    #[test]
    fn three_separated_syllables_are_three_onsets_on_their_island() {
        let levels = three_syllables();
        let hops = hops(&levels);
        let end = hops.last().unwrap().sample_end;
        let speech = [(10 * HOP, 44 * HOP)];
        let evidence = measure_speech_islands(input(&speech, Some(&hops), end));
        assert_eq!(evidence.islands.len(), 1);
        assert_eq!(evidence.islands[0].onsets, 3);
        assert!(evidence.onset_threshold_dbfs.is_some());
    }

    #[test]
    fn a_syllable_outside_every_island_is_not_counted_on_one() {
        let levels = three_syllables();
        let hops = hops(&levels);
        let end = hops.last().unwrap().sample_end;
        // Only the first syllable is speech; the other two lie in its silence.
        let speech = [(10 * HOP, 14 * HOP)];
        let evidence = measure_speech_islands(input(&speech, Some(&hops), end));
        assert_eq!(evidence.islands[0].onsets, 1);
        let after = evidence.islands[0].silence_after.unwrap();
        assert_eq!(after.samples, end - 14 * HOP);
        assert_eq!(after.silence, SilenceUnderSpan::SoundWithoutSpeech);
    }

    #[test]
    fn silence_beside_an_island_is_classed_by_its_energy() {
        // zero | quiet room | speech | zero
        let mut levels = vec![0.0_f32; 10];
        levels.extend([0.001_f32; 10]);
        levels.extend([0.3_f32; 5]);
        levels.extend([0.0_f32; 10]);
        let hops = hops(&levels);
        let end = hops.last().unwrap().sample_end;
        let speech = [(20 * HOP, 25 * HOP)];
        let mut measured = input(&speech, Some(&hops), end);
        measured.existence_floor_rms = Some(0.01);
        let evidence = measure_speech_islands(measured);
        let island = &evidence.islands[0];
        assert_eq!(
            island.silence_before,
            Some(IslandSilence {
                samples: 20 * HOP,
                silence: SilenceUnderSpan::BelowFloor,
            })
        );
        assert_eq!(
            island.silence_after,
            Some(IslandSilence {
                samples: 10 * HOP,
                silence: SilenceUnderSpan::DigitalZero,
            })
        );
        assert!((evidence.sound_floor_dbfs.unwrap() - -40.0).abs() < 0.01);
    }

    #[test]
    fn an_unreadable_energy_clock_counts_no_onsets_and_measures_no_silence() {
        let speech = [(1_000, 5_000)];
        let evidence = measure_speech_islands(input(&speech, None, 10_000));
        let island = &evidence.islands[0];
        assert_eq!(island.onsets, 0);
        assert_eq!(evidence.onset_threshold_dbfs, None);
        assert_eq!(evidence.sound_floor_dbfs, None);
        assert_eq!(
            island.silence_before.unwrap().silence,
            SilenceUnderSpan::EnergyUnmeasured
        );
    }

    #[test]
    fn speech_touching_the_capture_edges_leaves_no_edge_silence() {
        let speech = [(0, 10_000)];
        let evidence = measure_speech_islands(input(&speech, None, 10_000));
        assert_eq!(evidence.islands[0].silence_before, None);
        assert_eq!(evidence.islands[0].silence_after, None);
    }

    #[test]
    fn each_word_lands_on_the_island_it_overlaps_most_or_outside() {
        let island = |start, end| SpeechIsland {
            sample_start: start,
            sample_end: end,
            onsets: 0,
            inner_pauses: 0,
            silence_before: None,
            silence_after: None,
        };
        let islands = [island(1_000, 5_000), island(12_000, 20_000)];
        let words = [
            (900, 2_000),
            (4_500, 13_000),
            (13_000, 14_000),
            (30_000, 31_000),
        ];
        let (placed, outside) = words_per_island(&islands, &words);
        assert_eq!(outside, 1, "the word over silence is outside every island");
        assert_eq!(
            placed[0],
            IslandWords {
                words: 1,
                first_word_start: Some(900),
                last_word_end: Some(2_000),
            }
        );
        assert_eq!(
            placed[1],
            IslandWords {
                words: 2,
                first_word_start: Some(4_500),
                last_word_end: Some(14_000),
            },
            "the straddling word belongs to the island it overlaps most"
        );
    }
}
