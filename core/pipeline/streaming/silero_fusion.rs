//! W13-3B — Silero utterance identity + conservative per-word fusion.
//!
//! The product-owned settings snapshot arms this mandatory lane by default;
//! [`SILERO_FUSION_ENV`] remains an optional power-user override. When armed:
//! Silero Supervisor edges supply boundary, time, and energy evidence on the
//! PCM sample clock; Apple cumulative finals are sliced onto those ranges by
//! time; Whisper and Apple then fuse conservatively (agreements + clear gap
//! fills). Unresolved alternatives are receipted, never confidence-arbitrated.
//! Text-bearing results return to the Apple session: `admit_ledger_label`
//! offers each observation to `AcousticLedger::admit`, closed occurrences pass
//! through `AcousticLedger::seal`, and ledger events reach the transcript
//! reducer. This module owns no text admission or seal authority.
//!
//! # One Silero per session
//!
//! [`SileroIngress`] is the session's **only** `SpeechSession`. Both consumers
//! of speech edges read it: Silero boundary-range bookkeeping and the
//! Apple engine lifecycle (`EpochGate` wake/sleep). Two independent VAD
//! sessions over the same PCM would mean two spectra and two sets of
//! boundaries, and "the same utterance" would then mean two different sample
//! ranges depending on which consumer was asked. [`SileroIngress::observe`] is
//! the single decision point that derives both from one observation.

use std::collections::VecDeque;

use crate::audio::capture_receipt::{
    AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
};
use crate::audio::chunker::{SpeechEvent, SpeechSession, VadBoundaryEvidence, VadBoundaryKind};
use crate::config::RuntimeSettingsSnapshot;
use crate::pipeline::contracts::{
    NonSpeechEvidence, SidebandEvidence, SidebandEvidenceKind, SidebandProvenance,
};
use crate::stt::tail_provider::{TailSampleRange, TimedTailSegment};

/// Stable name of the optional power-user override. Its value is resolved by
/// the canonical settings loader, never in this pipeline module.
pub use crate::config::settings::SILERO_FUSION_ENV;

/// Bounded-context A/B selector. Never crosses a long-silence cut.
pub const SILERO_FUSION_CONTEXT_ENV: &str = "CODESCRIBE_SILERO_FUSION_CONTEXT";

/// Silence longer than this (samples at the capture rate) is a hard context
/// fence — left-audio pad must not reach across it.
pub const LONG_SILENCE_FENCE_SECS: f32 = 0.55;

/// Default left-audio pad when [`FusionContextMode::LeftAudioPad`] is armed.
pub const DEFAULT_LEFT_PAD_SECS: f32 = 0.40;

/// Symmetric decode context around a Silero utterance, in seconds.
///
/// Context only: the padded range is the audio a decoder may *see*. The
/// occurrence it may *own* stays the unpadded utterance range.
pub const DEFAULT_SYMMETRIC_PAD_SECS: f32 = 0.40;

/// Producer token for the Silero threshold-crossing observer.
pub const SILERO_BOUNDARIES_PRODUCER: &str = "silero_boundaries";

/// Symmetric pad applied to a raw Silero threshold crossing when the seal
/// speech set is built.
///
/// This is the chunker's own `pre_roll_sec`/`speech_pad_sec` (64 ms): a
/// threshold crossing is where Silero became *sure*, not where the word began,
/// and the same quantum the capture path already trusts is what makes the
/// acoustic set comparable with committed occurrence ranges.
pub const ACOUSTIC_SPEECH_PAD_SECS: f32 = 0.064;

/// Two padded speech ranges separated by no more than this are one range.
///
/// Same figure the energy ladder already merges on (`ACTIVE_SPEECH_MERGE_GAP_MS`
/// in `capture_receipt`), so the acoustic set and the fallback set describe
/// coverage at the same granularity instead of two different ones.
pub const ACOUSTIC_SPEECH_MERGE_GAP_SECS: f32 = 0.200;

/// Maximum sideband facts retained for later span attachment. Live consumers
/// receive every freshly emitted fact; only the retrospective lookup window is
/// bounded.
const MAX_RETAINED_SIDEBAND_EVIDENCE: usize = 512;

/// Whether the seal lane can bound existence for a product take, probed with
/// no session open: the sealed product setting and whether the shared Silero graph loads.
/// `seal_utterance_final` passes `may_qualify = silero_bound`, so without
/// both no occurrence can ever qualify — admission readiness must ask first
/// instead of letting a take record into a ledger that cannot seal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealLaneProbe {
    /// The immutable runtime settings generation resolved the lane as armed.
    pub armed: bool,
    /// The embedded Silero model actually loaded in this process.
    pub vad_available: bool,
}

/// Probe the seal lane. Cheap after the first call: the Silero session is a
/// process-wide `OnceLock`.
pub fn seal_lane_probe(snapshot: &RuntimeSettingsSnapshot) -> SealLaneProbe {
    let armed = snapshot.seal_lane_armed();
    let vad_available = SileroIngress::new(16_000, "admission-probe", 0).vad_available();
    SealLaneProbe {
        armed,
        vad_available,
    }
}

/// One Silero-bounded utterance on the session PCM clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SileroUtterance {
    pub id: u64,
    pub range: TailSampleRange,
    pub closed: bool,
}

/// Silero boundary-range bookkeeping. Pure data; the Supervisor machine in
/// [`SileroIngress`] is the only writer in production. This is not the
/// [`crate::pipeline::acoustic_ledger::AcousticLedger`] and owns no text,
/// occurrence admission, or seal authority.
#[derive(Debug, Clone, Default)]
pub struct UtteranceLedger {
    next_id: u64,
    utterances: Vec<SileroUtterance>,
}

impl UtteranceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint (or refresh) an open utterance covering `[sample_start, sample_end)`.
    pub fn open_or_extend(
        &mut self,
        session: &str,
        capture_epoch: u64,
        sample_start: u64,
        sample_end: u64,
    ) -> u64 {
        let sample_end = sample_end.max(sample_start);
        if let Some(open) = self.utterances.iter_mut().rev().find(|u| !u.closed) {
            open.range.sample_end = sample_end.max(open.range.sample_end);
            return open.id;
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.utterances.push(SileroUtterance {
            id,
            range: TailSampleRange {
                session: session.to_string(),
                capture_epoch,
                sample_start,
                sample_end,
            },
            closed: false,
        });
        id
    }

    /// Close the open utterance so the next speech edge mints a new identity.
    pub fn close_open(&mut self, sample_end: u64) -> Option<u64> {
        let open = self.utterances.iter_mut().rev().find(|u| !u.closed)?;
        open.range.sample_end = sample_end.max(open.range.sample_end);
        open.closed = true;
        Some(open.id)
    }

    pub fn utterances(&self) -> &[SileroUtterance] {
        &self.utterances
    }

    /// Utterance whose range contains `sample` (half-open). Prefers the
    /// tightest closed span; falls back to the open span.
    pub fn utterance_covering(&self, sample: u64) -> Option<&SileroUtterance> {
        self.utterances
            .iter()
            .filter(|u| u.range.sample_start <= sample && sample < u.range.sample_end)
            .min_by_key(|u| u.range.sample_end.saturating_sub(u.range.sample_start))
    }

    /// Tightest utterance that fully **encloses** `[sample_start, sample_end)`.
    ///
    /// This is the seal-time binding query: an Apple span may adopt a Silero
    /// range only when the spectrum edge already covers every sample Apple
    /// claimed. Mere overlap is refused on purpose — adopting a range that
    /// starts after Apple's first word would hand Layer 1 a window over audio
    /// the utterance never contained, and the span would seal against a decode
    /// of the wrong seconds. No enclosure ⇒ the caller keeps its own range
    /// (fail-open; content is never dropped for want of an edge).
    pub fn utterance_enclosing(
        &self,
        sample_start: u64,
        sample_end: u64,
    ) -> Option<&SileroUtterance> {
        let sample_end = sample_end.max(sample_start);
        self.utterances
            .iter()
            .filter(|u| u.range.sample_start <= sample_start && sample_end <= u.range.sample_end)
            .min_by_key(|u| u.range.sample_end.saturating_sub(u.range.sample_start))
    }

    /// Burn one identity without minting an utterance.
    ///
    /// The Apple-boundary fallback still needs a span id, and it must not be an
    /// id Silero will later mint for a real utterance: `note_apple_commit_timed`
    /// is idempotent on id, so a collision would silently merge an Apple span
    /// with an unrelated Silero one. One ledger, one id space.
    pub fn reserve_id(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }
}

/// The session's raw acoustic speech set: where Silero crossed its threshold.
///
/// This is deliberately **not** [`UtteranceLedger`]. An utterance range carries
/// padded STT-window semantics — it opens `pre_roll` before the crossing, stays
/// open across pauses shorter than the utterance gap, and closes on the capture
/// cursor rather than on the last speech frame. That padding is correct for
/// deciding *who owns which occurrence*, and wrong for asking *how much of this
/// take was speech*: measured on one archived take the utterance set reported
/// 2 974 s of "speech" for a 2 990 s recording whose offline Silero measurement
/// was 454 s. The 31 minutes of dropout noise that followed the last spoken
/// word were never speech; they were one padded window.
///
/// So the two live side by side and answer different questions. Ownership keeps
/// its padded ranges; coverage asks this set.
#[derive(Debug, Clone, Default)]
pub struct AcousticSpeechSet {
    /// Closed `[start, end)` crossings in PCM order, unpadded.
    closed: Vec<(u64, u64)>,
    /// Crossing opened but not yet closed.
    open_start: Option<u64>,
}

impl AcousticSpeechSet {
    /// Record a `SpeechStart` crossing.
    ///
    /// A second start with no intervening end keeps the earlier one: the
    /// earlier sample is the conservative boundary, and moving it forward would
    /// silently shrink the speech the set is meant to account for.
    fn open(&mut self, sample: u64) {
        if self.open_start.is_none() {
            self.open_start = Some(sample);
        }
    }

    /// Record a `SpeechEnd` crossing. An end with nothing open is ignored: it
    /// describes a segment this set never saw begin, and inventing a start for
    /// it would manufacture speech evidence.
    fn close(&mut self, sample: u64) {
        if let Some(start) = self.open_start.take() {
            self.closed.push((start, sample.max(start)));
        }
    }

    /// Unpadded crossings, with any still-open range closed at `samples_seen`.
    ///
    /// Stop mid-word is the normal case, so the open range is real speech up to
    /// the capture cursor — not something to drop.
    pub fn raw_ranges(&self, samples_seen: u64) -> Vec<(u64, u64)> {
        let mut ranges = self.closed.clone();
        if let Some(start) = self.open_start {
            ranges.push((start, samples_seen.max(start)));
        }
        ranges
    }

    /// Padded, gap-merged speech ranges on the session PCM clock.
    ///
    /// Padding is symmetric and clamped to `[0, samples_seen]`: a crossing at
    /// sample 0 cannot pad below zero, and a crossing at EOF cannot claim audio
    /// the session never captured. Short crossings survive — a 60 ms word is
    /// speech that the ledger is entitled to be asked about, and dropping it
    /// here would quietly shrink the very set that proves coverage.
    pub fn ranges(
        &self,
        session: &str,
        capture_epoch: u64,
        samples_seen: u64,
        pad_samples: u64,
        merge_gap_samples: u64,
    ) -> Vec<TailSampleRange> {
        let mut padded: Vec<(u64, u64)> = self
            .raw_ranges(samples_seen)
            .into_iter()
            .filter(|(start, end)| end >= start)
            .map(|(start, end)| {
                (
                    start.saturating_sub(pad_samples),
                    end.saturating_add(pad_samples).min(samples_seen),
                )
            })
            .filter(|(start, end)| end > start)
            .collect();
        padded.sort_unstable();

        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(padded.len());
        for (start, end) in padded {
            if let Some((_, previous_end)) = merged.last_mut()
                && start <= previous_end.saturating_add(merge_gap_samples)
            {
                *previous_end = (*previous_end).max(end);
            } else {
                merged.push((start, end));
            }
        }

        merged
            .into_iter()
            .map(|(sample_start, sample_end)| TailSampleRange {
                session: session.to_string(),
                capture_epoch,
                sample_start,
                sample_end,
            })
            .collect()
    }
}

/// What one observed capture chunk means to every consumer of the session's
/// single spectrum.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SileroIngest {
    /// Utterance identities the Supervisor closed inside this chunk.
    pub closed: Vec<u64>,
    /// Identity of the utterance still open after this chunk.
    pub open: Option<u64>,
    /// Speech was live anywhere in this chunk — a segment is open, or one
    /// closed inside it. This is the edge bit the Apple engine lifecycle
    /// (`EpochGate`) reads instead of running a second Silero over the same
    /// PCM; it is derived from the identical two facts the ledger is minted
    /// from, in the same call, so wake/sleep and utterance identity cannot
    /// disagree about where speech was.
    pub speech_live: bool,
    /// Newly measured content-free evidence, in PCM/sequence order.
    pub sideband: Vec<SidebandEvidence>,
}

/// Supervisor-mode Silero at the Apple PCM ingress. The session's only VAD.
pub struct SileroIngress {
    session: String,
    capture_epoch: u64,
    sample_rate: u32,
    vad: SpeechSession,
    ledger: UtteranceLedger,
    next_sideband_sequence: u64,
    last_speech_end: Option<u64>,
    sideband: VecDeque<SidebandEvidence>,
    /// Raw threshold crossings, unbounded on purpose.
    ///
    /// [`MAX_RETAINED_SIDEBAND_EVIDENCE`] bounds the *lookup window* for
    /// retrospective span attachment, which is a different job. A 50-minute
    /// take produces a few hundred crossings, and a coverage set that silently
    /// dropped its oldest ranges would report the beginning of the session as
    /// uncovered speech that was never speech.
    speech: AcousticSpeechSet,
    /// PCM this ingress actually ingested, recorded at ingest.
    ///
    /// The coverage question is answered against this extent, never against a
    /// caller-supplied endpoint: clamping an open crossing at a number the
    /// caller happens to hold does not prove those samples reached the VAD.
    observed_samples: u64,
    /// A chunk arrived that did not continue from [`Self::observed_samples`],
    /// so some captured audio never reached this observer. The hole cannot be
    /// certified as speech or as silence.
    discontinuous: bool,
    /// Session sample index of the first non-finite sample fed to this ingress.
    ///
    /// The VAD reads whatever arrives; NaN and infinities produce a probability
    /// that fails every threshold comparison, so invalid PCM leaves no crossing
    /// and would be reported as measured silence. Recording where validity
    /// ended keeps that region unmeasured instead.
    first_invalid_sample: Option<u64>,
}

impl SileroIngress {
    pub fn new(sample_rate: u32, session: impl Into<String>, capture_epoch: u64) -> Self {
        Self {
            session: session.into(),
            capture_epoch,
            sample_rate,
            vad: SpeechSession::new_utterance(sample_rate),
            ledger: UtteranceLedger::new(),
            next_sideband_sequence: 0,
            last_speech_end: None,
            sideband: VecDeque::new(),
            speech: AcousticSpeechSet::default(),
            observed_samples: 0,
            discontinuous: false,
            first_invalid_sample: None,
        }
    }

    /// Identity this ingress measures, for evidence authentication.
    pub fn evidence_identity(&self) -> CaptureEvidenceIdentity {
        CaptureEvidenceIdentity::new(self.session.clone(), self.capture_epoch)
    }

    /// PCM extent this ingress ingested. Recorded at ingest, not queried.
    pub fn observed_samples(&self) -> u64 {
        self.observed_samples
    }

    /// Record that `samples_len` of PCM ending at `samples_seen` reached this
    /// ingress.
    ///
    /// "Audio arrived" is a capture fact, not a VAD decision, so it is recorded
    /// on its own step: [`Self::ingest`] calls this before feeding the model,
    /// and fixtures that drive synthetic crossings call it to state the capture
    /// their crossings sit inside. It cannot invent a crossing, and a call that
    /// does not continue from the current extent marks the hole exactly as a
    /// real chunk would.
    pub fn note_observed_pcm(&mut self, samples_len: u64, samples_seen: u64) {
        let chunk_start = samples_seen.saturating_sub(samples_len);
        if chunk_start != self.observed_samples {
            self.discontinuous = true;
        }
        self.observed_samples = self.observed_samples.max(samples_seen);
    }

    /// Record whether the PCM in this chunk was measurable at all.
    ///
    /// Separated from the VAD read for the same reason [`Self::observe`] is:
    /// "the samples were finite" is a capture fact a fixture can state without
    /// a model, and a chunk of NaN must reach the same verdict whether the
    /// embedded model happens to be loaded or not. [`Self::ingest`] calls this
    /// on every chunk, immediately after [`Self::note_observed_pcm`] and before
    /// anything reads the audio.
    pub fn note_pcm_validity(&mut self, samples: &[f32], samples_seen: u64) {
        let chunk_start = samples_seen.saturating_sub(samples.len() as u64);
        let Some(offset) = samples.iter().position(|sample| !sample.is_finite()) else {
            return;
        };
        let first = chunk_start.saturating_add(offset as u64);
        let recorded = self.first_invalid_sample.get_or_insert(first);
        *recorded = (*recorded).min(first);
    }

    pub fn ledger(&self) -> &UtteranceLedger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut UtteranceLedger {
        &mut self.ledger
    }

    /// Authenticated acoustic evidence for the seal-coverage question.
    ///
    /// This is the only Silero surface a coverage verdict may read. It takes no
    /// caller endpoint: the extent comes from what this ingress actually
    /// ingested, so an unloaded model, an unfed session or a discontinuous
    /// cursor answer "unavailable" instead of an empty set that reads as
    /// silence. The padding and merge semantics are unchanged — the same
    /// [`Self::acoustic_speech_ranges`] math runs on the recorded extent.
    pub fn acoustic_speech_evidence(&self) -> AcousticSpeechEvidence {
        let identity = self.evidence_identity();
        if !self.vad_available() {
            // Every frame read as non-speech because no model loaded. An empty
            // crossing set here is absence of an observer, not absence of speech.
            return AcousticSpeechEvidence::unavailable(
                identity,
                SILERO_BOUNDARIES_PRODUCER,
                AcousticAvailability::NotObserved,
            );
        }
        if self.observed_samples == 0 {
            return AcousticSpeechEvidence::unavailable(
                identity,
                SILERO_BOUNDARIES_PRODUCER,
                AcousticAvailability::NotObserved,
            );
        }
        // Validity before continuity: a hole is audio this observer never got,
        // while invalid PCM is audio it got and could not read. Both refuse,
        // and the reason names which one actually happened.
        if let Some(valid_samples) = self.first_invalid_sample {
            return AcousticSpeechEvidence::unavailable(
                identity,
                SILERO_BOUNDARIES_PRODUCER,
                AcousticAvailability::InvalidMeasurement { valid_samples },
            );
        }
        if self.discontinuous {
            return AcousticSpeechEvidence::unavailable(
                identity,
                SILERO_BOUNDARIES_PRODUCER,
                AcousticAvailability::Discontinuous {
                    observed_samples: self.observed_samples,
                },
            );
        }
        let observed_samples = self.observed_samples();
        AcousticSpeechEvidence::measured(
            identity,
            SILERO_BOUNDARIES_PRODUCER,
            AcousticAvailability::Observed { observed_samples },
            self.acoustic_speech_ranges(observed_samples),
        )
    }

    /// Seal-time speech ranges: threshold crossings, padded by
    /// [`ACOUSTIC_SPEECH_PAD_SECS`] and merged across gaps up to
    /// [`ACOUSTIC_SPEECH_MERGE_GAP_SECS`], on this session's identity.
    ///
    /// No occurrence is minted, moved or resized by this call. It reports what
    /// the microphone heard; the ledger keeps deciding what was committed.
    ///
    /// This is the padding/merge math, **not** evidence: `samples_seen` is a
    /// caller-supplied endpoint and closing an open crossing on it proves
    /// nothing about ingestion. Coverage reads
    /// [`Self::acoustic_speech_evidence`], which supplies the recorded extent.
    pub fn acoustic_speech_ranges(&self, samples_seen: u64) -> Vec<TailSampleRange> {
        let rate = self.sample_rate.max(1) as f32;
        let pad = (ACOUSTIC_SPEECH_PAD_SECS * rate).round().max(0.0) as u64;
        let merge_gap = (ACOUSTIC_SPEECH_MERGE_GAP_SECS * rate).round().max(0.0) as u64;
        self.speech.ranges(
            &self.session,
            self.capture_epoch,
            samples_seen,
            pad,
            merge_gap,
        )
    }

    /// Whether Silero actually loaded. `false` means every frame reads as
    /// non-speech: no identity will ever be minted and no speech edge will ever
    /// fire, so consumers that gate on edges must fail open instead of resting
    /// forever.
    pub fn vad_available(&self) -> bool {
        self.vad.vad_available()
    }

    /// Feed one capture chunk. `samples_seen` is the session cursor *after*
    /// this chunk (same counter `apple_stream_worker` already owns).
    pub fn ingest(&mut self, samples: &[f32], samples_seen: u64) -> SileroIngest {
        if samples.is_empty() {
            // Nothing was observed, so nothing is recorded. An empty call must
            // not advance the extent this ingress claims to have heard.
            return SileroIngest::default();
        }
        // Record the observed extent and the validity of the supplied PCM,
        // before any speech decision reads it.
        self.note_observed_pcm(samples.len() as u64, samples_seen);
        self.note_pcm_validity(samples, samples_seen);
        let events = self.vad.feed(samples, 0);
        let boundaries = self.vad.take_vad_boundaries();
        let closed_here = events
            .iter()
            .any(|event| matches!(event, SpeechEvent::UtteranceFinal));
        let open_range = self.vad.open_segment_raw_range();
        let mut out = self.observe(open_range, closed_here, samples_seen);
        out.sideband = self.observe_boundaries(&boundaries);
        out
    }

    /// The whole decision, separated from the VAD read so it is testable on
    /// synthetic edges (Silero loads from embedded bytes; a unit test that
    /// silently degraded to "no model" would prove nothing). Production calls
    /// this exactly once per chunk, from [`Self::ingest`].
    pub fn observe(
        &mut self,
        open_range: Option<(u64, u64)>,
        closed_here: bool,
        samples_seen: u64,
    ) -> SileroIngest {
        let mut out = SileroIngest {
            speech_live: closed_here || open_range.is_some(),
            ..SileroIngest::default()
        };
        if let Some((start, end)) = open_range {
            out.open =
                Some(
                    self.ledger
                        .open_or_extend(&self.session, self.capture_epoch, start, end),
                );
        }
        if closed_here && let Some(id) = self.ledger.close_open(samples_seen) {
            out.closed.push(id);
            if out.open == Some(id) {
                out.open = None;
            }
        }
        out
    }

    /// Convert the chunker's exact Silero boundaries into ordered pipeline
    /// evidence, and record the same crossings in the acoustic speech set.
    ///
    /// This is intentionally separate from [`Self::observe`]: the fusion ledger
    /// retains its padded STT window semantics, while these crossings name the
    /// unpadded thresholds exactly. Both are derived here, from one drained
    /// batch, so the set that measures speech and the evidence that reports it
    /// cannot describe different samples.
    pub(crate) fn observe_boundaries(
        &mut self,
        boundaries: &[VadBoundaryEvidence],
    ) -> Vec<SidebandEvidence> {
        let mut emitted = Vec::new();
        for boundary in boundaries {
            match boundary.kind {
                VadBoundaryKind::SpeechStart => {
                    self.speech.open(boundary.sample);
                    if let Some(pause_start) = self.last_speech_end.take()
                        && pause_start < boundary.sample
                    {
                        emitted.push(self.push_sideband(
                            pause_start,
                            boundary.sample,
                            SidebandEvidenceKind::Pause {
                                duration_samples: boundary.sample - pause_start,
                                non_speech: NonSpeechEvidence::UnknownNonSpeech,
                            },
                        ));
                    }
                    emitted.push(self.push_sideband(
                        boundary.sample,
                        boundary.sample,
                        SidebandEvidenceKind::SpeechStart {
                            speech_probability: boundary.speech_probability,
                        },
                    ));
                }
                VadBoundaryKind::SpeechEnd => {
                    self.speech.close(boundary.sample);
                    emitted.push(self.push_sideband(
                        boundary.sample,
                        boundary.sample,
                        SidebandEvidenceKind::SpeechEnd {
                            speech_probability: boundary.speech_probability,
                        },
                    ));
                    self.last_speech_end = Some(boundary.sample);
                }
            }
        }
        self.sideband.extend(emitted.iter().cloned());
        while self.sideband.len() > MAX_RETAINED_SIDEBAND_EVIDENCE {
            self.sideband.pop_front();
        }
        emitted
    }

    fn push_sideband(
        &mut self,
        sample_start: u64,
        sample_end: u64,
        evidence: SidebandEvidenceKind,
    ) -> SidebandEvidence {
        self.next_sideband_sequence = self.next_sideband_sequence.saturating_add(1);
        SidebandEvidence {
            sequence: self.next_sideband_sequence,
            range: TailSampleRange {
                session: self.session.clone(),
                capture_epoch: self.capture_epoch,
                sample_start,
                sample_end: sample_end.max(sample_start),
            },
            sample_rate_hz: self.sample_rate,
            provenance: SidebandProvenance::SileroVad,
            evidence,
        }
    }

    /// Seal any still-open Supervisor segment at capture EOF.
    ///
    /// The acoustic crossing is closed on the same cursor as the utterance, so
    /// a stop mid-word leaves one consistent EOF boundary in both sets rather
    /// than a speech range that outlives the audio.
    pub fn flush(&mut self, samples_seen: u64) -> Option<u64> {
        let _ = self.vad.flush();
        self.speech.close(samples_seen);
        self.ledger.close_open(samples_seen)
    }
}

/// How a Whisper window is cut relative to a Silero utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FusionContextMode {
    /// Utterance plus symmetric context on both sides, clipped at the
    /// long-silence fence, at end of captured PCM, and at the neighbouring
    /// utterance. Default.
    ///
    /// A decoder handed exactly the utterance sees a word begin on sample one
    /// and end on the last sample; the first and last syllable arrive with no
    /// acoustic run-up. The context is what the decoder listens to, never what
    /// the occurrence claims — see [`bound_context_range`].
    #[default]
    SymmetricPad,
    /// Audio is exactly the Silero utterance.
    UtteranceOnly,
    /// Small left pad, clipped at the last long-silence fence.
    LeftAudioPad,
    /// Same audio as utterance-only; the sealed prefix is the prompt (never
    /// audio across a long silence).
    StableTextPrompt,
}

impl FusionContextMode {
    pub fn from_env() -> Self {
        Self::from_env_value(std::env::var(SILERO_FUSION_CONTEXT_ENV).ok().as_deref())
    }

    /// Pure selector shared by environment loading and token-table tests.
    /// Missing and non-Unicode environment values both reach `None`.
    fn from_env_value(raw: Option<&str>) -> Self {
        match raw {
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "left_pad" | "left-pad" | "pad" => Self::LeftAudioPad,
                "stable_prompt" | "stable-text" | "prompt" => Self::StableTextPrompt,
                "utterance_only" | "utterance-only" | "exact" => Self::UtteranceOnly,
                _ => Self::default(),
            },
            None => Self::default(),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SymmetricPad => "symmetric_pad",
            Self::UtteranceOnly => "utterance_only",
            Self::LeftAudioPad => "left_audio_pad",
            Self::StableTextPrompt => "stable_text_prompt",
        }
    }
}

/// Everything that may stop decode context from growing.
///
/// All four are hard limits, not preferences. Crossing any one of them either
/// hands the decoder audio that belongs to a different acoustic event, or audio
/// that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextBounds {
    /// End sample of the last long-silence cut before this utterance. `0` when
    /// no fence applies. Context never reaches back across it.
    pub long_silence_fence: u64,
    /// One past the last sample the session has actually captured. Right
    /// context stops here; asking beyond it resolves to no window at all.
    pub capture_end: u64,
    /// Start of the next utterance, when one exists. Right context stops
    /// before a neighbour so a decode window cannot straddle two occurrences.
    pub next_utterance_start: Option<u64>,
    /// End of the previous utterance, when one exists. Left context stops
    /// after a neighbour, for the same reason.
    pub previous_utterance_end: Option<u64>,
}

/// Cut the audio range a provider may see.
///
/// The returned range is **context, not ownership**. Occurrence identity is
/// minted from the unpadded utterance range by the caller and is not derived
/// from this value; widening the window can improve the words a decoder
/// produces, and can never widen what the resulting label owns. Long silence,
/// end of captured PCM and the neighbouring utterances are hard fences.
pub fn bound_context_range(
    utterance: &TailSampleRange,
    mode: FusionContextMode,
    pad_samples: u64,
    bounds: &ContextBounds,
) -> TailSampleRange {
    let last_long_silence_end = bounds.long_silence_fence;
    let mut range = utterance.clone();

    let wants_left_pad = matches!(
        mode,
        FusionContextMode::LeftAudioPad | FusionContextMode::SymmetricPad
    );
    if wants_left_pad {
        let want = utterance.sample_start.saturating_sub(pad_samples);
        range.sample_start = want.max(last_long_silence_end);
        if let Some(previous_end) = bounds.previous_utterance_end {
            range.sample_start = range
                .sample_start
                .max(previous_end.min(utterance.sample_start));
        }
    }
    if range.sample_start < last_long_silence_end
        && last_long_silence_end < range.sample_end
        && last_long_silence_end > utterance.sample_start.saturating_sub(pad_samples)
    {
        // Fence is inside the requested pad — clip, never cross.
        range.sample_start = last_long_silence_end.max(utterance.sample_start);
    }

    if mode == FusionContextMode::SymmetricPad {
        let mut want = utterance.sample_end.saturating_add(pad_samples);
        if bounds.capture_end > 0 {
            want = want.min(bounds.capture_end.max(utterance.sample_end));
        }
        if let Some(next_start) = bounds.next_utterance_start {
            want = want.min(next_start.max(utterance.sample_end));
        }
        range.sample_end = want.max(utterance.sample_end);
    }

    if range.sample_start > range.sample_end {
        range.sample_start = range.sample_end;
    }
    range
}

/// One word pinned to a PCM range for fusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionWord {
    pub text: String,
    pub sample_start: u64,
    pub sample_end: u64,
}

impl FusionWord {
    pub fn from_timed(segment: &TimedTailSegment) -> Self {
        Self {
            text: segment.text.clone(),
            sample_start: segment.range.sample_start,
            sample_end: segment.range.sample_end,
        }
    }

    fn midpoint(&self) -> u64 {
        self.sample_start + (self.sample_end.saturating_sub(self.sample_start) / 2)
    }
}

/// Assign Apple words to Silero utterances by PCM overlap. Words that fall
/// in no utterance are returned as leftovers (caller receipts `no_time_overlap`).
pub fn slice_apple_words(
    ledger: &UtteranceLedger,
    words: &[FusionWord],
) -> (Vec<(u64, Vec<FusionWord>)>, Vec<FusionWord>) {
    let mut leftover = Vec::new();
    let mut by_id: std::collections::BTreeMap<u64, Vec<FusionWord>> =
        std::collections::BTreeMap::new();
    for word in words {
        match ledger.utterance_covering(word.midpoint()) {
            Some(utterance) => by_id.entry(utterance.id).or_default().push(word.clone()),
            None => leftover.push(word.clone()),
        }
    }
    (by_id.into_iter().collect(), leftover)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: u64, end: u64) -> FusionWord {
        FusionWord {
            text: text.to_string(),
            sample_start: start,
            sample_end: end,
        }
    }

    fn range(start: u64, end: u64) -> TailSampleRange {
        TailSampleRange {
            session: "s".into(),
            capture_epoch: 0,
            sample_start: start,
            sample_end: end,
        }
    }

    /// The unification claim, stated as a test: **one** observation of the
    /// spectrum produces both the ledger identity and the lifecycle edge bit.
    /// Two speech segments split by a closing edge mint two identities, and the
    /// `speech_live` the epoch gate reads is true exactly across those two
    /// segments and false in the silence between them.
    #[test]
    fn one_observation_feeds_both_identity_and_the_lifecycle_edge() {
        let mut ingress = SileroIngress::new(16_000, "s", 0);

        // Segment 1: open at 0, still open, then close inside the third chunk.
        let a = ingress.observe(Some((0, 8_000)), false, 8_000);
        assert_eq!(a.open, Some(1));
        assert!(a.speech_live, "an open segment is a live speech edge");
        let b = ingress.observe(Some((0, 16_000)), false, 16_000);
        assert_eq!(b.open, Some(1), "an extending segment keeps its identity");
        let close = ingress.observe(None, true, 24_000);
        assert_eq!(close.closed, vec![1]);
        assert!(
            close.speech_live,
            "the chunk a segment closes in is still speech — the silence \
             counter starts after Silero's own hysteresis, never before it"
        );

        // Long silence: no edge, no identity.
        for cursor in [32_000u64, 40_000, 48_000] {
            let quiet = ingress.observe(None, false, cursor);
            assert!(!quiet.speech_live, "silence is not a speech edge");
            assert!(quiet.closed.is_empty());
            assert_eq!(quiet.open, None);
        }

        // Segment 2 past the long-silence fence: a NEW identity, not an extend.
        let fence = (LONG_SILENCE_FENCE_SECS * 16_000.0) as u64;
        let second_start = 24_000 + fence + 8_000;
        let c = ingress.observe(Some((second_start, second_start + 8_000)), false, 56_000);
        assert_eq!(
            c.open,
            Some(2),
            "speech after a closing edge mints a second utterance"
        );
        assert!(c.speech_live);

        let ledger = ingress.ledger();
        assert_eq!(ledger.utterances().len(), 2);
        assert_eq!(ledger.utterances()[0].range.sample_start, 0);
        assert_eq!(ledger.utterances()[0].range.sample_end, 24_000);
        assert!(ledger.utterances()[0].closed);
        assert_eq!(ledger.utterances()[1].range.sample_start, second_start);
        assert!(!ledger.utterances()[1].closed);
        assert!(
            ledger.utterances()[1].range.sample_start - ledger.utterances()[0].range.sample_end
                >= fence,
            "fixture must actually clear the long-silence fence"
        );
    }

    /// Sideband claims stop exactly where Silero's evidence stops: threshold
    /// edges plus an unknown non-speech pause between them.
    #[test]
    fn sideband_edges_and_pause_keep_exact_pcm_ranges_and_order() {
        let mut ingress = SileroIngress::new(16_000, "s", 4);
        let first = ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 8_000,
                speech_probability: 0.81,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 24_000,
                speech_probability: 0.12,
            },
        ]);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].sequence, 1);
        assert_eq!(first[0].range.sample_start, 8_000);
        assert_eq!(first[0].range.sample_end, 8_000);
        assert!(matches!(
            first[0].evidence,
            SidebandEvidenceKind::SpeechStart { .. }
        ));
        assert_eq!(first[1].sequence, 2);
        assert_eq!(first[1].range.sample_start, 24_000);
        assert_eq!(first[1].range.sample_end, 24_000);

        let resumed = ingress.observe_boundaries(&[VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 40_000,
            speech_probability: 0.76,
        }]);
        assert_eq!(resumed.len(), 2, "pause then the resuming speech edge");
        assert_eq!(resumed[0].sequence, 3);
        assert_eq!(resumed[0].range.sample_start, 24_000);
        assert_eq!(resumed[0].range.sample_end, 40_000);
        assert!(matches!(
            resumed[0].evidence,
            SidebandEvidenceKind::Pause {
                duration_samples: 16_000,
                non_speech: NonSpeechEvidence::UnknownNonSpeech,
            }
        ));
        assert_eq!(resumed[1].sequence, 4);
        assert!(matches!(
            resumed[1].evidence,
            SidebandEvidenceKind::SpeechStart { .. }
        ));
    }

    /// Long hands-free takes cannot retain one sideband row per speech edge
    /// forever; sequence identity remains global while lookup memory is
    /// capped to the newest evidence.
    #[test]
    fn retained_sideband_evidence_is_bounded_without_reusing_sequence_ids() {
        let mut ingress = SileroIngress::new(16_000, "long", 1);
        for index in 0..(MAX_RETAINED_SIDEBAND_EVIDENCE + 20) {
            let emitted = ingress.observe_boundaries(&[VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: index as u64,
                speech_probability: 0.1,
            }]);
            assert_eq!(emitted.len(), 1);
        }

        assert_eq!(ingress.sideband.len(), MAX_RETAINED_SIDEBAND_EVIDENCE);
        assert_eq!(
            ingress.sideband.front().expect("retained first").sequence,
            21
        );
        assert_eq!(
            ingress.sideband.back().expect("retained last").sequence,
            (MAX_RETAINED_SIDEBAND_EVIDENCE + 20) as u64
        );
    }

    /// Enclosure, not overlap: a span may only adopt a Silero range that
    /// already covers every sample it claimed.
    #[test]
    fn enclosure_is_required_before_a_span_adopts_a_silero_range() {
        let mut ledger = UtteranceLedger::new();
        ledger.open_or_extend("s", 0, 10_000, 30_000);
        ledger.close_open(30_000);

        let enclosed = ledger
            .utterance_enclosing(12_000, 20_000)
            .expect("a span inside the edge binds to it");
        assert_eq!(enclosed.id, 1);
        assert_eq!(enclosed.range.sample_start, 10_000);
        assert_eq!(enclosed.range.sample_end, 30_000);

        assert!(
            ledger.utterance_enclosing(5_000, 20_000).is_none(),
            "a span starting before the edge must NOT adopt it"
        );
        assert!(
            ledger.utterance_enclosing(20_000, 40_000).is_none(),
            "a span ending after the edge must NOT adopt it"
        );
        assert!(
            ledger.utterance_enclosing(80_000, 90_000).is_none(),
            "no edge at all is fail-open, not a panic"
        );
    }

    /// One ledger, one id space: an id burnt by the Apple-boundary fallback is
    /// never re-minted for a real utterance.
    #[test]
    fn reserved_ids_are_never_reused_by_a_minted_utterance() {
        let mut ledger = UtteranceLedger::new();
        assert_eq!(ledger.reserve_id(), 1);
        assert_eq!(ledger.reserve_id(), 2);
        assert_eq!(
            ledger.open_or_extend("s", 0, 0, 1_000),
            3,
            "minting must continue past every reserved id"
        );
        assert_eq!(ledger.utterances().len(), 1, "a reservation is not a span");
    }

    #[test]
    fn apple_words_slice_onto_silero_edges() {
        let mut ledger = UtteranceLedger::new();
        ledger.open_or_extend("s", 0, 0, 24_000);
        ledger.close_open(24_000);
        ledger.open_or_extend("s", 0, 32_000, 48_000);
        let words = vec![
            word("alpha", 1_000, 8_000),
            word("beta", 33_000, 40_000),
            word("orphan", 80_000, 88_000),
        ];
        let (sliced, leftover) = slice_apple_words(&ledger, &words);
        assert_eq!(sliced.len(), 2);
        assert_eq!(sliced[0].1[0].text, "alpha");
        assert_eq!(sliced[1].1[0].text, "beta");
        assert_eq!(leftover.len(), 1);
        assert_eq!(leftover[0].text, "orphan");
    }

    #[test]
    fn left_pad_never_crosses_long_silence() {
        let utterance = range(48_000, 64_000);
        let bounds = ContextBounds {
            long_silence_fence: 40_000,
            capture_end: 80_000,
            ..ContextBounds::default()
        };
        let padded =
            bound_context_range(&utterance, FusionContextMode::LeftAudioPad, 16_000, &bounds);
        assert_eq!(padded.sample_start, bounds.long_silence_fence);
        assert_eq!(padded.sample_end, 64_000);

        let utterance_only = bound_context_range(
            &utterance,
            FusionContextMode::UtteranceOnly,
            16_000,
            &bounds,
        );
        assert_eq!(utterance_only.sample_start, 48_000);
        assert_eq!(utterance_only.sample_end, 64_000);

        let prompt = bound_context_range(
            &utterance,
            FusionContextMode::StableTextPrompt,
            16_000,
            &bounds,
        );
        assert_eq!(prompt.sample_start, 48_000);
        assert_eq!(prompt.sample_end, 64_000);
    }

    /// The default cut reaches both ways. 400 ms at 16 kHz is 6 400 samples;
    /// nothing in the way means the decoder hears the run-up and the run-out.
    #[test]
    fn symmetric_context_reaches_both_sides_when_nothing_fences_it() {
        let utterance = range(48_000, 64_000);
        let bounds = ContextBounds {
            capture_end: 160_000,
            ..ContextBounds::default()
        };

        let padded =
            bound_context_range(&utterance, FusionContextMode::SymmetricPad, 6_400, &bounds);

        assert_eq!(padded.sample_start, 41_600);
        assert_eq!(padded.sample_end, 70_400);
        assert_eq!(
            FusionContextMode::default(),
            FusionContextMode::SymmetricPad,
            "symmetric context is the default cut, not an env-only mode"
        );
    }

    /// Every limit is hard. Whichever binds first wins, and none of them may
    /// push a bound back inside the utterance the caller owns.
    #[test]
    fn symmetric_context_stops_at_fence_eof_and_neighbours() {
        let utterance = range(48_000, 64_000);

        let fenced = bound_context_range(
            &utterance,
            FusionContextMode::SymmetricPad,
            6_400,
            &ContextBounds {
                long_silence_fence: 44_000,
                capture_end: 160_000,
                ..ContextBounds::default()
            },
        );
        assert_eq!(
            fenced.sample_start, 44_000,
            "left context clips at the fence"
        );

        let at_eof = bound_context_range(
            &utterance,
            FusionContextMode::SymmetricPad,
            6_400,
            &ContextBounds {
                capture_end: 66_000,
                ..ContextBounds::default()
            },
        );
        assert_eq!(
            at_eof.sample_end, 66_000,
            "right context cannot claim audio the session never captured"
        );

        let crowded = bound_context_range(
            &utterance,
            FusionContextMode::SymmetricPad,
            6_400,
            &ContextBounds {
                capture_end: 160_000,
                next_utterance_start: Some(67_000),
                previous_utterance_end: Some(45_000),
                ..ContextBounds::default()
            },
        );
        assert_eq!(
            crowded.sample_start, 45_000,
            "left context stops after the previous span"
        );
        assert_eq!(
            crowded.sample_end, 67_000,
            "right context stops before the next span"
        );

        // A capture cursor already behind the utterance end (EOF quantisation)
        // must not invert the window or shrink what the caller asked to decode.
        let quantised = bound_context_range(
            &utterance,
            FusionContextMode::SymmetricPad,
            6_400,
            &ContextBounds {
                capture_end: 63_000,
                ..ContextBounds::default()
            },
        );
        assert_eq!(quantised.sample_start, 41_600);
        assert_eq!(quantised.sample_end, 64_000);
        assert!(quantised.sample_start <= quantised.sample_end);
    }

    /// Padding is context. It never becomes ownership: the utterance range the
    /// caller mints occurrence identity from is untouched by any mode, and the
    /// context window always contains it.
    #[test]
    fn decode_context_never_widens_the_owned_utterance_range() {
        let utterance = range(48_000, 64_000);
        let bounds = ContextBounds {
            capture_end: 160_000,
            ..ContextBounds::default()
        };

        for mode in [
            FusionContextMode::SymmetricPad,
            FusionContextMode::UtteranceOnly,
            FusionContextMode::LeftAudioPad,
            FusionContextMode::StableTextPrompt,
        ] {
            let context = bound_context_range(&utterance, mode, 6_400, &bounds);
            assert!(
                context.sample_start <= utterance.sample_start
                    && context.sample_end >= utterance.sample_end,
                "{} context must contain the owned range",
                mode.as_str()
            );
            assert_eq!(context.session, utterance.session);
            assert_eq!(context.capture_epoch, utterance.capture_epoch);
        }

        // The owned range is a separate value and stays exactly as minted.
        assert_eq!(utterance.sample_start, 48_000);
        assert_eq!(utterance.sample_end, 64_000);
    }

    /// Two bursts of speech with real silence between them are two acoustic
    /// ranges, symmetrically padded by 64 ms — not one padded window that
    /// swallows the pause. This is the whole point of the set: on an archived
    /// take the utterance view called 99.5% of a 50-minute recording speech,
    /// against an offline measurement of 454 s.
    #[test]
    fn acoustic_speech_set_keeps_separate_bursts_separate() {
        let mut ingress = SileroIngress::new(16_000, "bursts", 7);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 16_000,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 32_000,
                speech_probability: 0.1,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 48_000,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 64_000,
                speech_probability: 0.1,
            },
        ]);

        let ranges = ingress.acoustic_speech_ranges(80_000);
        assert_eq!(ranges.len(), 2, "a one-second pause is not speech");
        // 64 ms at 16 kHz = 1 024 samples, applied on both sides.
        assert_eq!(ranges[0].sample_start, 14_976);
        assert_eq!(ranges[0].sample_end, 33_024);
        assert_eq!(ranges[1].sample_start, 46_976);
        assert_eq!(ranges[1].sample_end, 65_024);
        assert_eq!(ranges[0].session, "bursts");
        assert_eq!(ranges[0].capture_epoch, 7);

        let speech_samples: u64 = ranges
            .iter()
            .map(|range| range.sample_end - range.sample_start)
            .sum();
        assert!(
            speech_samples < 80_000,
            "the acoustic set must be smaller than the take, not equal to it"
        );
    }

    /// A gap at or under 200 ms is VAD quantisation inside one phrase, not a
    /// pause the coverage receipt should have to account for twice.
    #[test]
    fn acoustic_speech_set_merges_gaps_up_to_two_hundred_milliseconds() {
        let mut ingress = SileroIngress::new(16_000, "merge", 0);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 16_000,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 32_000,
                speech_probability: 0.1,
            },
            // 150 ms later — 2 400 samples, inside the merge gap once padded.
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 34_400,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 48_000,
                speech_probability: 0.1,
            },
        ]);

        let ranges = ingress.acoustic_speech_ranges(64_000);
        assert_eq!(ranges.len(), 1, "150 ms inside a phrase is one range");
        assert_eq!(ranges[0].sample_start, 14_976);
        assert_eq!(ranges[0].sample_end, 49_024);
    }

    /// Stop mid-word is ordinary. The open crossing is speech up to the capture
    /// cursor, and padding may not invent audio past it.
    #[test]
    fn acoustic_speech_set_closes_an_open_crossing_at_capture_end() {
        let mut ingress = SileroIngress::new(16_000, "eof", 0);
        ingress.observe_boundaries(&[VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechStart,
            sample: 16_000,
            speech_probability: 0.9,
        }]);

        let ranges = ingress.acoustic_speech_ranges(24_000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].sample_start, 14_976);
        assert_eq!(
            ranges[0].sample_end, 24_000,
            "the right pad is clamped at the capture cursor, never past it"
        );

        // `flush` closes the same crossing on the same cursor as the utterance.
        ingress.flush(24_000);
        let flushed = ingress.acoustic_speech_ranges(24_000);
        assert_eq!(flushed, ranges, "flush must not move an EOF boundary");
    }

    /// A 60 ms word is speech. The set records it; nothing in this cut is
    /// allowed to introduce a minimum-duration rejection that silently drops
    /// short legitimate utterances.
    #[test]
    fn acoustic_speech_set_preserves_words_shorter_than_250ms() {
        let mut ingress = SileroIngress::new(16_000, "short", 0);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 16_000,
                speech_probability: 0.9,
            },
            // 60 ms of speech — under every "minimum utterance" figure argued
            // for on this lane.
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 16_960,
                speech_probability: 0.1,
            },
        ]);

        let ranges = ingress.acoustic_speech_ranges(48_000);
        assert_eq!(ranges.len(), 1, "a short word is still speech");
        assert_eq!(ranges[0].sample_start, 14_976);
        assert_eq!(ranges[0].sample_end, 17_984);
    }

    /// Padding at the very start of a take clamps at sample zero rather than
    /// underflowing, and an end crossing with nothing open manufactures no
    /// speech at all.
    #[test]
    fn acoustic_speech_set_clamps_at_zero_and_refuses_orphan_ends() {
        let mut ingress = SileroIngress::new(16_000, "edges", 0);
        ingress.observe_boundaries(&[VadBoundaryEvidence {
            kind: VadBoundaryKind::SpeechEnd,
            sample: 4_000,
            speech_probability: 0.1,
        }]);
        assert!(
            ingress.acoustic_speech_ranges(48_000).is_empty(),
            "an end with no start is not evidence of speech"
        );

        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 100,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 8_000,
                speech_probability: 0.1,
            },
        ]);

        let ranges = ingress.acoustic_speech_ranges(48_000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].sample_start, 0, "the left pad clamps at zero");
        assert_eq!(ranges[0].sample_end, 9_024);
    }

    /// The two sets answer different questions and must not be confused. The
    /// padded utterance view stays open across the pause; the acoustic view
    /// does not.
    #[test]
    fn acoustic_set_and_utterance_ledger_measure_different_things() {
        let mut ingress = SileroIngress::new(16_000, "divergent", 0);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 16_000,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 32_000,
                speech_probability: 0.1,
            },
        ]);
        // One padded STT window spanning the whole take, the way the fusion
        // ledger legitimately mints ownership.
        ingress.observe(Some((0, 160_000)), true, 160_000);

        let utterance_samples: u64 = ingress
            .ledger()
            .utterances()
            .iter()
            .map(|utterance| utterance.range.sample_end - utterance.range.sample_start)
            .sum();
        let acoustic_samples: u64 = ingress
            .acoustic_speech_ranges(160_000)
            .iter()
            .map(|range| range.sample_end - range.sample_start)
            .sum();

        assert_eq!(
            utterance_samples, 160_000,
            "ownership keeps its padded window"
        );
        assert_eq!(acoustic_samples, 18_048, "coverage measures the crossings");
        assert!(
            acoustic_samples < utterance_samples,
            "the coverage set is the smaller, honest one"
        );
    }

    /// Regression proof: active boundary events (UtteranceFinal discriminant) and
    /// VAD sideband evidence survive payload/fusion cleanup with exact PCM ranges.
    #[test]
    fn regression_boundary_emission_and_vad_evidence_survive_payload_cleanup() {
        let mut ingress = SileroIngress::new(16_000, "regression_session", 1);
        let boundaries = vec![
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 16_000,
                speech_probability: 0.88,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 32_000,
                speech_probability: 0.15,
            },
        ];
        let sideband = ingress.observe_boundaries(&boundaries);
        assert_eq!(sideband.len(), 2);
        assert_eq!(sideband[0].range.sample_start, 16_000);
        assert_eq!(sideband[1].range.sample_start, 32_000);
        assert!(matches!(
            sideband[0].evidence,
            SidebandEvidenceKind::SpeechStart { .. }
        ));

        // Observation closed segment check
        let event = SpeechEvent::UtteranceFinal;
        assert!(
            matches!(event, SpeechEvent::UtteranceFinal),
            "unit boundary discriminant must match SpeechEvent::UtteranceFinal"
        );
        let obs = ingress.observe(Some((16_000, 32_000)), true, 32_000);
        assert_eq!(obs.closed, vec![1]);
        assert_eq!(ingress.ledger().utterances().len(), 1);
        assert_eq!(ingress.ledger().utterances()[0].range.sample_start, 16_000);
        assert_eq!(ingress.ledger().utterances()[0].range.sample_end, 32_000);
    }

    /// Executable truth of the context selector, against the registry entry.
    ///
    /// The documented default used to read `utterance`. That token is not one
    /// the parser knows: it falls through to the default arm, which is
    /// [`FusionContextMode::SymmetricPad`], not `UtteranceOnly`. Anyone reading
    /// the old entry and setting `utterance` to get exact-utterance audio got
    /// 400 ms of symmetric context instead. The recognised token is
    /// `utterance_only`; this test is what the registry entry now states.
    #[test]
    fn context_mode_parser_is_symmetric_pad_by_default_and_names_its_own_tokens() {
        assert_eq!(
            FusionContextMode::from_env_value(None),
            FusionContextMode::SymmetricPad
        );
        assert_eq!(
            FusionContextMode::default(),
            FusionContextMode::SymmetricPad
        );

        for token in ["utterance_only", "utterance-only", "exact", "  EXACT  "] {
            assert_eq!(
                FusionContextMode::from_env_value(Some(token)),
                FusionContextMode::UtteranceOnly,
                "{token} selects utterance-only audio"
            );
        }
        for token in ["left_pad", "left-pad", "pad", "Left_Pad"] {
            assert_eq!(
                FusionContextMode::from_env_value(Some(token)),
                FusionContextMode::LeftAudioPad
            );
        }
        for token in ["stable_prompt", "stable-text", "prompt"] {
            assert_eq!(
                FusionContextMode::from_env_value(Some(token)),
                FusionContextMode::StableTextPrompt
            );
        }

        // Malformed, unknown, and the historically documented `utterance` all
        // resolve to the default. Nothing here fails closed or panics, and none
        // of them silently selects utterance-only.
        for token in ["utterance", "", "   ", "symmetric", "left pad", "0"] {
            assert_eq!(
                FusionContextMode::from_env_value(Some(token)),
                FusionContextMode::SymmetricPad,
                "{token:?} is not a recognised token and must fall back to the default"
            );
        }

        assert_eq!(FusionContextMode::SymmetricPad.as_str(), "symmetric_pad");
        assert_eq!(FusionContextMode::UtteranceOnly.as_str(), "utterance_only");
        assert_eq!(FusionContextMode::LeftAudioPad.as_str(), "left_audio_pad");
        assert_eq!(
            FusionContextMode::StableTextPrompt.as_str(),
            "stable_text_prompt"
        );
        assert!(
            (DEFAULT_SYMMETRIC_PAD_SECS - 0.40).abs() < f32::EPSILON,
            "the registry entry states 400 ms each side"
        );
    }

    /// Two neighbouring utterances whose 400 ms pads would overlap.
    ///
    /// Overlapping decode context is allowed and often better: both decoders may
    /// listen into the silence between the words. What neither may do is reach a
    /// sample the other utterance owns. Ownership is minted from the unpadded
    /// range and is untouched by any of this.
    #[test]
    fn overlapping_decode_context_stops_at_the_neighbour_and_never_moves_ownership() {
        let rate = 16_000_f32;
        let pad = (DEFAULT_SYMMETRIC_PAD_SECS * rate).round() as u64;
        // 0.5 s apart: 6 400 samples of pad on each side of a 8 000-sample gap.
        let first = range(0, 16_000);
        let second = range(24_000, 40_000);

        let first_context = bound_context_range(
            &first,
            FusionContextMode::SymmetricPad,
            pad,
            &ContextBounds {
                long_silence_fence: 0,
                capture_end: 40_000,
                next_utterance_start: Some(second.sample_start),
                previous_utterance_end: None,
            },
        );
        let second_context = bound_context_range(
            &second,
            FusionContextMode::SymmetricPad,
            pad,
            &ContextBounds {
                long_silence_fence: 0,
                capture_end: 40_000,
                next_utterance_start: None,
                previous_utterance_end: Some(first.sample_end),
            },
        );

        assert!(
            first_context.sample_end <= second.sample_start,
            "left neighbour's context reached into the right neighbour's owned PCM"
        );
        assert!(
            second_context.sample_start >= first.sample_end,
            "right neighbour's context reached into the left neighbour's owned PCM"
        );
        assert!(
            first_context.sample_end > second_context.sample_start,
            "this fixture must actually overlap, or it proves nothing"
        );
        assert_eq!(
            first_context.sample_start, 0,
            "clamped at the session start"
        );
        assert_eq!(second_context.sample_end, 40_000, "clamped at captured PCM");

        // The owned ranges are separate values and were not touched.
        assert_eq!((first.sample_start, first.sample_end), (0, 16_000));
        assert_eq!((second.sample_start, second.sample_end), (24_000, 40_000));
    }

    /// No observed crossing means an empty acoustic set — at any capture cursor,
    /// and whether the VAD never fired or never loaded.
    ///
    /// This set is evidence of speech, never evidence of silence: emptiness here
    /// says only that this instrument produced nothing. Deciding what an empty
    /// set means belongs to the coverage producer selection in
    /// `apple_live_session`, which falls back rather than calling the take
    /// trivially complete.
    #[test]
    fn an_unobserved_vad_produces_an_empty_acoustic_set_not_a_silent_one() {
        let mut ingress = SileroIngress::new(16_000, "no-crossing", 3);

        assert!(ingress.acoustic_speech_ranges(0).is_empty());
        assert!(ingress.acoustic_speech_ranges(16_000).is_empty());
        assert!(
            ingress.acoustic_speech_ranges(48_000_000).is_empty(),
            "a long take with no crossing still measures no speech, not all of it"
        );

        // An empty boundary batch is not an observation either.
        assert!(ingress.observe_boundaries(&[]).is_empty());
        assert!(ingress.acoustic_speech_ranges(16_000).is_empty());

        // The utterance ledger is a different question and is also empty here:
        // neither set invents a range for audio nobody measured.
        assert!(ingress.ledger().utterances().is_empty());
    }

    /// A `SpeechEnd` at the exact capture cursor closes the range there, and the
    /// right pad cannot claim a sample past what was captured.
    #[test]
    fn a_crossing_at_end_of_capture_is_padded_only_to_the_last_captured_sample() {
        let mut ingress = SileroIngress::new(16_000, "eof", 0);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 8_000,
                speech_probability: 0.91,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 16_000,
                speech_probability: 0.12,
            },
        ]);

        let ranges = ingress.acoustic_speech_ranges(16_000);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].sample_start, 8_000 - 1_024);
        assert_eq!(
            ranges[0].sample_end, 16_000,
            "the 64 ms right pad is clamped at end of captured PCM"
        );
        assert_eq!(ranges[0].session, "eof");
    }

    /// The evidence surface takes no caller endpoint: it reports the extent the
    /// ingress recorded, and a hole in that extent refuses instead of letting
    /// the unobserved part read as silence.
    #[test]
    fn evidence_reports_the_recorded_extent_and_refuses_a_hole() {
        let mut ingress = SileroIngress::new(16_000, "extent", 3);
        assert_eq!(ingress.observed_samples(), 0);
        assert_eq!(
            ingress.acoustic_speech_evidence().availability(),
            AcousticAvailability::NotObserved,
            "an unfed ingress has measured nothing"
        );

        // An empty chunk observes nothing and must not advance the extent.
        ingress.ingest(&[], 16_000);
        assert_eq!(ingress.observed_samples(), 0);
        assert_eq!(
            ingress.acoustic_speech_evidence().availability(),
            AcousticAvailability::NotObserved
        );

        ingress.note_observed_pcm(8_000, 8_000);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 2_000,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 4_000,
                speech_probability: 0.1,
            },
        ]);
        let evidence = ingress.acoustic_speech_evidence();
        assert_eq!(ingress.observed_samples(), 8_000);
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::Observed {
                observed_samples: 8_000
            }
        );
        assert_eq!(evidence.producer(), SILERO_BOUNDARIES_PRODUCER);
        assert_eq!(evidence.identity().session, "extent");
        assert_eq!(evidence.identity().capture_epoch, 3);
        assert_eq!(evidence.ranges().len(), 1);

        // A chunk that skips ahead leaves audio this observer never heard.
        ingress.note_observed_pcm(4_000, 24_000);
        assert_eq!(
            ingress.acoustic_speech_evidence().availability(),
            AcousticAvailability::Discontinuous {
                observed_samples: 24_000
            },
            "a discontinuous tail cannot certify anything"
        );
        assert!(
            ingress.acoustic_speech_evidence().ranges().is_empty(),
            "unavailable evidence reports no ranges"
        );
    }

    /// rc-w3-acoustic-validity: invalid PCM reaching the VAD is unmeasured, not
    /// silent, and it outranks the continuity verdict.
    ///
    /// The model returns a probability for whatever it is handed. NaN and
    /// infinities fail every threshold comparison, so they leave no crossing at
    /// all — the exact shape of a silent stretch. This ingress therefore has to
    /// record validity at ingest, the same way it records the extent.
    #[test]
    fn invalid_pcm_is_unmeasured_not_silent() {
        let mut ingress = SileroIngress::new(16_000, "validity", 5);
        ingress.note_observed_pcm(8_000, 8_000);
        ingress.observe_boundaries(&[
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechStart,
                sample: 2_000,
                speech_probability: 0.9,
            },
            VadBoundaryEvidence {
                kind: VadBoundaryKind::SpeechEnd,
                sample: 4_000,
                speech_probability: 0.1,
            },
        ]);
        assert!(
            ingress.acoustic_speech_evidence().observed_speech(),
            "the fixture starts from a valid observation with a crossing in it"
        );

        // The next chunk continues the extent and carries NaN.
        ingress.note_observed_pcm(8_000, 16_000);
        ingress.note_pcm_validity(&vec![f32::NAN; 8_000], 16_000);
        let evidence = ingress.acoustic_speech_evidence();
        assert_eq!(
            evidence.availability(),
            AcousticAvailability::InvalidMeasurement {
                valid_samples: 8_000
            },
            "the invalid chunk starts where the valid extent ended"
        );
        assert!(
            evidence.ranges().is_empty(),
            "an earlier real crossing is not published on refused evidence"
        );
        assert!(!evidence.observed_speech());

        // Validity outranks continuity: a later hole does not rename the fault.
        ingress.note_observed_pcm(4_000, 32_000);
        assert_eq!(
            ingress.acoustic_speech_evidence().availability(),
            AcousticAvailability::InvalidMeasurement {
                valid_samples: 8_000
            },
            "audio that arrived unreadable is a different fact from audio that \
             never arrived, and the first one happened first"
        );

        // `ingest` records validity itself, before the model reads the chunk.
        let mut fed = SileroIngress::new(16_000, "validity-ingest", 5);
        fed.ingest(&vec![f32::NEG_INFINITY; 1_600], 1_600);
        assert_eq!(fed.observed_samples(), 1_600);
        assert_eq!(
            fed.acoustic_speech_evidence().availability(),
            AcousticAvailability::InvalidMeasurement { valid_samples: 0 }
        );

        // A finite chunk is not marked, so valid captures are untouched.
        let mut clean = SileroIngress::new(16_000, "validity-clean", 5);
        clean.ingest(&vec![0.0f32; 1_600], 1_600);
        assert_eq!(
            clean.acoustic_speech_evidence().availability(),
            AcousticAvailability::Observed {
                observed_samples: 1_600
            },
            "finite silence stays a measurement"
        );
    }
}
