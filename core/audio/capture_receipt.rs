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

use std::sync::{Arc, Mutex, OnceLock};

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
    /// PCM this owner actually ingested. Zero means nothing was ever measured,
    /// which is a different fact from "measured, and it was silent".
    observed_samples: u64,
    /// Start of the earliest hop that carried non-finite PCM, if any.
    ///
    /// Validity is **positional**, not a total. `push_samples` substitutes zero
    /// for every non-finite sample before measuring, so a contaminated hop
    /// reports a depressed RMS and reads as silence. Counting those samples and
    /// comparing the total against the whole observed extent — the shape this
    /// field replaces — hid exactly that: one invalid second inside a
    /// two-second take never reached the threshold, so the unmeasurable second
    /// was certified silent. What the reader needs is where the measurement
    /// stopped being trustworthy, which is this.
    first_invalid_sample: Option<u64>,
}

/// Producer token for the capture energy ladder.
pub const CAPTURE_ENERGY_PRODUCER: &str = "capture_energy";

/// Identity one capture-evidence owner is bound to.
///
/// A reader supplies the identity it expects and the owner answers whether it
/// matches. This is the whole difference from the previous shape, where the
/// caller's own session/epoch were stamped onto whatever hops happened to sit
/// in a process-global ladder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEvidenceIdentity {
    pub session: String,
    pub capture_epoch: u64,
}

impl CaptureEvidenceIdentity {
    /// Bind an identity for one take.
    pub fn new(session: impl Into<String>, capture_epoch: u64) -> Self {
        Self {
            session: session.into(),
            capture_epoch,
        }
    }

    /// Whether this identity names exactly the requested take.
    pub fn matches(&self, session: &str, capture_epoch: u64) -> bool {
        self.session == session && self.capture_epoch == capture_epoch
    }
}

/// Whether an acoustic observer measured a take, and how much of it.
///
/// Absence of speech and absence of measurement are separate states. Only
/// [`Self::Observed`] may support a silence claim, and only up to
/// `observed_samples`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcousticAvailability {
    /// The observer ingested `observed_samples` of contiguous PCM for the
    /// bound identity. An empty range set here is measured silence.
    Observed { observed_samples: u64 },
    /// No PCM ever reached this observer for the bound identity.
    NotObserved,
    /// A measurement exists, but it names a different session or epoch.
    IdentityMismatch,
    /// PCM arrived and some of it was non-finite, so this observer's
    /// measurement cannot be trusted for the take.
    ///
    /// Non-finite input is substituted with zero before measurement, which
    /// makes an unmeasurable region indistinguishable from a silent one. The
    /// observer therefore refuses the whole take rather than certifying the
    /// part it could still read: an invalid region is not silence whether it
    /// arrives first, last, or between two valid ones. `valid_samples` is the
    /// contiguous extent measured before the first invalid sample — diagnostic
    /// only, never a coverage extent.
    InvalidMeasurement { valid_samples: u64 },
    /// PCM arrived with a hole — some captured audio never reached this
    /// observer, either because a chunk skipped ahead or because the extent
    /// stops short of the capture the take produced. The unobserved part cannot
    /// be certified either way.
    Discontinuous { observed_samples: u64 },
}

impl AcousticAvailability {
    /// Contiguous extent this observer may speak for, if any.
    pub fn observed_samples(self) -> Option<u64> {
        match self {
            Self::Observed { observed_samples } => Some(observed_samples),
            Self::NotObserved
            | Self::IdentityMismatch
            | Self::InvalidMeasurement { .. }
            | Self::Discontinuous { .. } => None,
        }
    }

    /// Stable token for logs and projections.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed { .. } => "observed",
            Self::NotObserved => "not_observed",
            Self::IdentityMismatch => "identity_mismatch",
            Self::InvalidMeasurement { .. } => "invalid_measurement",
            Self::Discontinuous { .. } => "discontinuous",
        }
    }
}

/// Owner-authenticated speech evidence for exactly one take.
///
/// Both the identity and the availability state come from the observer that
/// did the measuring. A consumer cannot upgrade availability, and an empty
/// [`Self::ranges`] means silence only when the availability says the observer
/// was actually there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcousticSpeechEvidence {
    identity: CaptureEvidenceIdentity,
    producer: &'static str,
    availability: AcousticAvailability,
    ranges: Vec<TailSampleRange>,
}

impl AcousticSpeechEvidence {
    /// Mint evidence from an observer's own measurement.
    ///
    /// Only the two acoustic observers (this module's energy ladder and the
    /// session's single Silero ingress) call this in production; the ranges
    /// must already be on the observer's own PCM clock.
    pub fn measured(
        identity: CaptureEvidenceIdentity,
        producer: &'static str,
        availability: AcousticAvailability,
        ranges: Vec<TailSampleRange>,
    ) -> Self {
        Self {
            identity,
            producer,
            availability,
            ranges,
        }
    }

    /// Evidence that carries no measurement. Ranges are dropped: an
    /// unavailable observer has nothing to say about where speech was.
    pub fn unavailable(
        identity: CaptureEvidenceIdentity,
        producer: &'static str,
        availability: AcousticAvailability,
    ) -> Self {
        debug_assert!(
            availability.observed_samples().is_none(),
            "an observed measurement must carry its ranges"
        );
        Self {
            identity,
            producer,
            availability,
            ranges: Vec::new(),
        }
    }

    pub fn identity(&self) -> &CaptureEvidenceIdentity {
        &self.identity
    }

    pub fn producer(&self) -> &'static str {
        self.producer
    }

    pub fn availability(&self) -> AcousticAvailability {
        self.availability
    }

    pub fn ranges(&self) -> &[TailSampleRange] {
        &self.ranges
    }

    /// Whether this observer measured a contiguous extent it may speak for.
    /// A `true` answer says nothing about whether it heard any speech.
    pub fn is_observed(&self) -> bool {
        self.availability.observed_samples().is_some()
    }

    /// Whether this observer measured speech (not just measured).
    pub fn observed_speech(&self) -> bool {
        self.is_observed() && !self.ranges.is_empty()
    }
}

/// Shared, identity-bound owner of one take's capture energy ladder.
///
/// The production writer is the async capture arm and the production reader is
/// the blocking Apple worker thread, so this is one handle both sides hold —
/// not a process-global slot with a reset entrypoint. Cloning shares the same
/// measurement; it does not fork a second authority.
#[derive(Debug, Clone)]
pub struct CaptureEnergyOwner {
    identity: Arc<CaptureEvidenceIdentity>,
    clock: Arc<Mutex<SessionEnergyClock>>,
}

impl CaptureEnergyOwner {
    /// Open the energy ladder for one capture epoch.
    pub fn bind(session: impl Into<String>, capture_epoch: u64) -> Self {
        Self {
            identity: Arc::new(CaptureEvidenceIdentity::new(session, capture_epoch)),
            clock: Arc::new(Mutex::new(SessionEnergyClock::default())),
        }
    }

    /// Identity this ladder measures. A successor take binds its own owner.
    pub fn identity(&self) -> &CaptureEvidenceIdentity {
        &self.identity
    }

    fn record_hop(&self, sample_start: u64, sample_end: u64, rms: f32, nonfinite: u64) {
        let mut clock = self.clock.lock().unwrap_or_else(|e| e.into_inner());
        clock.observed_samples = clock.observed_samples.max(sample_end);
        if nonfinite > 0 {
            // The hop is contaminated wherever the invalid samples sat inside
            // it, so the trustworthy prefix ends where the hop begins. Hops
            // arrive in capture order but `min` keeps this true regardless.
            let first = clock.first_invalid_sample.get_or_insert(sample_start);
            *first = (*first).min(sample_start);
        }
        if sample_end <= sample_start || !rms.is_finite() || rms < 0.0 {
            return;
        }
        clock.hops.push(EnergyHop {
            sample_start,
            sample_end,
            rms,
        });
    }

    /// Mean RMS of hops overlapping `[sample_start, sample_end)`, as dBFS.
    ///
    /// Missing hops, inverted ranges, or a silent window return `None`. This is
    /// intensity on the PCM clock — not a confidence score.
    pub fn session_energy_db(&self, sample_start: u64, sample_end: u64) -> Option<f32> {
        if sample_end <= sample_start {
            return None;
        }
        let clock = self.clock.lock().unwrap_or_else(|e| e.into_inner());
        let mut weighted = 0.0_f64;
        let mut covered = 0.0_f64;
        for hop in &clock.hops {
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

    /// Active-speech evidence measured by this take's capture energy ladder.
    ///
    /// `session` / `capture_epoch` are the identity the caller **expects**; a
    /// mismatch is reported as [`AcousticAvailability::IdentityMismatch`] and
    /// no range is relabelled. This exposes the existing detector on the
    /// canonical PCM clock; it does not run a second VAD or read transcript
    /// text.
    pub fn session_active_speech_ranges(
        &self,
        session: &str,
        capture_epoch: u64,
        sample_rate: u32,
    ) -> AcousticSpeechEvidence {
        let identity = (*self.identity).clone();
        if !self.identity.matches(session, capture_epoch) {
            return AcousticSpeechEvidence::unavailable(
                identity,
                CAPTURE_ENERGY_PRODUCER,
                AcousticAvailability::IdentityMismatch,
            );
        }
        let merge_gap = u64::from(sample_rate).saturating_mul(ACTIVE_SPEECH_MERGE_GAP_MS) / 1_000;
        let clock = self.clock.lock().unwrap_or_else(|e| e.into_inner());
        if clock.observed_samples == 0 {
            return AcousticSpeechEvidence::unavailable(
                identity,
                CAPTURE_ENERGY_PRODUCER,
                AcousticAvailability::NotObserved,
            );
        }
        if let Some(first_invalid) = clock.first_invalid_sample {
            // One invalid region is enough. Publishing the valid prefix as a
            // measurement would hand the ledger an extent that stops short of
            // the capture without saying so, which is the same certified-silence
            // lie one step further down.
            return AcousticSpeechEvidence::unavailable(
                identity,
                CAPTURE_ENERGY_PRODUCER,
                AcousticAvailability::InvalidMeasurement {
                    valid_samples: first_invalid,
                },
            );
        }
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for hop in clock
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
        AcousticSpeechEvidence::measured(
            identity,
            CAPTURE_ENERGY_PRODUCER,
            AcousticAvailability::Observed {
                observed_samples: clock.observed_samples,
            },
            ranges
                .into_iter()
                .map(|(sample_start, sample_end)| TailSampleRange {
                    session: session.to_string(),
                    capture_epoch,
                    sample_start,
                    sample_end,
                })
                .collect(),
        )
    }
}

fn last_receipt_slot() -> &'static Mutex<Option<CaptureLevelReceipt>> {
    LAST_RECEIPT.get_or_init(|| Mutex::new(None))
}

fn last_open_path_slot() -> &'static Mutex<Option<CapturePathMeta>> {
    LAST_OPEN_PATH.get_or_init(|| Mutex::new(None))
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
///
/// The accumulator is the **writer** of session acoustic evidence, so the
/// binding lives here rather than on the read path: an unbound accumulator
/// (guided calibration) still produces its full statistical receipt, but its
/// hops never reach a take's ladder.
#[derive(Debug, Default)]
pub struct CaptureLevelAccumulator {
    /// Owner this writer feeds. `None` = statistics-only (calibration).
    energy: Option<CaptureEnergyOwner>,
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
    /// Statistics-only accumulator. Measures the buffer, owns no take.
    ///
    /// Guided energy calibration uses this: its numbers must stay available
    /// without a live take's acoustic evidence inheriting them.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulator bound to one capture epoch's energy ladder.
    ///
    /// Every hop this writer measures lands in `owner`, and only there. The
    /// reader on the worker thread holds a clone of the same owner.
    pub fn bound_to(owner: &CaptureEnergyOwner) -> Self {
        Self {
            energy: Some(owner.clone()),
            ..Self::default()
        }
    }

    /// Ingest one captured block (mono f32, already downmixed).
    pub fn push_samples(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut sum_sq = 0.0_f64;
        let mut zeros = 0_u64;
        let mut clips = 0_u64;
        let mut nonfinite = 0_u64;
        let mut peak = 0.0_f32;
        for sample in samples {
            let x = if sample.is_finite() {
                *sample
            } else {
                nonfinite += 1;
                0.0
            };
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
        if let Some(owner) = self.energy.as_ref() {
            owner.record_hop(sample_start, self.sample_count, rms, nonfinite);
        }
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
    use crate::pipeline::contracts::{USER_TERMINAL_WARNING_CODES, warning_is_user_terminal};
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
        // hold true engine-warning terminal codes. Admission refusals use the
        // typed presentation-status projection instead of this side-channel.
        assert_eq!(
            USER_TERMINAL_WARNING_CODES,
            &["transcription_failed"],
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
        let owner = CaptureEnergyOwner::bind("intensity", 1);
        let mut acc = CaptureLevelAccumulator::bound_to(&owner);
        acc.push_samples(&vec![0.0; 160]);
        acc.push_samples(&vec![0.1; 160]);
        acc.push_samples(&vec![0.0; 160]);
        assert!(
            owner.session_energy_db(0, 160).is_none(),
            "digital-zero hops have no finite dBFS"
        );
        let speech = owner.session_energy_db(160, 320).expect("speech hop");
        assert!(speech.is_finite());
        assert!(owner.session_energy_db(480, 640).is_none());
        let successor = CaptureEnergyOwner::bind("intensity", 2);
        assert!(
            successor.session_energy_db(160, 320).is_none(),
            "a successor epoch opens its own ladder"
        );
    }

    /// Captured zeros and no samples at all are different facts. The first is a
    /// measurement whose answer is "no speech"; the second is no measurement.
    #[test]
    fn measured_silence_and_absent_measurement_have_different_availability() {
        let silent = CaptureEnergyOwner::bind("availability", 1);
        let mut writer = CaptureLevelAccumulator::bound_to(&silent);
        writer.push_samples(&vec![0.0; 16_000]);
        let measured = silent.session_active_speech_ranges("availability", 1, 16_000);
        assert_eq!(
            measured.availability(),
            AcousticAvailability::Observed {
                observed_samples: 16_000
            }
        );
        assert!(measured.ranges().is_empty(), "zeros are never speech");
        assert!(measured.is_observed());
        assert!(!measured.observed_speech());

        let absent = CaptureEnergyOwner::bind("availability", 1);
        let unmeasured = absent.session_active_speech_ranges("availability", 1, 16_000);
        assert_eq!(
            unmeasured.availability(),
            AcousticAvailability::NotObserved,
            "a ladder nobody fed has measured nothing"
        );
        assert!(unmeasured.ranges().is_empty());
        assert!(!unmeasured.is_observed());
    }

    /// A buffer of NaN/inf is mapped to zero for measurement, so it must not be
    /// allowed to read as measured silence — in any position.
    ///
    /// Position is the whole point. The previous shape compared the non-finite
    /// total against the whole observed extent, so an invalid region vanished
    /// from the verdict as soon as enough valid PCM arrived on either side of
    /// it; the ladder then published a zero-RMS hop and the take sealed as
    /// silent audio nobody had actually measured.
    #[test]
    fn invalid_samples_cannot_certify_measured_silence_in_any_position() {
        let owner = CaptureEnergyOwner::bind("invalid", 1);
        let mut writer = CaptureLevelAccumulator::bound_to(&owner);
        writer.push_samples(&vec![f32::NAN; 320]);
        writer.push_samples(&vec![f32::INFINITY; 320]);
        assert_eq!(
            owner
                .session_active_speech_ranges("invalid", 1, 16_000)
                .availability(),
            AcousticAvailability::InvalidMeasurement { valid_samples: 0 },
            "NaN and infinities are equally unmeasurable, and nothing valid \
             preceded them"
        );

        // A finite buffer afterwards measures its own extent, but it cannot
        // retro-validate what came before it.
        writer.push_samples(&vec![0.0; 320]);
        assert_eq!(
            owner
                .session_active_speech_ranges("invalid", 1, 16_000)
                .availability(),
            AcousticAvailability::InvalidMeasurement { valid_samples: 0 },
            "valid PCM after an invalid region does not make that region silent"
        );

        // Valid first, then invalid: the trustworthy prefix is reported as a
        // diagnostic and the take is still refused.
        let mixed = CaptureEnergyOwner::bind("mixed", 1);
        let mut mixed_writer = CaptureLevelAccumulator::bound_to(&mixed);
        mixed_writer.push_samples(&vec![0.25; 320]);
        mixed_writer.push_samples(&vec![f32::NEG_INFINITY; 320]);
        let evidence = mixed.session_active_speech_ranges("mixed", 1, 16_000);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::InvalidMeasurement { valid_samples: 320 },
            "the first 320 samples were measurable; the take is not"
        );
        assert!(
            evidence.ranges().is_empty(),
            "refused evidence publishes no speech, not even the valid prefix's"
        );
        assert!(!evidence.is_observed());

        // One invalid sample inside an otherwise valid hop is enough: zero
        // substitution has already depressed that hop's RMS, so its silence
        // and its speech are equally unreliable.
        let one_bad = CaptureEnergyOwner::bind("one-bad", 1);
        let mut one_bad_writer = CaptureLevelAccumulator::bound_to(&one_bad);
        one_bad_writer.push_samples(&vec![0.25; 320]);
        let mut contaminated = vec![0.25f32; 320];
        contaminated[17] = f32::NAN;
        one_bad_writer.push_samples(&contaminated);
        assert_eq!(
            one_bad
                .session_active_speech_ranges("one-bad", 1, 16_000)
                .availability(),
            AcousticAvailability::InvalidMeasurement { valid_samples: 320 },
        );

        // Valid silence keeps its measurement and keeps succeeding.
        let quiet = CaptureEnergyOwner::bind("quiet", 1);
        let mut quiet_writer = CaptureLevelAccumulator::bound_to(&quiet);
        quiet_writer.push_samples(&vec![0.0; 640]);
        let quiet_evidence = quiet.session_active_speech_ranges("quiet", 1, 16_000);
        assert_eq!(
            quiet_evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: 640
            },
            "measured silence is a measurement; only invalid input is not"
        );
        assert!(quiet_evidence.is_observed());
        assert!(quiet_evidence.ranges().is_empty());
    }

    /// The reader supplies the identity it expects; the owner refuses to answer
    /// for a foreign take instead of stamping the caller's labels onto its hops.
    #[test]
    fn foreign_identity_is_refused_not_relabelled() {
        let owner = CaptureEnergyOwner::bind("owner-take", 7);
        let mut writer = CaptureLevelAccumulator::bound_to(&owner);
        writer.push_samples(&[0.25; 160]);

        for (session, epoch) in [("owner-take", 8), ("other-take", 7)] {
            let evidence = owner.session_active_speech_ranges(session, epoch, 16_000);
            assert_eq!(
                evidence.availability(),
                AcousticAvailability::IdentityMismatch,
                "{session}/{epoch} is not this owner's take"
            );
            assert!(evidence.ranges().is_empty());
            assert_eq!(evidence.identity().session, "owner-take");
            assert_eq!(evidence.identity().capture_epoch, 7);
        }

        let mine = owner.session_active_speech_ranges("owner-take", 7, 16_000);
        assert!(mine.observed_speech());
        assert_eq!(mine.ranges()[0].session, "owner-take");
        assert_eq!(mine.ranges()[0].capture_epoch, 7);
    }

    /// Guided calibration measures its own buffer and contaminates no take.
    #[test]
    fn calibration_accumulator_cannot_append_to_a_live_take() {
        let live = CaptureEnergyOwner::bind("live-take", 1);
        let mut calibration = CaptureLevelAccumulator::new();
        calibration.push_samples(&[0.25; 16_000]);
        let receipt = calibration.finalize(CapturePathMeta {
            device_name: "EarPods".into(),
            sample_rate: 16_000,
            channels: 1,
        });
        assert_eq!(
            receipt.sample_count, 16_000,
            "calibration keeps its statistics"
        );
        assert!(receipt.active_speech_median_db.is_finite());
        assert_eq!(
            live.session_active_speech_ranges("live-take", 1, 16_000)
                .availability(),
            AcousticAvailability::NotObserved,
            "an unbound writer may not appear in a take's acoustic evidence"
        );
    }

    /// Production writes on the async capture arm and reads on the blocking
    /// Apple worker thread. Prove the same bound owner crosses that boundary,
    /// and that a concurrently created successor cannot inherit the writes.
    #[test]
    fn bound_owner_crosses_threads_and_successor_stays_separate() {
        let owner = CaptureEnergyOwner::bind("cross-thread", 3);
        let successor = CaptureEnergyOwner::bind("cross-thread", 4);
        let written = std::sync::Arc::new(std::sync::Barrier::new(2));

        let writer_owner = owner.clone();
        let writer_gate = std::sync::Arc::clone(&written);
        let writer = std::thread::spawn(move || {
            let mut accumulator = CaptureLevelAccumulator::bound_to(&writer_owner);
            accumulator.push_samples(&[0.25; 160]);
            accumulator.push_samples(&[0.0; 160]);
            writer_gate.wait();
        });

        let reader_owner = owner.clone();
        let reader_successor = successor.clone();
        let reader_gate = std::sync::Arc::clone(&written);
        let reader = std::thread::spawn(move || {
            reader_gate.wait();
            let evidence = reader_owner.session_active_speech_ranges("cross-thread", 3, 16_000);
            let successor_evidence =
                reader_successor.session_active_speech_ranges("cross-thread", 4, 16_000);
            (evidence, successor_evidence)
        });

        writer.join().expect("writer thread");
        let (evidence, successor_evidence) = reader.join().expect("reader thread");

        assert_eq!(
            evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: 320
            },
            "the reader thread must see the writer thread's extent"
        );
        assert_eq!(evidence.ranges().len(), 1);
        assert_eq!(
            (
                evidence.ranges()[0].sample_start,
                evidence.ranges()[0].sample_end
            ),
            (0, 160),
            "the silent second hop is not speech"
        );
        assert_eq!(
            successor_evidence.availability(),
            AcousticAvailability::NotObserved,
            "a successor epoch may not inherit predecessor availability"
        );
    }

    /// Unavailable evidence carries no ranges: an observer that was not there
    /// cannot also report where speech was.
    #[test]
    fn unavailable_evidence_carries_no_ranges() {
        let evidence = AcousticSpeechEvidence::unavailable(
            CaptureEvidenceIdentity::new("no-observer", 1),
            CAPTURE_ENERGY_PRODUCER,
            AcousticAvailability::NotObserved,
        );
        assert!(evidence.ranges().is_empty());
        assert!(!evidence.is_observed());
        assert!(!evidence.observed_speech());
        assert_eq!(evidence.availability().observed_samples(), None);
        assert_eq!(evidence.availability().as_str(), "not_observed");
        assert_eq!(evidence.producer(), CAPTURE_ENERGY_PRODUCER);
    }
}
