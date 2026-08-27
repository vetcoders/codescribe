//! Per-session capture-level receipt.
//!
//! The W30 input-level break (−38.3 → −43.9 dB, then −46.5 by W33) sat
//! invisible for three weeks because only per-buffer RMS ticks existed.
//! This receipt is the session aggregate: median RMS, peak, device, rate,
//! channels, plus the Amendment-3 active-speech key (sample count, clip,
//! dropout, noise, SNR). The all-audio median stays for debugging.
//!
//! WARN `capture_level_low` is a quality receipt. It must never join
//! [`USER_TERMINAL_WARNING_CODES`](crate::pipeline::contracts::USER_TERMINAL_WARNING_CODES).

use std::sync::{Mutex, OnceLock};

use tracing::{info, warn};

use crate::pipeline::contracts::{EngineEvent, EventSink};
use crate::stt::tail_provider::TailSampleRange;

/// Session-end receipt code (log line + last-snapshot key).
pub const CAPTURE_LEVEL_RECEIPT_CODE: &str = "capture_level_receipt";
/// Non-terminal WARN when the **active-speech** median sits below the floor.
pub const CAPTURE_LEVEL_LOW_CODE: &str = "capture_level_low";
/// Env override for the low-level floor (dBFS). Default −52.
pub const CAPTURE_LEVEL_LOW_DB_ENV: &str = "CODESCRIBE_CAPTURE_LEVEL_LOW_DB";
/// Corpus-derived floor: golden era ≈ −38, break ≈ −44, −52 leaves headroom.
pub const DEFAULT_CAPTURE_LEVEL_LOW_DB: f32 = -52.0;
/// macOS 27 gates silence to hard zeros (take 191351, both mic modes).
pub const DIGITAL_ZERO_ABS: f32 = 1.0e-8;
/// Linear RMS below this is not active speech (~−80 dBFS).
pub const ACTIVE_SPEECH_LINEAR_FLOOR: f32 = 1.0e-4;
/// Adjacent active hops separated only by an ordinary word-edge pause stay one
/// measured speech span. Seal coverage separately tolerates a smaller 250 ms
/// uncovered edge; this merge is not transcript-dependent.
pub const ACTIVE_SPEECH_MERGE_GAP_MS: u64 = 200;
/// Near-full-scale samples count as clipping.
pub const CLIP_ABS: f32 = 0.99;

static LAST_RECEIPT: OnceLock<Mutex<Option<CaptureLevelReceipt>>> = OnceLock::new();
static LAST_OPEN_PATH: OnceLock<Mutex<Option<CapturePathMeta>>> = OnceLock::new();
static SESSION_ENERGY: OnceLock<Mutex<SessionEnergyClock>> = OnceLock::new();

/// One capture hop on the session PCM axis. Intensity lives here, not on tokens.
#[derive(Debug, Clone, Copy)]
struct EnergyHop {
    sample_start: u64,
    sample_end: u64,
    rms: f32,
}

#[derive(Debug, Default)]
struct SessionEnergyClock {
    hops: Vec<EnergyHop>,
}

fn last_receipt_slot() -> &'static Mutex<Option<CaptureLevelReceipt>> {
    LAST_RECEIPT.get_or_init(|| Mutex::new(None))
}

fn last_open_path_slot() -> &'static Mutex<Option<CapturePathMeta>> {
    LAST_OPEN_PATH.get_or_init(|| Mutex::new(None))
}

fn session_energy_slot() -> &'static Mutex<SessionEnergyClock> {
    SESSION_ENERGY.get_or_init(|| Mutex::new(SessionEnergyClock::default()))
}

/// Open a new capture epoch's energy ladder. Call at live-session start only.
pub fn begin_session_energy_clock() {
    *session_energy_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = SessionEnergyClock::default();
}

fn record_session_energy_hop(sample_start: u64, sample_end: u64, rms: f32) {
    if sample_end <= sample_start || !rms.is_finite() || rms < 0.0 {
        return;
    }
    session_energy_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .hops
        .push(EnergyHop {
            sample_start,
            sample_end,
            rms,
        });
}

/// Mean RMS of hops overlapping `[sample_start, sample_end)`, as dBFS.
///
/// Missing hops, inverted ranges, or a silent window return `None`. This is
/// intensity on the PCM clock — not a confidence score.
pub fn session_energy_db(sample_start: u64, sample_end: u64) -> Option<f32> {
    if sample_end <= sample_start {
        return None;
    }
    let hops = session_energy_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut weighted = 0.0_f64;
    let mut covered = 0.0_f64;
    for hop in &hops.hops {
        let lo = hop.sample_start.max(sample_start);
        let hi = hop.sample_end.min(sample_end);
        if hi <= lo {
            continue;
        }
        let width = (hi - lo) as f64;
        weighted += f64::from(hop.rms) * width;
        covered += width;
    }
    if covered <= 0.0 {
        return None;
    }
    let db = linear_to_db((weighted / covered) as f32);
    db.is_finite().then_some(db)
}

/// Active-speech ranges already measured by the streaming capture energy
/// ladder. This exposes the existing detector on the canonical PCM clock; it
/// does not run a second VAD or inspect transcript text.
pub fn session_active_speech_ranges(
    session: &str,
    capture_epoch: u64,
    sample_rate: u32,
) -> Vec<TailSampleRange> {
    let merge_gap = u64::from(sample_rate).saturating_mul(ACTIVE_SPEECH_MERGE_GAP_MS) / 1_000;
    let hops = session_energy_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for hop in hops
        .hops
        .iter()
        .filter(|hop| hop.rms >= ACTIVE_SPEECH_LINEAR_FLOOR)
    {
        if let Some((_, previous_end)) = ranges.last_mut()
            && hop.sample_start <= previous_end.saturating_add(merge_gap)
        {
            *previous_end = (*previous_end).max(hop.sample_end);
        } else {
            ranges.push((hop.sample_start, hop.sample_end));
        }
    }
    ranges
        .into_iter()
        .map(|(sample_start, sample_end)| TailSampleRange {
            session: session.to_string(),
            capture_epoch,
            sample_start,
            sample_end,
        })
        .collect()
}

/// Remember the live capture path (device / rate / channels) without a new TCC prompt.
pub fn publish_open_capture_path(meta: CapturePathMeta) {
    *last_open_path_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(meta);
}

/// Last opened capture path, if the recorder published one this process.
pub fn last_open_capture_path() -> Option<CapturePathMeta> {
    last_open_path_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Last finalized capture receipt in this process, if any.
pub fn last_capture_level_receipt() -> Option<CaptureLevelReceipt> {
    last_receipt_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn remember_last(receipt: &CaptureLevelReceipt) {
    *last_receipt_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(receipt.clone());
}

/// Convert a linear RMS (0..~1) to dBFS. Non-positive values are −∞.
pub fn linear_to_db(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 0.0 {
        return f32::NEG_INFINITY;
    }
    20.0 * linear.log10()
}

/// Convert dBFS back to linear amplitude.
pub fn db_to_linear(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    10.0_f32.powf(db / 20.0)
}

/// Low-level floor, env-overridable. Invalid / missing env keeps the default.
pub fn capture_level_low_db() -> f32 {
    match std::env::var(CAPTURE_LEVEL_LOW_DB_ENV) {
        Ok(raw) => raw
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v < 0.0)
            .unwrap_or(DEFAULT_CAPTURE_LEVEL_LOW_DB),
        Err(_) => DEFAULT_CAPTURE_LEVEL_LOW_DB,
    }
}

/// Input-path identity attached at finalize (seconds stay at adapters).
#[derive(Debug, Clone, PartialEq)]
pub struct CapturePathMeta {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
}

impl CapturePathMeta {
    /// Device from the already-open capture path / `AUDIO_INPUT_DEVICE`.
    /// Never opens a new Core Audio query — no new permission prompt.
    pub fn from_open_path(sample_rate: u32, channels: u16, device_name: Option<&str>) -> Self {
        let device_name = device_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                std::env::var("AUDIO_INPUT_DEVICE")
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "system_default".to_string());
        Self {
            device_name,
            sample_rate,
            channels: channels.max(1),
        }
    }

    /// Prefer the already-open recorder path; fall back to env / defaults.
    pub fn resolve(sample_rate: u32, channels: u16, device_name: Option<&str>) -> Self {
        match last_open_capture_path() {
            Some(open) => Self {
                device_name: device_name
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or(open.device_name),
                sample_rate: if sample_rate > 0 {
                    sample_rate
                } else {
                    open.sample_rate
                },
                channels: channels.max(open.channels).max(1),
            },
            None => Self::from_open_path(sample_rate, channels, device_name),
        }
    }
}

/// Running per-buffer capture stats. Cheap enough for the CoreAudio callback.
#[derive(Debug, Default)]
pub struct CaptureLevelAccumulator {
    sample_count: u64,
    digital_zero_samples: u64,
    clipping_samples: u64,
    peak_linear: f32,
    all_block_rms: Vec<f32>,
    speech_block_rms: Vec<f32>,
    noise_block_rms: Vec<f32>,
    dropout_blocks: u64,
    seen_speech: bool,
    trailing_zero_run: u64,
}

impl CaptureLevelAccumulator {
    /// Empty accumulator for one session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one captured block (mono f32, already downmixed).
    pub fn push_samples(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut sum_sq = 0.0_f64;
        let mut zeros = 0_u64;
        let mut clips = 0_u64;
        let mut peak = 0.0_f32;
        for sample in samples {
            let x = if sample.is_finite() { *sample } else { 0.0 };
            let abs = x.abs();
            if abs <= DIGITAL_ZERO_ABS {
                zeros += 1;
            }
            if abs >= CLIP_ABS {
                clips += 1;
            }
            if abs > peak {
                peak = abs;
            }
            sum_sq += f64::from(x) * f64::from(x);
        }
        let rms = (sum_sq / samples.len() as f64).sqrt() as f32;
        let sample_start = self.sample_count;
        self.sample_count += samples.len() as u64;
        record_session_energy_hop(sample_start, self.sample_count, rms);
        self.digital_zero_samples += zeros;
        self.clipping_samples += clips;
        if peak > self.peak_linear {
            self.peak_linear = peak;
        }
        self.all_block_rms.push(rms);

        let all_digital_zero = zeros == samples.len() as u64;
        if rms >= ACTIVE_SPEECH_LINEAR_FLOOR && !all_digital_zero {
            if self.seen_speech && self.trailing_zero_run > 0 {
                self.dropout_blocks += self.trailing_zero_run;
            }
            self.speech_block_rms.push(rms);
            self.seen_speech = true;
            self.trailing_zero_run = 0;
        } else if all_digital_zero || rms <= DIGITAL_ZERO_ABS {
            if self.seen_speech {
                self.trailing_zero_run += 1;
            }
        } else {
            self.noise_block_rms.push(rms);
            self.trailing_zero_run = 0;
        }
    }

    /// Freeze the session receipt. `meta` is path identity, not a second clock.
    pub fn finalize(&self, meta: CapturePathMeta) -> CaptureLevelReceipt {
        let all_audio_median_db = median_db(&self.all_block_rms);
        let active_speech_median_db = median_db(&self.speech_block_rms);
        let noise_floor_db = median_db(&self.noise_block_rms);
        let peak_db = linear_to_db(self.peak_linear);
        let snr_db = if active_speech_median_db.is_finite() && noise_floor_db.is_finite() {
            Some(active_speech_median_db - noise_floor_db)
        } else {
            None
        };
        let threshold_db = capture_level_low_db();
        let low = !active_speech_median_db.is_finite() || active_speech_median_db < threshold_db;
        CaptureLevelReceipt {
            code: CAPTURE_LEVEL_RECEIPT_CODE,
            device_name: meta.device_name,
            sample_rate: meta.sample_rate,
            channels: meta.channels,
            sample_count: self.sample_count,
            digital_zero_samples: self.digital_zero_samples,
            active_speech_samples: self.speech_block_count_samples(),
            clipping_samples: self.clipping_samples,
            dropout_blocks: self.dropout_blocks,
            all_audio_median_db,
            active_speech_median_db,
            peak_db,
            noise_floor_db,
            snr_db,
            threshold_db,
            low,
        }
    }

    fn speech_block_count_samples(&self) -> u64 {
        // Block size is not uniform; report the speech-block count as a
        // sample-adjacent figure via the digital-zero complement when possible.
        self.sample_count.saturating_sub(self.digital_zero_samples)
    }
}

fn median_db(values: &[f32]) -> f32 {
    if values.is_empty() {
        return f32::NEG_INFINITY;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    let linear = if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) * 0.5
    } else {
        sorted[mid]
    };
    linear_to_db(linear)
}

/// Frozen session receipt. WARN is keyed on `active_speech_median_db`.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureLevelReceipt {
    pub code: &'static str,
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_count: u64,
    pub digital_zero_samples: u64,
    pub active_speech_samples: u64,
    pub clipping_samples: u64,
    pub dropout_blocks: u64,
    pub all_audio_median_db: f32,
    pub active_speech_median_db: f32,
    pub peak_db: f32,
    pub noise_floor_db: f32,
    pub snr_db: Option<f32>,
    pub threshold_db: f32,
    pub low: bool,
}

impl CaptureLevelReceipt {
    /// Active-speech floor miss — the only WARN this receipt can raise.
    pub fn is_low(&self) -> bool {
        self.low
    }

    /// Counts-only WARN text. No transcript content.
    pub fn warning_message(&self) -> String {
        format!(
            "active_speech_median_db={:.1} threshold_db={:.1} all_audio_median_db={:.1} peak_db={:.1} samples={} digital_zero={} clip={} dropout={} device={} rate={} ch={}",
            self.active_speech_median_db,
            self.threshold_db,
            self.all_audio_median_db,
            self.peak_db,
            self.sample_count,
            self.digital_zero_samples,
            self.clipping_samples,
            self.dropout_blocks,
            self.device_name,
            self.sample_rate,
            self.channels
        )
    }

    /// Coarse quality token for a later Audio-menu surface.
    pub fn quality_verdict(&self) -> &'static str {
        if self.low {
            "low"
        } else if self.clipping_samples > 0 || self.dropout_blocks > 0 {
            "degraded"
        } else {
            "ok"
        }
    }

    /// Session-end log line. Always info for the receipt; WARN is separate.
    pub fn log(&self) {
        info!(
            code = self.code,
            device = self.device_name.as_str(),
            sample_rate = self.sample_rate,
            channels = self.channels,
            sample_count = self.sample_count,
            digital_zero_samples = self.digital_zero_samples,
            active_speech_samples = self.active_speech_samples,
            clipping_samples = self.clipping_samples,
            dropout_blocks = self.dropout_blocks,
            all_audio_median_db = format!("{:.1}", self.all_audio_median_db),
            active_speech_median_db = format!("{:.1}", self.active_speech_median_db),
            peak_db = format!("{:.1}", self.peak_db),
            noise_floor_db = format!("{:.1}", self.noise_floor_db),
            snr_db = self.snr_db.map(|v| format!("{v:.1}")),
            threshold_db = format!("{:.1}", self.threshold_db),
            quality = self.quality_verdict(),
            "capture_level_receipt"
        );
    }
}

/// Log the receipt and emit a non-terminal WARN when the active-speech floor
/// is missed. The sink still receives a Warning event; the bridge must keep
/// routing it off `on_error` via [`crate::pipeline::contracts::warning_is_user_terminal`].
pub fn emit_capture_level_receipt(sink: &dyn EventSink, receipt: &CaptureLevelReceipt) {
    receipt.log();
    remember_last(receipt);
    if receipt.is_low() {
        warn!(
            code = CAPTURE_LEVEL_LOW_CODE,
            active_speech_median_db = format!("{:.1}", receipt.active_speech_median_db),
            threshold_db = format!("{:.1}", receipt.threshold_db),
            "capture_level_low"
        );
        sink.on_event(&EngineEvent::Warning {
            code: CAPTURE_LEVEL_LOW_CODE.to_string(),
            message: receipt.warning_message(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::{
        ADMISSION_REFUSED_WARNING_CODE, USER_TERMINAL_WARNING_CODES, warning_is_user_terminal,
    };
    use std::sync::Mutex;

    struct CapturingSink {
        events: Mutex<Vec<EngineEvent>>,
    }

    impl EventSink for CapturingSink {
        fn on_event(&self, event: &EngineEvent) {
            self.events.lock().expect("sink").push(event.clone());
        }
    }

    fn tone(amplitude: f32, n: usize) -> Vec<f32> {
        // Square wave: RMS equals amplitude, matching `block_rms` contracts.
        (0..n)
            .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
            .collect()
    }

    fn zeros(n: usize) -> Vec<f32> {
        vec![0.0; n]
    }

    /// Active-speech median keys the WARN; digital-zero silence must not drag
    /// it. Attenuated speech below −52 dB warns. The WARN is never terminal.
    #[test]
    fn w13_capture_receipt_active_speech() {
        // W13-5: quality receipts never become terminal. The list may only
        // hold true take-terminal codes: a failed transcription and a refused
        // acoustic admission (the take never opened a microphone).
        assert_eq!(
            USER_TERMINAL_WARNING_CODES,
            &["transcription_failed", ADMISSION_REFUSED_WARNING_CODE],
            "W13-5 must not enlarge the terminal-warning list beyond take-terminal codes"
        );
        assert!(
            !warning_is_user_terminal(CAPTURE_LEVEL_LOW_CODE),
            "capture_level_low must stay a quality receipt"
        );
        assert!(
            !warning_is_user_terminal(CAPTURE_LEVEL_RECEIPT_CODE),
            "capture_level_receipt must stay a quality receipt"
        );

        let golden = db_to_linear(-38.0);
        let attenuated = db_to_linear(-58.0);
        let meta = CapturePathMeta {
            device_name: "EarPods".into(),
            sample_rate: 48_000,
            channels: 1,
        };

        let mut healthy = CaptureLevelAccumulator::new();
        // 191351-class mix: digital-zero floors around speech, both mic modes.
        for _ in 0..20 {
            healthy.push_samples(&zeros(512));
        }
        for _ in 0..8 {
            healthy.push_samples(&tone(golden, 512));
        }
        for _ in 0..20 {
            healthy.push_samples(&zeros(512));
        }
        let healthy_receipt = healthy.finalize(meta.clone());
        assert!(
            healthy_receipt.active_speech_median_db.is_finite(),
            "active-speech median must be defined when speech is present"
        );
        assert!(
            (healthy_receipt.active_speech_median_db + 38.0).abs() < 1.5,
            "active-speech median should sit near the golden −38 dB, got {}",
            healthy_receipt.active_speech_median_db
        );
        assert!(
            healthy_receipt.all_audio_median_db < healthy_receipt.active_speech_median_db - 10.0
                || !healthy_receipt.all_audio_median_db.is_finite(),
            "all-audio median must be dragged by digital-zero floors (all={} active={})",
            healthy_receipt.all_audio_median_db,
            healthy_receipt.active_speech_median_db
        );
        assert!(
            !healthy_receipt.is_low(),
            "golden-era active speech must not WARN (active={})",
            healthy_receipt.active_speech_median_db
        );
        assert_eq!(healthy_receipt.device_name, "EarPods");
        assert_eq!(healthy_receipt.sample_rate, 48_000);
        assert_eq!(healthy_receipt.channels, 1);
        assert!(healthy_receipt.digital_zero_samples > 0);
        assert!(healthy_receipt.sample_count > healthy_receipt.digital_zero_samples);
        assert_eq!(healthy_receipt.quality_verdict(), "ok");

        let healthy_sink = CapturingSink {
            events: Mutex::new(Vec::new()),
        };
        emit_capture_level_receipt(&healthy_sink, &healthy_receipt);
        assert!(
            healthy_sink.events.lock().expect("sink").is_empty(),
            "normal level must not emit capture_level_low"
        );

        let mut quiet = CaptureLevelAccumulator::new();
        for _ in 0..12 {
            quiet.push_samples(&zeros(512));
        }
        for _ in 0..8 {
            quiet.push_samples(&tone(attenuated, 512));
        }
        for _ in 0..12 {
            quiet.push_samples(&zeros(512));
        }
        let quiet_receipt = quiet.finalize(meta);
        assert!(
            quiet_receipt.active_speech_median_db < DEFAULT_CAPTURE_LEVEL_LOW_DB,
            "attenuated take must sit below −52 dB, got {}",
            quiet_receipt.active_speech_median_db
        );
        assert!(
            quiet_receipt.is_low(),
            "attenuated active speech must WARN (active={})",
            quiet_receipt.active_speech_median_db
        );

        let quiet_sink = CapturingSink {
            events: Mutex::new(Vec::new()),
        };
        emit_capture_level_receipt(&quiet_sink, &quiet_receipt);
        let events = quiet_sink.events.lock().expect("sink");
        match events.as_slice() {
            [EngineEvent::Warning { code, message }] => {
                assert_eq!(code, CAPTURE_LEVEL_LOW_CODE);
                assert!(
                    message.contains("active_speech_median_db="),
                    "WARN must name the active-speech key: {message}"
                );
                assert!(
                    !message.contains("Dictation stopped"),
                    "WARN text must not look terminal: {message}"
                );
            }
            other => panic!("expected one capture_level_low warning, got {other:?}"),
        }
        assert!(
            !warning_is_user_terminal(CAPTURE_LEVEL_LOW_CODE),
            "emitting the WARN must not change the terminal class"
        );
    }

    #[test]
    fn session_energy_db_is_pcm_range_intensity() {
        begin_session_energy_clock();
        let mut acc = CaptureLevelAccumulator::new();
        acc.push_samples(&vec![0.0; 160]);
        acc.push_samples(&vec![0.1; 160]);
        acc.push_samples(&vec![0.0; 160]);
        assert!(
            session_energy_db(0, 160).is_none(),
            "digital-zero hops have no finite dBFS"
        );
        let speech = session_energy_db(160, 320).expect("speech hop");
        assert!(speech.is_finite());
        assert!(session_energy_db(480, 640).is_none());
        begin_session_energy_clock();
        assert!(session_energy_db(160, 320).is_none());
    }
}
