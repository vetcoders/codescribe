//! PCM anchors: physical admission, capture-rate binding, acoustic serials,
//! word evidence, derivation and signed composition.
//!
//! Everything here maps a claim onto captured samples. An occurrence earns an
//! [`AcousticSerial`] only after [`AcousticEvidence`] proves the region exists;
//! every emitted token carries a [`WordEvidenceReceipt`] back to that serial,
//! and [`OccurrenceComposition`] is the signed sequence a reducer reads.

use super::*;

// ---------------------------------------------------------------------------
// Physical evidence, calibration, and admission
// ---------------------------------------------------------------------------

/// The named, measured existence threshold an occurrence must clear.
///
/// * inputs: values supplied by the immutable runtime settings snapshot.
/// * outputs: the qualification verdict in [`AcousticLedger::qualify`] and the
///   valley floor in [`AcousticLedger::record_derivation`].
/// * invariants: there is no `Default`. A threshold this ledger invented itself
///   would be a runtime calibration decision, and W1 is not allowed to make
///   one; the caller must state the measured floor and the version it was
///   measured under.
/// * intended W2 consumers: the capture/VAD evidence path that qualifies PCM
///   before any producer is allowed to speak about it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnergyCalibration {
    /// Version label of the calibration run these floors came from. It travels
    /// into every serial so a receipt can never be read under the wrong ruler.
    pub version: String,
    /// Minimum energy integral a region must reach to exist at all.
    pub min_energy_integral: f64,
    /// Minimum silent samples that must separate two regions before they may be
    /// called two physical events.
    pub min_valley_samples: u64,
}

impl EnergyCalibration {
    /// Build a calibration from measured floors.
    pub fn new(
        version: impl Into<String>,
        min_energy_integral: f64,
        min_valley_samples: u64,
    ) -> Self {
        Self {
            version: version.into(),
            min_energy_integral,
            min_valley_samples,
        }
    }
}

/// Physical evidence measured over one candidate PCM region.
///
/// * inputs: the capture clock, the energy hops the session already records,
///   and the VAD boundaries Silero already emits.
/// * outputs: [`AcousticSerial`] and the admission verdict.
/// * invariants: no field here is ever part of an identity key. Energy and
///   dBFS *qualify* a region; they never say which region it is.
/// * intended W2 consumers: `core/audio/streaming_recorder.rs` and the VAD
///   evidence path.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcousticEvidence {
    /// The physical coordinate the evidence was measured over.
    pub occurrence: OccurrenceIdentity,
    /// Duration of the region in milliseconds on the capture clock.
    pub duration_ms: f64,
    /// Summed `rms^2 * sample_count` over the region's hops that reach the
    /// calibrated existence floor, the same floor `min_energy_integral` is
    /// derived from. Sub-floor hops do not add up to existence.
    pub energy_integral: f64,
    /// Mean RMS of the region in dBFS.
    pub mean_rms_dbfs: f64,
    /// Peak level of the region in dBFS.
    pub peak_dbfs: f64,
    /// Sample at which VAD opened the region, when it opened one.
    pub vad_open_sample: Option<u64>,
    /// Sample at which VAD closed the region, when it closed one. `None` is the
    /// N5 shape: real energy, no closing boundary, therefore never sealable.
    pub vad_close_sample: Option<u64>,
    /// Calibration version the energy figures were measured under.
    pub evidence_calibration_version: String,
}

/// Why a candidate region was refused admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// The range names no audio (zero-width or reversed).
    ZeroWidth,
    /// Energy stayed under the calibrated floor: below-threshold noise, not
    /// speech.
    BelowCalibratedEnergy,
    /// VAD never opened at or before the first sample, so nothing bounded the
    /// region.
    VadDidNotOpen,
    /// The evidence was measured under a different calibration than the one the
    /// caller is judging it with. Comparing them would silently change the
    /// meaning of the floor.
    CalibrationMismatch,
}

impl AdmissionRefusal {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZeroWidth => "zero_width",
            Self::BelowCalibratedEnergy => "below_calibrated_energy",
            Self::VadDidNotOpen => "vad_did_not_open",
            Self::CalibrationMismatch => "calibration_mismatch",
        }
    }
}

/// The ledger's answer to "did speech physically happen here?".
///
/// * inputs: [`AcousticEvidence`] and [`EnergyCalibration`].
/// * outputs: a minted [`AcousticSerial`] on qualification, a typed refusal
///   otherwise.
/// * invariants: no lexical content participates in this decision, in either
///   direction.
/// * intended W2 consumers: capture/VAD, which must qualify a region before any
///   producer may submit an observation for it.
#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionReceipt {
    /// The region exists. Its mandatory serial is minted and held.
    Qualified {
        /// The physical event.
        occurrence: OccurrenceIdentity,
        /// The evidence receipt every later token must be able to cite.
        serial: AcousticSerial,
    },
    /// The region does not qualify as physical speech.
    Refused {
        /// The coordinate that was offered.
        occurrence: OccurrenceIdentity,
        /// Why it was refused.
        reason: AdmissionRefusal,
    },
}

impl AdmissionReceipt {
    /// Whether an occurrence entered the ledger's evidence table.
    pub fn is_qualified(&self) -> bool {
        matches!(self, Self::Qualified { .. })
    }

    /// The minted serial, when the region qualified.
    pub fn serial(&self) -> Option<&AcousticSerial> {
        match self {
            Self::Qualified { serial, .. } => Some(serial),
            Self::Refused { .. } => None,
        }
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Qualified { .. } => "qualified",
            Self::Refused { .. } => "refused",
        }
    }
}

// ---------------------------------------------------------------------------
// Mandatory versioned acoustic serial
// ---------------------------------------------------------------------------

/// Version of the canonical serial input below. Bump it whenever a field is
/// added, removed, or reinterpreted, so an old receipt can never be silently
/// re-read under new rules.
pub const ACOUSTIC_SERIAL_VERSION: u16 = 1;

/// Domain separator for the serial digest, so a digest of these bytes can never
/// collide with a digest taken elsewhere in the product.
pub(super) const ACOUSTIC_SERIAL_DOMAIN: &str = "codescribe.acoustic-serial.v1";

/// The mandatory, versioned receipt of physical evidence for one occurrence.
///
/// * inputs: [`AcousticEvidence`] at qualification time.
/// * outputs: the digest and evidence fields every emitted token must cite
///   through a [`WordEvidenceReceipt`].
/// * invariants: **a serial is not an identity key.** It deliberately does not
///   implement `Eq`, `Hash`, or `Ord`, so the type system alone prevents it from
///   ever becoming the key of a map, a set, or a sorted ledger. The only key is
///   [`OccurrenceIdentity`]. The digest is deterministic: identical evidence
///   under an identical calibration yields an identical serial, with no clock,
///   counter, or ordering in its input.
/// * intended W2 consumers: the reducer, the Transcript Bus projection, the
///   bridge, and the Swift overlay — all of which read it and none of which
///   mint it.
#[derive(Debug, Clone, PartialEq)]
pub struct AcousticSerial {
    /// Version of the canonical input this digest was taken over.
    pub version: u16,
    /// Lowercase hex SHA-256 over the canonical input.
    pub digest: String,
    /// The physical coordinate the serial is rooted in.
    pub occurrence: OccurrenceIdentity,
    /// Duration of the region in milliseconds.
    pub duration_ms: f64,
    /// Calibrated energy integral (area) of the region.
    pub energy_integral: f64,
    /// Mean RMS of the region in dBFS.
    pub mean_rms_dbfs: f64,
    /// Peak level of the region in dBFS.
    pub peak_dbfs: f64,
    /// VAD opening boundary, when one exists.
    pub vad_open_sample: Option<u64>,
    /// VAD closing boundary, when one exists. Its absence is what keeps an
    /// energy-qualified occurrence unsealable.
    pub vad_close_sample: Option<u64>,
    /// Calibration version the energy figures were measured under.
    pub evidence_calibration_version: String,
}

impl AcousticSerial {
    /// Mint the serial for one qualified region.
    ///
    /// Called only by [`AcousticLedger::qualify`]: minting is an admission
    /// consequence, never a producer's privilege.
    pub fn mint(evidence: &AcousticEvidence) -> Self {
        let digest = Self::digest_of(evidence);
        Self {
            version: ACOUSTIC_SERIAL_VERSION,
            digest,
            occurrence: evidence.occurrence.clone(),
            duration_ms: evidence.duration_ms,
            energy_integral: evidence.energy_integral,
            mean_rms_dbfs: evidence.mean_rms_dbfs,
            peak_dbfs: evidence.peak_dbfs,
            vad_open_sample: evidence.vad_open_sample,
            vad_close_sample: evidence.vad_close_sample,
            evidence_calibration_version: evidence.evidence_calibration_version.clone(),
        }
    }

    /// The exact byte string the digest is taken over.
    ///
    /// Floats enter as their IEEE-754 bit patterns rather than as formatted
    /// decimals: a decimal rendering is locale- and precision-dependent, and a
    /// receipt whose digest depends on how a number was printed is not a
    /// receipt.
    pub fn canonical_input(evidence: &AcousticEvidence) -> String {
        let boundary = |sample: Option<u64>| match sample {
            Some(value) => value.to_string(),
            None => "none".to_string(),
        };
        [
            ACOUSTIC_SERIAL_DOMAIN.to_string(),
            ACOUSTIC_SERIAL_VERSION.to_string(),
            evidence.occurrence.session.clone(),
            evidence.occurrence.capture_epoch.to_string(),
            evidence.occurrence.sample_start.to_string(),
            evidence.occurrence.sample_end.to_string(),
            evidence.duration_ms.to_bits().to_string(),
            evidence.energy_integral.to_bits().to_string(),
            evidence.mean_rms_dbfs.to_bits().to_string(),
            evidence.peak_dbfs.to_bits().to_string(),
            boundary(evidence.vad_open_sample),
            boundary(evidence.vad_close_sample),
            evidence.evidence_calibration_version.clone(),
        ]
        .join("\n")
    }

    /// Lowercase hex SHA-256 over [`AcousticSerial::canonical_input`].
    pub fn digest_of(evidence: &AcousticEvidence) -> String {
        let mut hasher = Sha256::new();
        hasher.update(Self::canonical_input(evidence).as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether VAD closed the region at or after its last sample.
    ///
    /// This is the seal's boundary evidence, and the reason an energy-only
    /// region stays open forever.
    pub fn vad_closed(&self) -> bool {
        self.vad_close_sample
            .is_some_and(|close| close >= self.occurrence.sample_end)
    }
}

// ---------------------------------------------------------------------------
// Word evidence
// ---------------------------------------------------------------------------

/// Why a token could not be given an evidence receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceRefusal {
    /// The token cited no acoustic serial at all. This is the fail-closed edge:
    /// an unsigned token is not emitted, it is refused.
    NoSerialCited,
    /// The token carried no text to attribute.
    EmptyToken,
    /// The declared token coverage falls outside every serial it cites, so the
    /// citation does not actually support the token.
    CoverageOutsideCitedSerials,
    /// A composed occurrence has no serial, so its tokens cannot be signed.
    OccurrenceNotQualified,
}

impl EvidenceRefusal {
    /// Stable label for receipts and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSerialCited => "no_serial_cited",
            Self::EmptyToken => "empty_token",
            Self::CoverageOutsideCitedSerials => "coverage_outside_cited_serials",
            Self::OccurrenceNotQualified => "occurrence_not_qualified",
        }
    }
}

/// The mandatory signature one emitted lexical token carries.
///
/// * inputs: the token text, its observation-local ordinal, the producer that
///   emitted it, and one or more [`AcousticSerial`]s.
/// * outputs: the per-word evidence row the runtime trace and the overlay read.
/// * invariants: at least one serial is always cited — construction fails
///   closed otherwise. Citing a serial does **not** make the serial a second
///   identity key: many tokens may cite one occurrence, and one token may cite
///   several, precisely because the citation is a receipt and not a key.
/// * intended W2 consumers: `app/presentation/emitter.rs`,
///   `app/presentation/transcript_bus.rs`, and the overlay projection.
#[derive(Debug, Clone, PartialEq)]
pub struct WordEvidenceReceipt {
    /// The emitted lexical token. A label, never a key.
    pub token: String,
    /// Ordinal of this token inside the observation that emitted it.
    pub token_ordinal: usize,
    /// Producer that emitted the token.
    pub producer: ObservationProducer,
    /// Producer-local generation the token was emitted at.
    pub generation: u64,
    /// Serials the token is rooted in. Never empty.
    pub serials: Vec<AcousticSerial>,
    /// First sample the producer attributes to this token, when it provides one.
    pub token_sample_start: Option<u64>,
    /// One past the last sample the producer attributes to this token, when it
    /// provides one.
    pub token_sample_end: Option<u64>,
    /// Exact PCM sources. A merged token can cite disjoint ranges; its single
    /// start/end fields remain absent instead of claiming their bounding box.
    pub source_pcm_ranges: Vec<OccurrenceIdentity>,
    /// Boundaries describe a word group rather than this individual token.
    pub group_accuracy: bool,
    /// Per-word acoustic confidence from the producer that emitted this token
    /// (A6). `None` = the producer supplied no metric — never "low".
    pub confidence: Option<crate::pipeline::word_confidence::WordConfidence>,
    /// Live lexicon rewrote this token's surface before admission (d5); the
    /// acoustic confidence still describes the heard audio.
    pub surface_rewritten: bool,
}

impl WordEvidenceReceipt {
    /// Sign one token with the serials it is rooted in, or refuse it.
    pub fn cite(
        token: impl Into<String>,
        token_ordinal: usize,
        observation: &ObservationIdentity,
        serials: Vec<AcousticSerial>,
        coverage: Option<(u64, u64)>,
    ) -> Result<Self, EvidenceRefusal> {
        let token = token.into();
        if token.trim().is_empty() {
            return Err(EvidenceRefusal::EmptyToken);
        }
        if serials.is_empty() {
            return Err(EvidenceRefusal::NoSerialCited);
        }
        if let Some((start, end)) = coverage {
            let supported = serials.iter().any(|serial| {
                start < end
                    && start >= serial.occurrence.sample_start
                    && end <= serial.occurrence.sample_end
            });
            if !supported {
                return Err(EvidenceRefusal::CoverageOutsideCitedSerials);
            }
        }
        Ok(Self {
            token,
            token_ordinal,
            producer: observation.producer,
            generation: observation.generation,
            serials,
            token_sample_start: coverage.map(|(start, _)| start),
            token_sample_end: coverage.map(|(_, end)| end),
            source_pcm_ranges: coverage
                .map(|(start, end)| {
                    vec![OccurrenceIdentity::new(
                        &observation.occurrence.session,
                        observation.occurrence.capture_epoch,
                        start,
                        end,
                    )]
                })
                .unwrap_or_default(),
            group_accuracy: coverage.is_none(),
            confidence: None,
            surface_rewritten: false,
        })
    }

    /// The occurrences this token cites, in citation order.
    pub fn cited_occurrences(&self) -> impl Iterator<Item = &OccurrenceIdentity> {
        self.serials.iter().map(|serial| &serial.occurrence)
    }

    /// The hex digests this token cites, in citation order.
    pub fn cited_digests(&self) -> impl Iterator<Item = &str> {
        self.serials.iter().map(|serial| serial.digest.as_str())
    }
}

// ---------------------------------------------------------------------------
// Derivation and composition
// ---------------------------------------------------------------------------

/// Refinement of physical coverage that mints provenance instead of rewriting
/// history.
///
/// * inputs: a later, finer or coarser segmentation of already-qualified audio.
/// * outputs: new provenance identities recorded next to — never instead of —
///   the coordinates and serials already on the ledger.
/// * invariants: a derivation never edits a parent occurrence, never re-mints a
///   parent serial, and never invents a split that the calibrated valley floor
///   does not support. Token count is not evidence of a physical boundary.
/// * intended W2 consumers: the VAD/segmentation refinement path, and nothing
///   that authors text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccurrenceDerivation {
    /// One qualified region resolved into several, separated by real valleys.
    Split {
        /// The region as it was qualified.
        parent: OccurrenceIdentity,
        /// The finer regions, in capture order.
        children: Vec<OccurrenceIdentity>,
    },
    /// Several qualified regions recognised as one physical event.
    Merge {
        /// The regions as they were qualified, in capture order.
        parents: Vec<OccurrenceIdentity>,
        /// The coarser region they compose.
        child: OccurrenceIdentity,
    },
    /// One region's boundaries tightened without changing how many events exist.
    Refine {
        /// The region as it was qualified.
        parent: OccurrenceIdentity,
        /// The tightened region.
        child: OccurrenceIdentity,
    },
}

impl OccurrenceDerivation {
    /// Coordinates this derivation reads.
    pub fn parents(&self) -> Vec<&OccurrenceIdentity> {
        match self {
            Self::Split { parent, .. } | Self::Refine { parent, .. } => vec![parent],
            Self::Merge { parents, .. } => parents.iter().collect(),
        }
    }

    /// Coordinates this derivation mints.
    pub fn children(&self) -> Vec<&OccurrenceIdentity> {
        match self {
            Self::Split { children, .. } => children.iter().collect(),
            Self::Merge { child, .. } | Self::Refine { child, .. } => vec![child],
        }
    }

    /// Stable label for receipts and logs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Split { .. } => "split",
            Self::Merge { .. } => "merge",
            Self::Refine { .. } => "refine",
        }
    }

    /// Why this derivation may not be recorded, if it may not.
    ///
    /// `None` means the derivation only adds provenance. Every other answer is
    /// a refusal to rewrite PCM history.
    pub fn rejects(&self, calibration: &EnergyCalibration) -> Option<&'static str> {
        let parents = self.parents();
        let children = self.children();
        if parents.is_empty() || children.is_empty() {
            return Some("derivation_without_endpoints");
        }
        let anchor = parents[0];
        if parents
            .iter()
            .chain(children.iter())
            .any(|coordinate| !coordinate.is_anchored())
        {
            return Some("derivation_endpoint_unanchored");
        }
        if parents
            .iter()
            .chain(children.iter())
            .any(|coordinate| !coordinate.same_capture(anchor))
        {
            return Some("derivation_crosses_capture");
        }
        match self {
            Self::Split { parent, children } => {
                if children.len() < 2 {
                    return Some("split_without_two_children");
                }
                if children.iter().any(|child| {
                    child.sample_start < parent.sample_start || child.sample_end > parent.sample_end
                }) {
                    return Some("split_child_outside_parent");
                }
                let mut ordered: Vec<&OccurrenceIdentity> = children.iter().collect();
                ordered.sort();
                let separated = ordered.windows(2).all(|pair| {
                    pair[1].sample_start.saturating_sub(pair[0].sample_end)
                        >= calibration.min_valley_samples
                });
                if !separated {
                    // A2: adjacent lexical tokens without a qualifying valley are
                    // not two physical events, however many words were heard.
                    return Some("split_without_calibrated_valley");
                }
                None
            }
            Self::Merge { parents, child } => {
                if parents.len() < 2 {
                    return Some("merge_without_two_parents");
                }
                if parents.iter().any(|parent| {
                    parent.sample_start < child.sample_start || parent.sample_end > child.sample_end
                }) {
                    return Some("merge_parent_outside_child");
                }
                None
            }
            Self::Refine { parent, child } => {
                if child.sample_start < parent.sample_start || child.sample_end > parent.sample_end
                {
                    return Some("refine_child_outside_parent");
                }
                None
            }
        }
    }
}

/// The ordered lexical composition of one coverage.
///
/// * inputs: the labels the ledger currently holds for the occurrences under a
///   coverage, plus their serials.
/// * outputs: the signed token sequence a reducer projects into a document.
/// * invariants: composition fails closed. Every token in it carries a
///   [`WordEvidenceReceipt`], so an unsigned token cannot reach a document
///   through this part. Occurrence count, not token count, is the conserved
///   quantity: five occurrences labelled `Iwo` compose five signed tokens.
/// * intended W2 consumers: `app/presentation/emitter.rs::TranscriptReducer`.
#[derive(Debug, Clone, PartialEq)]
pub struct OccurrenceComposition {
    /// Coordinate the composition covers.
    pub coverage: OccurrenceIdentity,
    /// Signed tokens, in capture order.
    pub tokens: Vec<WordEvidenceReceipt>,
}

impl OccurrenceComposition {
    /// Distinct occurrences the composed tokens cite.
    pub fn occurrence_count(&self) -> usize {
        self.tokens
            .iter()
            .flat_map(|token| token.cited_occurrences())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Number of signed tokens.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// Whether every token carries at least one serial.
    ///
    /// Construction already guarantees this; the accessor exists so a
    /// projection can assert it at its own boundary instead of trusting ours.
    pub fn is_fully_signed(&self) -> bool {
        self.tokens.iter().all(|token| !token.serials.is_empty())
    }
}

impl AcousticLedger {
    /// Capture rate used to judge clock-lie over a declared sample range.
    pub fn bind_capture_rate(&mut self, sample_rate_hz: u32) {
        if sample_rate_hz > 0 {
            self.capture_rate_hz = Some(sample_rate_hz);
        }
    }

    pub(in crate::pipeline) fn capture_rate(&self) -> Option<u32> {
        self.capture_rate_hz
    }

    /// Lowest committed sample, if any anchored text has landed.
    pub fn first_covered_sample(&self) -> Option<u64> {
        self.first_covered_sample
    }

    /// One past the highest committed sample.
    pub fn last_covered_sample(&self) -> Option<u64> {
        self.last_covered_sample
    }

    /// Energy lookups that observed the clock and found no voiced hop.
    pub fn energy_lookups_without_voiced_hop(&self) -> u64 {
        self.energy_lookups_without_voiced_hop
    }

    /// Record one energy-ladder lookup that returned no voiced hop.
    pub fn note_energy_lookup_without_voiced_hop(&mut self) {
        self.energy_lookups_without_voiced_hop =
            self.energy_lookups_without_voiced_hop.saturating_add(1);
    }

    pub(super) fn note_covered_span(&mut self, occurrence: &OccurrenceIdentity) {
        if !occurrence.is_anchored() {
            return;
        }
        self.first_covered_sample = Some(match self.first_covered_sample {
            Some(current) => current.min(occurrence.sample_start),
            None => occurrence.sample_start,
        });
        self.last_covered_sample = Some(match self.last_covered_sample {
            Some(current) => current.max(occurrence.sample_end),
            None => occurrence.sample_end,
        });
    }

    // -- admission: does this region physically exist? ----------------------

    /// Qualify a physically observed region and mint its mandatory serial.
    ///
    /// This is the *only* place a serial is minted, and the only gate through
    /// which a coordinate becomes evidence. Text is not an input here, in
    /// either direction: a region qualifies because of energy and VAD, or it
    /// does not qualify at all.
    pub fn qualify(
        &mut self,
        evidence: &AcousticEvidence,
        calibration: &EnergyCalibration,
    ) -> AdmissionReceipt {
        let occurrence = evidence.occurrence.clone();
        let refuse = |reason| AdmissionReceipt::Refused {
            occurrence: evidence.occurrence.clone(),
            reason,
        };
        if !occurrence.is_anchored() {
            return refuse(AdmissionRefusal::ZeroWidth);
        }
        if evidence.evidence_calibration_version != calibration.version {
            return refuse(AdmissionRefusal::CalibrationMismatch);
        }
        if evidence.energy_integral < calibration.min_energy_integral {
            return refuse(AdmissionRefusal::BelowCalibratedEnergy);
        }
        let opened = evidence
            .vad_open_sample
            .is_some_and(|open| open <= occurrence.sample_start);
        if !opened {
            return refuse(AdmissionRefusal::VadDidNotOpen);
        }
        let serial = AcousticSerial::mint(evidence);
        self.evidence.insert(occurrence.clone(), serial.clone());
        self.qualified_by_end
            .entry(occurrence.sample_end)
            .or_default()
            .insert(occurrence.clone());
        if !self.is_sealed(&occurrence) {
            self.unsealed_qualified.insert(occurrence.clone());
        }
        crate::pipeline::trail::record(
            &occurrence,
            crate::pipeline::trail::TrailEvent::Qualification {
                evidence: evidence.clone(),
                calibration: calibration.clone(),
                capture_rate_hz: self.capture_rate_hz,
            },
        );
        AdmissionReceipt::Qualified { occurrence, serial }
    }

    /// The serial minted for one occurrence, if it was qualified.
    pub fn serial_of(&self, occurrence: &OccurrenceIdentity) -> Option<&AcousticSerial> {
        self.evidence.get(occurrence)
    }

    /// Whether the occurrence cleared the calibrated existence predicate.
    pub fn is_qualified(&self, occurrence: &OccurrenceIdentity) -> bool {
        self.evidence.contains_key(occurrence)
    }

    /// Qualified occurrences, in capture order.
    pub fn qualified_occurrences(&self) -> impl Iterator<Item = &OccurrenceIdentity> {
        self.evidence.keys()
    }

    /// Only owners that can still do live work. Sealed evidence remains in
    /// `qualified_occurrences` for late-input refusal and replay authentication.
    pub(crate) fn unsealed_qualified_occurrences(
        &self,
    ) -> impl Iterator<Item = &OccurrenceIdentity> {
        self.unsealed_qualified.iter()
    }

    /// Lookup current-window owners by their acoustic end, retaining coarse
    /// owners that began before the window and sealed owners that must refuse
    /// late words. Normal live windows never walk the completed prefix.
    pub(crate) fn qualified_occurrences_overlapping<'a>(
        &'a self,
        range: &'a OccurrenceIdentity,
    ) -> impl Iterator<Item = &'a OccurrenceIdentity> {
        let mut owners = self
            .qualified_by_end
            .range((
                std::ops::Bound::Excluded(range.sample_start),
                std::ops::Bound::Unbounded,
            ))
            .flat_map(|(_, owners)| owners.iter())
            .filter(move |owner| {
                range.is_anchored()
                    && owner.same_capture(range)
                    && owner.sample_start < range.sample_end
            })
            .collect::<Vec<_>>();
        owners.sort_unstable();
        owners.into_iter()
    }

    // -- derivation ---------------------------------------------------------

    /// Record a refinement of physical coverage as provenance.
    ///
    /// The parent coordinate, its serial and its decision history are left
    /// exactly as they were: this appends a derivation, it never edits PCM
    /// history. `Err` carries the exact reason the derivation was refused.
    pub fn record_derivation(
        &mut self,
        derivation: OccurrenceDerivation,
        calibration: &EnergyCalibration,
    ) -> Result<(), &'static str> {
        if let Some(reason) = derivation.rejects(calibration) {
            return Err(reason);
        }
        if derivation
            .parents()
            .iter()
            .any(|parent| !self.evidence.contains_key(*parent))
        {
            return Err("derivation_parent_not_qualified");
        }
        self.derivations.push(derivation);
        Ok(())
    }

    /// Every recorded derivation, in arrival order.
    pub fn derivations(&self) -> &[OccurrenceDerivation] {
        &self.derivations
    }

    /// Derivations that name one occurrence as a parent or a child.
    pub fn derivations_of<'a>(
        &'a self,
        occurrence: &'a OccurrenceIdentity,
    ) -> impl Iterator<Item = &'a OccurrenceDerivation> {
        self.derivations.iter().filter(move |derivation| {
            derivation
                .parents()
                .into_iter()
                .chain(derivation.children())
                .any(|coordinate| coordinate == occurrence)
        })
    }

    // -- composition --------------------------------------------------------

    /// Compose the signed token sequence for one coverage.
    ///
    /// Fails closed: an occurrence without a serial, or holding an empty label,
    /// stops the whole composition rather than quietly delivering a token that
    /// nothing signed or dropping a physical event.
    pub fn compose(
        &self,
        coverage: &OccurrenceIdentity,
    ) -> Result<OccurrenceComposition, EvidenceRefusal> {
        let mut tokens = Vec::new();
        for (occurrence, held) in &self.committed {
            if !occurrence.same_capture(coverage)
                || occurrence.sample_start < coverage.sample_start
                || occurrence.sample_end > coverage.sample_end
            {
                continue;
            }
            let serial = self
                .evidence
                .get(occurrence)
                .ok_or(EvidenceRefusal::OccurrenceNotQualified)?;
            let before = tokens.len();
            let mut observation_ordinals: Vec<(ObservationIdentity, usize)> = Vec::new();
            for slot in &held.slots {
                let position = observation_ordinals
                    .iter()
                    .position(|(observation, _)| observation == &slot.observation)
                    .unwrap_or_else(|| {
                        observation_ordinals.push((slot.observation.clone(), 0));
                        observation_ordinals.len() - 1
                    });
                let ordinal = &mut observation_ordinals[position].1;
                let source_pcm_ranges = self.slot_source_ranges(slot);
                for word in slot.text.split_whitespace() {
                    let mut receipt = WordEvidenceReceipt::cite(
                        word,
                        *ordinal,
                        &slot.observation,
                        vec![serial.clone()],
                        (source_pcm_ranges.len() == 1)
                            .then_some((slot.sample_start, slot.sample_end)),
                    )?;
                    receipt.source_pcm_ranges = source_pcm_ranges.clone();
                    receipt.group_accuracy =
                        source_pcm_ranges.len() > 1 || slot.text.split_whitespace().count() > 1;
                    // A6: the slot's acoustic confidence and lexicon-rewrite
                    // mark travel with every token split out of it.
                    receipt.confidence = slot.confidence;
                    receipt.surface_rewritten = slot.surface_rewritten;
                    tokens.push(receipt);
                    *ordinal += 1;
                }
            }
            if tokens.len() == before
                && !(held.slots.is_empty()
                    && self
                        .word_deletions
                        .iter()
                        .any(|receipt| receipt.operation.observation.occurrence == *occurrence))
            {
                return Err(EvidenceRefusal::EmptyToken);
            }
        }
        Ok(OccurrenceComposition {
            coverage: coverage.clone(),
            tokens,
        })
    }
}
