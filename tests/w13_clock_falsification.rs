//! W13-0 clock falsification: golden fixture identity + clock-truth histograms.
//!
//! Test-only. No production behavior change.
//!
//! The named loader (`w13_golden_fixture_manifest_loads`) is hermetic: it
//! asserts the committed manifest shape and never requires private audio.
//! When the operator corpus is present (`CODESCRIBE_DATA_ASSETS` or
//! `~/.codescribe/data_assets`), a second test measures digital-zero regions
//! and the `extract_speech` timebase warp. Transcript text is never logged.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/w13_clock.rs"]
mod w13_clock;
use w13_clock::{DurationBucket, duration_buckets, histogram_apple_word_spans};

const MANIFEST_REL: &str = "tests/fixtures/w13_golden_manifest.json";
const CLOCK_LIES_REL: &str = "tests/fixtures/w13_clock_lies.md";
const EXPECTED_TAKE_IDS: [&str; 3] = ["171939", "191351", "193523"];
/// 500 ms windows — matches `core/vad/mod.rs::EXTRACT_WINDOW_MS`.
const EXTRACT_WINDOW_MS: u32 = 500;
/// Ignore single-sample dropouts when naming a "silence region".
const MIN_ZERO_REGION_SAMPLES: usize = 16;

#[derive(Debug, Deserialize)]
struct GoldenManifest {
    schema: String,
    cut: String,
    language: String,
    takes: Vec<GoldenTake>,
}

#[derive(Debug, Deserialize)]
struct GoldenTake {
    id: String,
    slug: String,
    fixture: String,
    wav_sha256: String,
    sample_rate: u32,
    sample_count: u64,
    duration_secs: f64,
    #[serde(default)]
    mic_regions: Vec<MicRegion>,
}

#[derive(Debug, Deserialize)]
struct MicRegion {
    mode: String,
    sample_start_secs: f64,
    sample_end_secs: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct DigitalZeroReport {
    label: String,
    sample_start: u64,
    sample_end: u64,
    sample_count: u64,
    zero_samples: u64,
    zero_ratio: f64,
    region_count: usize,
    longest_region_samples: u64,
    region_histogram: Vec<DurationBucket>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct CompactionReport {
    take_id: String,
    original_samples: u64,
    compacted_samples: u64,
    dropped_samples: u64,
    dropped_ratio: f64,
    speech_windows: usize,
    total_windows: usize,
    interior_drop_runs: usize,
    max_interior_gap_samples: u64,
    max_naive_warp_secs: f64,
    no_speech_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TakeClockReport {
    take_id: String,
    fixture: String,
    sample_rate: u32,
    sample_count: u64,
    digital_zero: DigitalZeroReport,
    mic_regions: Vec<DigitalZeroReport>,
    compaction: CompactionReport,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_manifest() -> GoldenManifest {
    let path = repo_root().join(MANIFEST_REL);
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Same resolution order as `scripts/lib/data-assets.sh` / e2e helpers.
fn data_assets_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CODESCRIBE_DATA_ASSETS") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        let local = PathBuf::from(home).join(".codescribe/data_assets");
        if local.is_dir() {
            return local;
        }
    }
    repo_root().join("tests/assets/data_assets")
}

fn resolve_fixture(rel: &str) -> Option<PathBuf> {
    let path = data_assets_dir().join(rel);
    path.is_file().then_some(path)
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    format!("{:x}", hasher.finalize())
}

fn load_wav_mono(path: &Path) -> (Vec<f32>, u32) {
    let mut reader =
        hound::WavReader::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let spec = reader.spec();
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.expect("wav sample") as f32 / i16::MAX as f32)
            .collect(),
        (hound::SampleFormat::Int, 24 | 32) => reader
            .samples::<i32>()
            .map(|s| s.expect("wav sample") as f32 / i32::MAX as f32)
            .collect(),
        (hound::SampleFormat::Float, _) => reader
            .samples::<f32>()
            .map(|s| s.expect("wav sample"))
            .collect(),
        other => panic!("unsupported wav format on {}: {other:?}", path.display()),
    };
    let mono = if spec.channels > 1 {
        samples
            .chunks(spec.channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    } else {
        samples
    };
    (mono, spec.sample_rate)
}

fn digital_zero_regions(samples: &[f32], sample_rate: u32, label: &str) -> DigitalZeroReport {
    let mut regions = Vec::new();
    let mut i = 0;
    while i < samples.len() {
        if samples[i] != 0.0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < samples.len() && samples[i] == 0.0 {
            i += 1;
        }
        let len = i - start;
        if len >= MIN_ZERO_REGION_SAMPLES {
            regions.push(len);
        }
    }
    let zero_samples = samples.iter().filter(|s| **s == 0.0).count() as u64;
    DigitalZeroReport {
        label: label.to_string(),
        sample_start: 0,
        sample_end: samples.len() as u64,
        sample_count: samples.len() as u64,
        zero_samples,
        zero_ratio: if samples.is_empty() {
            0.0
        } else {
            zero_samples as f64 / samples.len() as f64
        },
        region_count: regions.len(),
        longest_region_samples: regions.iter().copied().max().unwrap_or(0) as u64,
        region_histogram: duration_buckets(&regions, sample_rate),
    }
}

fn slice_by_secs(samples: &[f32], sample_rate: u32, start_secs: f64, end_secs: f64) -> &[f32] {
    let sr = sample_rate.max(1) as f64;
    let start = ((start_secs * sr).floor() as usize).min(samples.len());
    let end = ((end_secs * sr).ceil() as usize)
        .min(samples.len())
        .max(start);
    &samples[start..end]
}

/// Reconstruct the compacted timebase from `extract_speech`'s sparkline.
///
/// `█`/`▓` = kept speech window; `░`/` ` = dropped. Each full window is
/// 500 ms. This is the mapping W13-3A must persist — today it is discarded.
fn compaction_timebase(take_id: &str, samples: &[f32], sample_rate: u32) -> CompactionReport {
    let (compacted, stats) = codescribe_core::vad::extract_speech(samples, sample_rate);
    let window_size = (sample_rate.saturating_mul(EXTRACT_WINDOW_MS) / 1000) as usize;
    let mut interior_drop_runs = 0usize;
    let mut max_interior_gap = 0usize;
    let mut in_drop = false;
    let mut current_gap = 0usize;
    let mut seen_speech = false;
    let mut trailing = false;
    for ch in stats.sparkline.chars() {
        let kept = ch == '\u{2588}' || ch == '\u{2593}';
        if kept {
            if in_drop && seen_speech {
                interior_drop_runs += 1;
                max_interior_gap = max_interior_gap.max(current_gap);
            }
            in_drop = false;
            current_gap = 0;
            seen_speech = true;
            trailing = false;
        } else {
            if !in_drop {
                in_drop = true;
                current_gap = 0;
            }
            current_gap += window_size;
            trailing = seen_speech;
        }
    }
    if trailing && in_drop {
        // trailing silence is not an interior gap
    }
    let dropped = samples.len().saturating_sub(compacted.len()) as u64;
    CompactionReport {
        take_id: take_id.to_string(),
        original_samples: samples.len() as u64,
        compacted_samples: compacted.len() as u64,
        dropped_samples: dropped,
        dropped_ratio: if samples.is_empty() {
            0.0
        } else {
            dropped as f64 / samples.len() as f64
        },
        speech_windows: stats.speech_windows,
        total_windows: stats.total_windows,
        interior_drop_runs,
        max_interior_gap_samples: max_interior_gap as u64,
        max_naive_warp_secs: if sample_rate == 0 {
            0.0
        } else {
            dropped as f64 / f64::from(sample_rate)
        },
        no_speech_reason: stats.no_speech_reason,
    }
}

fn measure_take(take: &GoldenTake, wav: &Path) -> TakeClockReport {
    let (samples, sample_rate) = load_wav_mono(wav);
    assert_eq!(
        sample_rate, take.sample_rate,
        "take {} sample_rate drifted: fixture {sample_rate} vs manifest {}",
        take.id, take.sample_rate
    );
    assert_eq!(
        samples.len() as u64,
        take.sample_count,
        "take {} sample_count drifted",
        take.id
    );
    let digital_zero = digital_zero_regions(&samples, sample_rate, "full-take");
    let mic_regions = take
        .mic_regions
        .iter()
        .map(|region| {
            let slice = slice_by_secs(
                &samples,
                sample_rate,
                region.sample_start_secs,
                region.sample_end_secs,
            );
            let mut report = digital_zero_regions(slice, sample_rate, &region.mode);
            report.sample_start =
                (region.sample_start_secs * f64::from(sample_rate)).floor() as u64;
            report.sample_end = (region.sample_end_secs * f64::from(sample_rate)).ceil() as u64;
            report
        })
        .collect();
    let compaction = compaction_timebase(&take.id, &samples, sample_rate);
    TakeClockReport {
        take_id: take.id.clone(),
        fixture: take.fixture.clone(),
        sample_rate,
        sample_count: samples.len() as u64,
        digital_zero,
        mic_regions,
        compaction,
    }
}

#[test]
fn w13_golden_fixture_manifest_loads() {
    // Pin so the operator dotenv cannot flip embed/model paths under this test.
    // SAFETY: test-only env pin; this integration test does not share the
    // process with production threads that read the same key.
    unsafe {
        std::env::set_var("CODESCRIBE_NO_EMBED", "1");
    }

    let manifest = load_manifest();
    assert_eq!(manifest.schema, "codescribe.w13.golden.v1");
    assert_eq!(manifest.cut, "w13-0-clock-falsification");
    assert_eq!(manifest.language, "pl-PL");
    assert_eq!(manifest.takes.len(), 3, "exactly the three evidence takes");

    let ids: Vec<&str> = manifest.takes.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, EXPECTED_TAKE_IDS);

    for take in &manifest.takes {
        assert!(
            take.fixture.starts_with("w13/w13_") && take.fixture.ends_with(".wav"),
            "take {} fixture must stay behind the data_assets fence: {}",
            take.id,
            take.fixture
        );
        assert_eq!(take.wav_sha256.len(), 64, "sha256 hex");
        assert!(take.sample_rate > 0);
        assert!(take.sample_count > 0);
        assert!(take.duration_secs > 0.0);
        assert!(!take.slug.is_empty());
    }

    let isolation = manifest
        .takes
        .iter()
        .find(|t| t.id == "191351")
        .expect("191351 present");
    assert_eq!(isolation.mic_regions.len(), 2);
    assert_eq!(isolation.mic_regions[0].mode, "standard");
    assert_eq!(isolation.mic_regions[1].mode, "voice_isolation");

    let lies = repo_root().join(CLOCK_LIES_REL);
    let lies_body = fs::read_to_string(&lies)
        .unwrap_or_else(|e| panic!("clock lies missing at {}: {e}", lies.display()));
    assert!(
        lies_body.contains("core/vad/mod.rs:99-123"),
        "W13-3A list must name the extract_speech concat"
    );
    assert!(
        lies_body.contains("bridge/src/recording.rs:686-707"),
        "W13-3A list must name the outbound segment drop"
    );
}

#[test]
fn w13_clock_histograms_from_golden_fixtures() {
    // SAFETY: test-only env pin; this integration test does not share the
    // process with production threads that read the same key.
    unsafe {
        std::env::set_var("CODESCRIBE_NO_EMBED", "1");
    }

    let manifest = load_manifest();
    let mut reports = Vec::new();
    let mut missing = Vec::new();
    for take in &manifest.takes {
        match resolve_fixture(&take.fixture) {
            None => missing.push(take.fixture.clone()),
            Some(path) => {
                let digest = sha256_file(&path);
                assert_eq!(
                    digest, take.wav_sha256,
                    "fixture {} sha256 drifted — refuse silent swap",
                    take.fixture
                );
                reports.push(measure_take(take, &path));
            }
        }
    }

    if reports.is_empty() {
        eprintln!(
            "w13_clock_histograms_from_golden_fixtures: SKIP — no golden WAVs under {} (missing: {missing:?})",
            data_assets_dir().display()
        );
        return;
    }

    assert!(
        missing.is_empty(),
        "partial golden set is not a measurement: missing {missing:?}"
    );

    let encoded = serde_json::to_string_pretty(&reports).expect("serialize histograms");
    let out_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root().join("target"));
    let _ = fs::create_dir_all(&out_dir);
    let out_path = out_dir.join("w13-0-clock-histograms.json");
    fs::write(&out_path, &encoded).unwrap_or_else(|e| panic!("write {}: {e}", out_path.display()));
    eprintln!("W13-0 histograms -> {}", out_path.display());
    for report in &reports {
        eprintln!(
            "take {} zeros={}/{} ({:.3}) regions={} compact_drop={:.3} interior_gaps={} max_warp_s={:.3}",
            report.take_id,
            report.digital_zero.zero_samples,
            report.digital_zero.sample_count,
            report.digital_zero.zero_ratio,
            report.digital_zero.region_count,
            report.compaction.dropped_ratio,
            report.compaction.interior_drop_runs,
            report.compaction.max_naive_warp_secs
        );
        for region in &report.mic_regions {
            eprintln!(
                "  mic {} zeros={}/{} ({:.3}) regions={} longest={}",
                region.label,
                region.zero_samples,
                region.sample_count,
                region.zero_ratio,
                region.region_count,
                region.longest_region_samples
            );
        }
    }

    let long = reports
        .iter()
        .find(|r| r.take_id == "191351")
        .expect("191351 measured");
    assert_eq!(long.mic_regions.len(), 2);
    // Digital-zero floors exist in BOTH mic modes (evidence §2). Refuse a
    // measurement that reports a silent Standard half as "no zeros".
    assert!(
        long.mic_regions.iter().all(|r| r.zero_samples > 0),
        "expected digital zeros in both mic modes of 191351"
    );
    assert!(
        long.compaction.dropped_samples > 0 || long.compaction.no_speech_reason.is_some(),
        "extract_speech on a 337 s take must either compact or report why not"
    );
}

#[test]
fn histogram_apple_word_spans_flags_overlap_and_restart() {
    let spans = [(0.0, 0.4), (0.3, 0.7), (0.1, 0.2)];
    let (hist, overlap, restarts) = histogram_apple_word_spans(&spans);
    assert_eq!(overlap, 2);
    assert!(restarts >= 1);
    assert_eq!(hist.iter().map(|b| b.count).sum::<usize>(), 3);
}

#[test]
fn digital_zero_regions_ignore_single_sample_dropouts() {
    let mut samples = vec![0.1_f32; 100];
    samples[10] = 0.0;
    samples[40..70].fill(0.0);
    let report = digital_zero_regions(&samples, 1000, "synth");
    assert_eq!(report.zero_samples, 31);
    assert_eq!(report.region_count, 1);
    assert_eq!(report.longest_region_samples, 30);
}
