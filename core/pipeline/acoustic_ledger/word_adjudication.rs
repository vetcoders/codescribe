//! Local lexical evidence owned by AcousticLedger. PCM and SlotTarget identify
//! a component; strings are compared only after that component is resolved.
use super::*;
use serde::{Deserialize, Serialize};

/// A repeated PCM frame cannot corroborate a trial. Replay rejects earlier
/// policies rather than reinterpret their recorded lexical decisions.
/// v4: an uncorroborated disagreement is settled by PCM authority, not by
/// which decode arrived first (`band_authority`).
pub const WORD_POLICY: &str = "word-adjudication/v4";
const MAX_OPEN_COMPONENTS: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceWord {
    pub sample_start: u64,
    pub sample_end: u64,
    /// Absent when the caller supplied only a rewritten surface.
    pub original_text: Option<String>,
    pub surface: String,
    pub confidence: Option<crate::pipeline::word_confidence::WordConfidence>,
    pub surface_rewritten: bool,
    pub decode: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordEvidenceInput {
    pub producer_request: Option<crate::stt::tail_provider::TailRequestIdentity>,
    pub observation: ObservationIdentity,
    pub words: Vec<SourceWord>,
    pub backend: Option<String>,
    /// Unknown is deliberate: positive confidence does not prove finality.
    pub finality: String,
    pub timing: String,
    pub trial: Option<WordTrial>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordHypothesis {
    pub producer_request: Option<crate::stt::tail_provider::TailRequestIdentity>,
    pub observation: ObservationIdentity,
    pub pins: Vec<SourceWord>,
    pub targets: Vec<SlotTarget>,
    pub decode: Option<(u64, u64)>,
    pub original_text: Option<String>,
    pub surface: String,
    pub backend: Option<String>,
    pub finality: String,
    pub timing: String,
    /// The decoder covered the material source scope; not a boundary verdict.
    pub complete: bool,
    #[serde(default)]
    pub acoustic_boundaries_complete: bool,
    pub q: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordChoiceReceipt {
    pub policy: String,
    pub observation: ObservationIdentity,
    pub targets: Vec<SlotTarget>,
    pub source_ranges: Vec<OccurrenceIdentity>,
    pub candidate: WordHypothesis,
    pub support: Vec<WordHypothesis>,
    pub selected_label: String,
    pub reason: String,
    pub accepted: bool,
    /// Admission eligibility alone never discharges a lexical dispute.
    #[serde(default)]
    pub lexical_resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordTrial {
    pub id: u64,
    pub owner: OccurrenceIdentity,
    pub targets: Vec<SlotTarget>,
    pub source_ranges: Vec<OccurrenceIdentity>,
    pub q: u32,
    /// This trial reserves new native work rather than sharing a returned grid
    /// frame. Persist the distinction so replay enforces the same budget.
    #[serde(default)]
    pub fresh_decode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordTrialReceipt {
    pub trial: WordTrial,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordFinality {
    pub targets: Vec<SlotTarget>,
    pub selected: WordHypothesis,
    pub unresolved: bool,
    pub trial: Option<WordTrialReceipt>,
}

#[derive(Debug, Clone)]
struct WordAdjudication {
    owner: OccurrenceIdentity,
    targets: Vec<SlotTarget>,
    incumbent: WordHypothesis,
    apple: Vec<WordHypothesis>,
    whisper: Vec<WordHypothesis>,
    conflict: bool,
    attempted: bool,
    fresh_attempted: bool,
    trial: Option<WordTrial>,
    last_trial: Option<WordTrialReceipt>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct WordAdjudicationState {
    input: Option<WordEvidenceInput>,
    components: Vec<WordAdjudication>,
    pub(super) choices: Vec<WordChoiceReceipt>,
    closed_trials: BTreeMap<u64, [u8; 32]>,
    retain_closed_history: bool,
    active_trials: Vec<WordTrial>,
    next_trial: u64,
    provider: Option<(
        crate::stt::tail_provider::TailRequestIdentity,
        String,
        String,
        String,
    )>,
}

fn acoustic_pair(a: ObservationProducer, b: ObservationProducer) -> bool {
    matches!(a, ObservationProducer::Apple | ObservationProducer::Whisper)
        && matches!(b, ObservationProducer::Apple | ObservationProducer::Whisper)
}

fn trial_digest(trial: &WordTrial) -> [u8; 32] {
    Sha256::digest(serde_json::to_vec(trial).expect("serializable word trial")).into()
}

pub(super) fn label_equal(a: &str, b: &str) -> bool {
    // Equality is evidence about a resolved component, never a target finder.
    normalize_word_token(a) == normalize_word_token(b)
}

/// `context_quality` saturation: the pin sits in the middle third of its
/// decode window. That region is the publication band — with 9 s windows at
/// 3 s strides every instant has exactly one window whose band owns it, and
/// an EOF-shortened window shrinks its band proportionally instead of losing
/// the recording tail.
pub const FULL_CONTEXT_QUALITY: u32 = 1_000_000;

/// Label-changing rights floor: margin of at least one SIXTH of the decode
/// window (half saturation; 1.5 s a side at 9 s windows).
///
/// Requiring full saturation was measurably too strict: the
/// `two_complete_windows_can_correct_an_earlier_wrong_number` product
/// contract corrects through windows at q = 0.94 and 0.75, and the grid
/// guarantees a later window with a deeper margin only one stride away —
/// stalling a two-witness correction that long trades a real fix for
/// ceremony. The incidents this floor must stop sit far below it: the
/// "odwzujący" take-over decoded at q ≈ 0.06 and the "56" edge pins at
/// q ≈ 0.08–0.17. Everything between is decided by the existing agreement
/// machinery, not by geometry alone.
pub const BAND_RIGHTS_FLOOR: u32 = FULL_CONTEXT_QUALITY / 2;

/// Fixed point context ranking. Overflow cannot turn a long window into a vote.
pub fn context_quality(start: u64, end: u64, decode: (u64, u64)) -> u32 {
    let (left, right) = decode;
    if left >= right || start >= end || start < left || end > right {
        return 0;
    }
    let margin = (start - left).min(right - end);
    ((u128::from(margin) * 3 * u128::from(FULL_CONTEXT_QUALITY) / u128::from(right - left))
        .min(u128::from(FULL_CONTEXT_QUALITY))) as u32
}

/// PCM authority a Whisper witness holds over its own source scope: 2 when
/// the scope sits in its window's publication band (the grid gives every
/// instant one such window), 1 with label-changing rights, 0 for edge
/// evidence. A tier, not a score: two witnesses in the same tier are equal
/// and their dispute stays with the agreement machinery.
fn pcm_authority(q: u32) -> u8 {
    if q >= FULL_CONTEXT_QUALITY {
        2
    } else if q >= BAND_RIGHTS_FLOOR {
        1
    } else {
        0
    }
}

impl WordHypothesis {
    fn family(&self) -> ObservationProducer {
        if self
            .backend
            .as_ref()
            .is_some_and(|backend| backend.ends_with(":apple_speech"))
        {
            ObservationProducer::Apple
        } else {
            self.observation.producer
        }
    }
}

impl WordAdjudication {
    fn has_decode(&self, capture: &OccurrenceIdentity, decode: Option<(u64, u64)>) -> bool {
        decode.is_some()
            && self
                .whisper
                .iter()
                .chain(std::iter::once(&self.incumbent))
                .any(|hypothesis| {
                    hypothesis.family() == ObservationProducer::Whisper
                        && hypothesis.observation.occurrence.same_capture(capture)
                        && hypothesis.decode == decode
                })
    }

    fn support(&self) -> Vec<WordHypothesis> {
        let mut support = self.apple.clone();
        support.extend(self.whisper.clone());
        if self.incumbent.complete && !support.contains(&self.incumbent) {
            support.push(self.incumbent.clone());
        }
        support
    }

    fn observe(&mut self, hypothesis: WordHypothesis) -> bool {
        match hypothesis.family() {
            ObservationProducer::Apple => {
                if self.apple.last().is_some_and(|prior| {
                    prior.observation.generation > hypothesis.observation.generation
                        || (prior.original_text == hypothesis.original_text
                            && prior.pins == hypothesis.pins)
                }) {
                    return false;
                }
                self.apple.push(hypothesis);
                if self.apple.len() > 2 {
                    self.apple.remove(0);
                }
                true
            }
            ObservationProducer::Whisper => {
                // The same audio with a new request id is still one witness.
                if hypothesis.decode.is_none() {
                    return false;
                }
                if let Some(prior) = self.whisper.iter_mut().find(|prior| {
                    prior.family() == ObservationProducer::Whisper
                        && prior.decode == hypothesis.decode
                        && prior
                            .observation
                            .occurrence
                            .same_capture(&hypothesis.observation.occurrence)
                }) {
                    // Keep the latest alternative from this frame for a
                    // bounded trial, without counting it as a second vote.
                    if hypothesis.observation.generation > prior.observation.generation {
                        *prior = hypothesis;
                    }
                    return false;
                }
                self.whisper.push(hypothesis);
                self.whisper.sort_by_key(|h| h.decode.map(|(s, e)| (e, s)));
                if self.whisper.len() > 3 {
                    self.whisper.remove(0);
                }
                true
            }
            _ => false,
        }
    }
}

impl AcousticLedger {
    pub(super) fn record_word_decode_bounds(
        &mut self,
        observation: &ObservationIdentity,
        words: &[WordPin],
    ) {
        let owner = &observation.occurrence;
        let complete_words = words
            .iter()
            .filter(|pin| {
                pin.decode_sample_start
                    .is_some_and(|start| start < pin.sample_start)
                    && pin.decode_sample_end.is_some_and(|end| {
                        if observation.producer == ObservationProducer::Whisper {
                            pin.sample_end <= end
                                && !self.decode_word_fence_incomplete(
                                    owner,
                                    pin.decode_sample_start.unwrap(),
                                    end,
                                    pin.sample_start,
                                    pin.sample_end,
                                )
                        } else {
                            pin.sample_end < end
                        }
                    })
                    && pin.sample_start < pin.sample_end
                    && pin.text.split_whitespace().count() == 1
            })
            // Completeness belongs to the original decoded word. Index its
            // bounded projection so a cut at an owner edge is not a decode cut.
            .map(|pin| {
                (
                    pin.sample_start.max(owner.sample_start),
                    pin.sample_end.min(owner.sample_end),
                )
            })
            .collect::<Vec<_>>();
        if words
            .iter()
            .any(|pin| pin.decode_sample_start.is_some() || pin.decode_sample_end.is_some())
        {
            self.complete_decoded_words
                .insert(observation.clone(), complete_words);
        }
        if let Some((start, end)) = words
            .first()
            .and_then(|pin| Some((pin.decode_sample_start?, pin.decode_sample_end?)))
            && start < end
            && words.iter().all(|pin| {
                pin.decode_sample_start == Some(start)
                    && pin.decode_sample_end == Some(end)
                    && start <= pin.sample_start
                    && pin.sample_end <= end
            })
        {
            self.decoded_word_windows
                .insert(observation.clone(), (start, end));
        }
    }

    /// Original producer pins enter before a surface rewrite or rank refusal.
    /// This staging fact has no document or seal authority.
    pub(crate) fn stage_word_evidence(
        &mut self,
        observation: &ObservationIdentity,
        original: &[WordPin],
        backend: Option<&str>,
        finality: &str,
    ) {
        let provider = self
            .word_adjudication
            .provider
            .as_ref()
            .filter(|(request, _, _, _)| {
                observation.producer == ObservationProducer::Whisper
                    && request.range.session == observation.occurrence.session
                    && request.range.capture_epoch == observation.occurrence.capture_epoch
                    && original.iter().all(|pin| {
                        pin.decode_sample_start == Some(request.range.sample_start)
                            && pin.decode_sample_end == Some(request.range.sample_end)
                    })
            });
        self.word_adjudication.input = Some(WordEvidenceInput {
            producer_request: provider.map(|p| p.0.clone()),
            observation: observation.clone(),
            words: original
                .iter()
                .map(|pin| SourceWord {
                    sample_start: pin.sample_start,
                    sample_end: pin.sample_end,
                    original_text: (!pin.surface_rewritten).then(|| pin.text.clone()),
                    surface: pin.text.clone(),
                    confidence: pin.confidence,
                    surface_rewritten: pin.surface_rewritten,
                    decode: pin.decode_sample_start.zip(pin.decode_sample_end),
                })
                .collect(),
            backend: backend
                .map(str::to_owned)
                .or_else(|| provider.map(|p| p.1.clone())),
            finality: provider.map_or_else(|| finality.to_owned(), |p| p.2.clone()),
            timing: provider.map_or_else(|| "capture_word_pins".into(), |p| p.3.clone()),
            trial: None,
        });
    }

    pub(crate) fn bind_word_provider(
        &mut self,
        payload: &crate::stt::tail_provider::TailProviderPayload,
    ) {
        self.word_adjudication.provider = Some((
            payload.identity.clone(),
            format!(
                "{}:{}",
                payload.provider_id.as_str(),
                payload.evidence.source.as_str()
            ),
            format!("{:?}", payload.evidence.stability).to_lowercase(),
            payload.evidence.timing_quality.as_str().into(),
        ));
    }

    pub(super) fn prepare_word_evidence(
        &mut self,
        observation: &ObservationIdentity,
        pins: &[WordPin],
    ) {
        if self
            .word_adjudication
            .input
            .as_ref()
            .is_none_or(|input| input.observation != *observation)
        {
            self.stage_word_evidence(observation, pins, None, "unknown");
        }
        if let Some(input) = self.word_adjudication.input.as_mut() {
            for source in &mut input.words {
                if let Some(pin) = pins.iter().find(|pin| {
                    pin.sample_start == source.sample_start && pin.sample_end == source.sample_end
                }) {
                    source.surface = pin.text.clone();
                    source.surface_rewritten = pin.surface_rewritten;
                }
            }
        }
    }

    pub(crate) fn word_evidence_input(
        &self,
        observation: &ObservationIdentity,
    ) -> Option<&WordEvidenceInput> {
        self.word_adjudication
            .input
            .as_ref()
            .filter(|input| input.observation == *observation)
    }

    pub(crate) fn restore_word_evidence(&mut self, input: WordEvidenceInput) {
        self.word_adjudication.input = Some(input);
    }

    /// Material playback sources, excluding a split child's retired siblings.
    fn word_source_ranges(&self, sources: &[WordSlot]) -> Vec<OccurrenceIdentity> {
        let mut ranges = Vec::new();
        for source in sources {
            for range in self.slot_source_ranges(source) {
                if !ranges.contains(&range) {
                    ranges.push(range);
                }
            }
        }
        ranges.sort();
        ranges
    }

    fn provisional_apple_revision(
        &self,
        observation: &ObservationIdentity,
        sources: &[WordSlot],
    ) -> bool {
        observation.producer == ObservationProducer::Apple
            && !sources.is_empty()
            && sources
                .iter()
                .all(|source| source.producer == ObservationProducer::Apple)
            && self.word_evidence_input(observation).is_some_and(|input| {
                sources.iter().all(|source| {
                    input.words.iter().any(|pin| {
                        pin.sample_start == source.sample_start
                            && pin.sample_end == source.sample_end
                    })
                })
            })
            && !self.word_adjudication.components.iter().any(|component| {
                component.targets.iter().any(|target| {
                    sources
                        .iter()
                        .any(|source| SlotTarget::from(source) == *target)
                }) && !component.whisper.is_empty()
            })
    }

    pub(super) fn asr_source_scope_complete(
        &self,
        observation: &ObservationIdentity,
        sources: &[WordSlot],
    ) -> bool {
        if observation.producer == ObservationProducer::ManualHuman
            || sources.iter().all(|source| self.coarse_word_source(source))
            || self.provisional_apple_revision(observation, sources)
        {
            return true;
        }
        let decode = self
            .decoded_word_windows
            .get(observation)
            .copied()
            .or_else(|| {
                let input = self.word_evidence_input(observation)?;
                let decode = input.words.first()?.decode?;
                input
                    .words
                    .iter()
                    .all(|pin| pin.decode == Some(decode))
                    .then_some(decode)
            });
        let Some((start, end)) = decode else {
            return false;
        };
        start < end
            && self.word_source_ranges(sources).iter().all(|range| {
                range.same_capture(&observation.occurrence)
                    && start <= range.sample_start
                    && range.sample_end <= end
            })
    }

    fn hypothesis(
        &self,
        observation: &ObservationIdentity,
        sources: &[WordSlot],
        outputs: &[WordSlot],
    ) -> WordHypothesis {
        let input = self.word_evidence_input(observation);
        let pins = outputs
            .iter()
            .map(|output| {
                input
                    .and_then(|input| {
                        input.words.iter().find(|pin| {
                            pin.sample_start.max(observation.occurrence.sample_start)
                                == output.sample_start
                                && pin.sample_end.min(observation.occurrence.sample_end)
                                    == output.sample_end
                        })
                    })
                    .cloned()
                    .unwrap_or_else(|| SourceWord {
                        sample_start: output.sample_start,
                        sample_end: output.sample_end,
                        original_text: (!output.surface_rewritten).then(|| output.text.clone()),
                        surface: output.text.clone(),
                        confidence: output.confidence,
                        surface_rewritten: output.surface_rewritten,
                        decode: self.decoded_word_windows.get(&output.observation).copied(),
                    })
            })
            .collect::<Vec<_>>();
        let decode = pins
            .first()
            .and_then(|pin| pin.decode)
            .filter(|decode| pins.iter().all(|pin| pin.decode == Some(*decode)));
        let ranges = self.word_source_ranges(sources);
        let start = ranges
            .iter()
            .map(|r| r.sample_start)
            .chain(pins.iter().map(|p| p.sample_start))
            .min()
            .unwrap_or(0);
        let end = ranges
            .iter()
            .map(|r| r.sample_end)
            .chain(pins.iter().map(|p| p.sample_end))
            .max()
            .unwrap_or(0);
        let measured = !pins.is_empty() && pins.iter().all(|pin| pin.sample_start < pin.sample_end);
        let timing = input.map_or("capture_word_pins", |input| input.timing.as_str());
        // Scope completeness and acoustic boundary quality are separate.
        // The existing geometry guards protect splits/contractions; q ranks
        // context without converting a capture-edge word into missing speech.
        let complete = measured
            && matches!(timing, "capture_word_pins" | "exact_sample_range")
            && decode.is_some_and(|(s, e)| s < e && s <= start && end <= e);
        let acoustic_boundaries_complete = complete
            && decode.is_some_and(|(s, e)| {
                pins.iter().all(|pin| {
                    (s < pin.sample_start || pin.sample_start == 0)
                        && !self.decode_word_fence_incomplete(
                            &observation.occurrence,
                            s,
                            e,
                            pin.sample_start,
                            pin.sample_end,
                        )
                })
            });
        WordHypothesis {
            producer_request: input.and_then(|input| input.producer_request.clone()),
            observation: observation.clone(),
            original_text: pins
                .iter()
                .map(|pin| pin.original_text.clone())
                .collect::<Option<Vec<_>>>()
                .map(|words| words.join(" ")),
            surface: compose_label(outputs),
            pins,
            targets: sources.iter().map(SlotTarget::from).collect(),
            decode,
            backend: input.and_then(|input| input.backend.clone()),
            finality: input.map_or_else(|| "unknown".into(), |input| input.finality.clone()),
            timing: input.map_or_else(|| "capture_word_pins".into(), |input| input.timing.clone()),
            complete,
            acoustic_boundaries_complete,
            q: decode.map_or(0, |decode| context_quality(start, end, decode)),
        }
    }

    pub(super) fn record_word_choice(
        &mut self,
        observation: &ObservationIdentity,
        sources: &[WordSlot],
        outputs: &[WordSlot],
        reason: &str,
        accepted: bool,
    ) {
        let targets = sources.iter().map(SlotTarget::from).collect::<Vec<_>>();
        let support = self
            .word_adjudication
            .components
            .iter()
            .find(|c| c.targets == targets)
            .map_or_else(Vec::new, WordAdjudication::support);
        self.word_adjudication.choices.push(WordChoiceReceipt {
            policy: WORD_POLICY.into(),
            observation: observation.clone(),
            targets,
            source_ranges: self.word_source_ranges(sources),
            candidate: self.hypothesis(observation, sources, outputs),
            support,
            selected_label: if accepted {
                compose_label(outputs)
            } else {
                compose_label(sources)
            },
            reason: reason.into(),
            accepted,
            lexical_resolved: false,
        });
    }

    /// Revision proofs for open owners. Closed-owner proofs live in the disk
    /// trail, while their selected hypothesis lives in the immutable seal.
    pub fn word_choices(&self) -> &[WordChoiceReceipt] {
        &self.word_adjudication.choices
    }

    fn component_for_sources(
        &self,
        owner: &OccurrenceIdentity,
        sources: &[WordSlot],
    ) -> WordAdjudication {
        let targets = sources.iter().map(SlotTarget::from).collect::<Vec<_>>();
        let members = self
            .word_adjudication
            .components
            .iter()
            .filter(|c| {
                c.owner == *owner && c.targets.iter().all(|target| targets.contains(target))
            })
            .collect::<Vec<_>>();
        let mut incumbent = self.hypothesis(&sources[0].observation, sources, sources);
        if sources
            .iter()
            .any(|source| source.observation != sources[0].observation)
        {
            incumbent.original_text = None;
            incumbent.complete = false;
            incumbent.acoustic_boundaries_complete = false;
        }
        let mut component = WordAdjudication {
            owner: owner.clone(),
            targets: targets.clone(),
            incumbent,
            apple: Vec::new(),
            whisper: Vec::new(),
            conflict: members.iter().any(|c| c.conflict),
            attempted: members.iter().any(|c| c.attempted),
            fresh_attempted: members.iter().any(|c| c.fresh_attempted),
            trial: None,
            last_trial: members.iter().find_map(|c| c.last_trial.clone()),
        };
        // A partial member is not a complete group hypothesis. Assemble only
        // witnesses whose same producer pass accounts for every named target.
        let mut groups: Vec<(WordHypothesis, Vec<SlotTarget>)> = Vec::new();
        for member in members {
            for hypothesis in member.support() {
                let group = groups.iter_mut().find(|(h, _)| {
                    h.family() == hypothesis.family()
                        && h.observation
                            .occurrence
                            .same_capture(&hypothesis.observation.occurrence)
                        && if h.family() == ObservationProducer::Whisper {
                            h.decode.is_some() && h.decode == hypothesis.decode
                        } else {
                            h.observation.request == hypothesis.observation.request
                                && h.observation.generation == hypothesis.observation.generation
                        }
                });
                if let Some((group, covered)) = group {
                    group.complete &= hypothesis.complete;
                    group.acoustic_boundaries_complete &= hypothesis.acoustic_boundaries_complete;
                    for pin in hypothesis.pins {
                        if !group.pins.contains(&pin) {
                            group.pins.push(pin);
                        }
                    }
                    for target in &member.targets {
                        if !covered.contains(target) {
                            covered.push(target.clone());
                        }
                    }
                } else {
                    groups.push((hypothesis, member.targets.clone()));
                }
            }
        }
        let ranges = self.word_source_ranges(sources);
        for (mut hypothesis, covered) in groups {
            if !targets.iter().all(|target| covered.contains(target)) {
                continue;
            }
            hypothesis
                .pins
                .sort_by_key(|pin| (pin.sample_start, pin.sample_end));
            hypothesis.original_text = hypothesis
                .pins
                .iter()
                .map(|pin| pin.original_text.clone())
                .collect::<Option<Vec<_>>>()
                .map(|words| words.join(" "));
            hypothesis.surface = hypothesis
                .pins
                .iter()
                .map(|pin| pin.surface.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            hypothesis.targets = targets.clone();
            let start = ranges
                .iter()
                .map(|range| range.sample_start)
                .min()
                .unwrap_or(0);
            let end = ranges
                .iter()
                .map(|range| range.sample_end)
                .max()
                .unwrap_or(0);
            hypothesis.complete = hypothesis.complete
                && hypothesis
                    .decode
                    .is_some_and(|(s, e)| s <= start && end <= e);
            hypothesis.acoustic_boundaries_complete &= hypothesis.complete;
            hypothesis.q = hypothesis
                .decode
                .map_or(0, |decode| context_quality(start, end, decode));
            component.observe(hypothesis);
        }
        if component.apple.is_empty()
            && component.whisper.is_empty()
            && component.incumbent.original_text.is_some()
        {
            component.observe(component.incumbent.clone());
        }
        component
    }

    /// Returns None for non-Apple/Whisper authority domains. A lexical refusal
    /// never creates missing-speech debt; geometry is checked separately.
    pub(super) fn adjudicate_word_sources(
        &mut self,
        observation: &ObservationIdentity,
        sources: &[WordSlot],
        outputs: &[WordSlot],
    ) -> Option<bool> {
        if let Some(trial) = self
            .word_evidence_input(observation)
            .and_then(|input| input.trial.as_ref())
            && trial.targets != sources.iter().map(SlotTarget::from).collect::<Vec<_>>()
        {
            self.record_word_choice(
                observation,
                sources,
                outputs,
                "trial_target_mismatch",
                false,
            );
            return Some(false);
        }
        if sources.is_empty()
            || sources.iter().all(|source| self.coarse_word_source(source))
            || !sources
                .iter()
                .all(|source| acoustic_pair(source.producer, observation.producer))
        {
            return None;
        }
        let targets = sources.iter().map(SlotTarget::from).collect::<Vec<_>>();
        let provisional_apple = self.provisional_apple_revision(observation, sources);
        let candidate = self.hypothesis(observation, sources, outputs);
        let complete = self.asr_source_scope_complete(observation, sources) && candidate.complete;
        let existing = self
            .word_adjudication
            .components
            .iter()
            .position(|c| c.targets == targets);
        let mut component = if let Some(index) = existing {
            self.word_adjudication.components.remove(index)
        } else {
            if self.word_adjudication.components.len() >= MAX_OPEN_COMPONENTS {
                self.record_word_choice(observation, sources, outputs, "evidence_capacity", false);
                return Some(false);
            }
            let component = self.component_for_sources(&observation.occurrence, sources);
            self.word_adjudication.components.retain(|c| {
                c.owner != observation.occurrence
                    || !c.targets.iter().all(|target| targets.contains(target))
            });
            component
        };
        // A rejected older Apple callback cannot revise the provisional
        // label or reopen a conflict against fresher retained evidence.
        if candidate.family() == ObservationProducer::Apple
            && component.apple.last().is_some_and(|prior| {
                prior.observation.generation >= candidate.observation.generation
            })
        {
            self.word_adjudication.components.push(component);
            self.record_word_choice(
                observation,
                sources,
                outputs,
                "stale_apple_generation",
                false,
            );
            return Some(false);
        }
        let trial = self
            .word_evidence_input(observation)
            .and_then(|input| input.trial.as_ref());
        let trial_matches = trial.is_some_and(|trial| {
            component.trial.as_ref() == Some(trial) && trial.targets == targets
        });
        let prior_support = component.support();
        // The incumbent can outlive the three retained Whisper alternatives.
        // Replaying its frame does not become fresh when that history rolls.
        let repeated_decode = candidate.family() == ObservationProducer::Whisper
            && component.has_decode(&candidate.observation.occurrence, candidate.decode);
        let fresh = component.observe(candidate.clone()) && !repeated_decode;
        let provisional_apple = provisional_apple && fresh;
        let repeated_label = label_equal(&candidate.surface, &compose_label(sources));
        let raw = candidate.original_text.as_deref();
        let apple = component
            .apple
            .last()
            .and_then(|h| h.original_text.as_deref());
        let whisper_support = component
            .support()
            .into_iter()
            .filter(|h| {
                h.family() == ObservationProducer::Whisper
                    && h.complete
                    && h.acoustic_boundaries_complete
                    && h.original_text.is_some()
                    && h.original_text
                        .as_deref()
                        .zip(raw)
                        .is_some_and(|(a, b)| label_equal(a, b))
            })
            .map(|h| h.decode)
            .collect::<BTreeSet<_>>()
            .len();
        let agreement = raw.is_some()
            && ((apple.zip(raw).is_some_and(|(a, b)| label_equal(a, b)) && whisper_support > 0)
                || (whisper_support >= 2
                    && apple.is_none_or(|a| raw.is_some_and(|b| label_equal(a, b)))));
        let confirmed_trial = trial_matches
            && fresh
            && complete
            && candidate.acoustic_boundaries_complete
            && raw.is_some()
            && prior_support.iter().any(|h| {
                // A new request for the same PCM cannot corroborate itself,
                // including an incumbent no longer in the rolling history.
                ((h.family() == ObservationProducer::Whisper
                    && h.complete
                    && h.acoustic_boundaries_complete
                    && h.decode.is_some()
                    && h.decode != candidate.decode)
                    || (h.family() == ObservationProducer::Apple
                        && component.apple.last() == Some(h)))
                    && h.original_text
                        .as_deref()
                        .zip(raw)
                        .is_some_and(|(a, b)| label_equal(a, b))
            });
        let raw_disagrees = raw
            .zip(component.incumbent.original_text.as_deref())
            .is_some_and(|(a, b)| !label_equal(a, b));
        let geometry_preserves_complete_source = candidate.acoustic_boundaries_complete
            || !sources.iter().any(|source| self.complete_word_slot(source));
        // Publication band: a Whisper recognition from the edge of its decode
        // window may inform a dispute, never overwrite a disagreeing
        // incumbent through plain stream agreement. The decoder completes
        // clipped phonemes into plausible non-words there ("56" take-over;
        // "obiecujący" → "odwzujący"), so label-changing agreement requires
        // the candidate's pin in the middle third of its window — or a banded
        // prior hypothesis that already proposed the same label. Two paths
        // stay ungated on purpose: first placement (at recording start there
        // is no earlier window to own those seconds), and a confirmed trial —
        // the trial is already the controlled escalation (two prior
        // witnesses, fresh PCM, a receipt) and is at times the only repair
        // when the banded window itself misread.
        let band_rights = candidate.family() != ObservationProducer::Whisper
            || !raw_disagrees
            || component.incumbent.original_text.is_none()
            || candidate.q >= BAND_RIGHTS_FLOOR
            || prior_support.iter().any(|h| {
                h.q >= BAND_RIGHTS_FLOOR
                    && h.original_text
                        .as_deref()
                        .zip(raw)
                        .is_some_and(|(a, b)| label_equal(a, b))
            });
        let lexical_resolved = provisional_apple
            || confirmed_trial
            || (trial.is_none()
                && complete
                && candidate.acoustic_boundaries_complete
                && agreement
                && fresh
                && band_rights);
        // First placement is ungated, so an edge or off-band decode can hold a
        // word only because it arrived first; the same decode arriving second
        // would be refused above. Without corroboration on either side, the
        // witness whose PCM context places this exact scope higher keeps the
        // label. The dispute stays open: no agreement was reached, so the
        // conflict, its trial and the word finality still say so.
        let band_authority = trial.is_none()
            && !lexical_resolved
            && raw_disagrees
            && fresh
            && complete
            && candidate.acoustic_boundaries_complete
            && geometry_preserves_complete_source
            && candidate.family() == ObservationProducer::Whisper
            && component.incumbent.family() == ObservationProducer::Whisper
            && {
                let incumbent_label = component.incumbent.original_text.as_deref();
                let witnesses = prior_support
                    .iter()
                    .filter(|h| {
                        h.family() == ObservationProducer::Whisper
                            && h.complete
                            && h.acoustic_boundaries_complete
                            && h.decode != candidate.decode
                            && h.original_text
                                .as_deref()
                                .zip(incumbent_label)
                                .is_some_and(|(a, b)| label_equal(a, b))
                    })
                    .collect::<Vec<_>>();
                let incumbent_authority = witnesses
                    .iter()
                    .map(|h| pcm_authority(h.q))
                    .max()
                    .unwrap_or(0);
                // Two rights-holding frames, or the current Apple label, are
                // corroboration the candidate does not have.
                let corroborated = witnesses
                    .iter()
                    .filter(|h| pcm_authority(h.q) > 0)
                    .filter_map(|h| h.decode)
                    .collect::<BTreeSet<_>>()
                    .len()
                    >= 2
                    || apple
                        .zip(incumbent_label)
                        .is_some_and(|(a, b)| label_equal(a, b));
                !corroborated && pcm_authority(candidate.q) > incumbent_authority
            };
        let (accepted, reason) = if provisional_apple {
            // A still-provisional Apple word may evolve on the same exact
            // pins. Once Whisper supplies evidence, normal adjudication owns it.
            (true, "apple_provisional_revision")
        } else if !complete {
            (false, "incomplete_source_scope")
        } else if !geometry_preserves_complete_source {
            (false, "incomplete_acoustic_boundary")
        } else if trial.is_some() {
            if confirmed_trial {
                (true, "trial_confirmed")
            } else {
                (false, "trial_unresolved")
            }
        } else if !band_rights {
            (false, "outside_publication_band")
        } else if repeated_label {
            // Corroborated timing can extend the same physical word. Keeping
            // its first, shorter pin would turn a later suffix into a new word.
            let same_geometry = sources.len() == outputs.len()
                && sources.iter().zip(outputs).all(|(old, new)| {
                    old.sample_start == new.sample_start && old.sample_end == new.sample_end
                });
            (
                (same_geometry && !component.conflict && raw.is_some() && !raw_disagrees)
                    || lexical_resolved,
                "same_label_evidence",
            )
        } else if lexical_resolved {
            (true, "source_agreement")
        } else if band_authority {
            (true, "band_authority")
        } else {
            (false, "lexical_disagreement")
        };
        let apple_disagrees = candidate.family() == ObservationProducer::Apple
            && raw.is_some()
            && !candidate
                .original_text
                .as_deref()
                .zip(component.incumbent.original_text.as_deref())
                .is_some_and(|(a, b)| label_equal(a, b))
            && component
                .support()
                .iter()
                .any(|h| h.family() == ObservationProducer::Whisper && h.complete);
        let held_whisper_disagrees = repeated_label
            && apple.is_some()
            && !apple.zip(raw).is_some_and(|(a, b)| label_equal(a, b))
            && candidate.family() == ObservationProducer::Whisper
            && complete;
        if (complete && (!repeated_label || raw_disagrees) && !lexical_resolved)
            || apple_disagrees
            || held_whisper_disagrees
        {
            component.conflict = true;
        }
        if lexical_resolved {
            component.conflict = false;
            component.attempted = false;
            // Keep the active trial until its completion receipt is recorded.
            if trial.is_none() {
                component.trial = None;
            }
        }
        let support = component.support();
        self.word_adjudication.components.push(component);
        self.word_adjudication.choices.push(WordChoiceReceipt {
            policy: WORD_POLICY.into(),
            observation: observation.clone(),
            targets,
            source_ranges: self.word_source_ranges(sources),
            candidate,
            support,
            selected_label: if accepted {
                compose_label(outputs)
            } else {
                compose_label(sources)
            },
            reason: reason.into(),
            accepted,
            lexical_resolved,
        });
        Some(accepted)
    }

    /// Called only after admission committed. Failed batches cannot advance targets.
    pub(super) fn remember_word_outputs(&mut self, observation: &ObservationIdentity) {
        let outputs = self
            .slots_of(&observation.occurrence)
            .unwrap_or(&[])
            .to_vec();
        let operations = self
            .slot_operations
            .iter()
            .filter(|op| op.observation == *observation)
            .cloned()
            .collect::<Vec<_>>();
        for op in operations {
            if op.outputs.is_empty() {
                continue;
            }
            let targets = op.sources.iter().map(SlotTarget::from).collect::<Vec<_>>();
            if let Some(index) = self
                .word_adjudication
                .components
                .iter()
                .position(|c| c.targets == targets)
            {
                let hypothesis = self.hypothesis(observation, &op.outputs, &op.outputs);
                let component = &mut self.word_adjudication.components[index];
                component.targets = op.outputs.iter().map(SlotTarget::from).collect();
                component.incumbent = hypothesis;
            }
        }
        self.word_adjudication.components.retain(|component| {
            component.owner != observation.occurrence
                || component.targets.iter().all(|target| {
                    outputs
                        .iter()
                        .any(|output| SlotTarget::from(output) == *target)
                })
        });
        for output in outputs
            .iter()
            .filter(|output| output.observation == *observation)
        {
            let target = SlotTarget::from(output);
            if !matches!(
                output.producer,
                ObservationProducer::Apple | ObservationProducer::Whisper
            ) || self
                .word_adjudication
                .components
                .iter()
                .any(|c| c.targets.contains(&target))
                || self.word_adjudication.components.len() >= MAX_OPEN_COMPONENTS
            {
                continue;
            }
            let hypothesis = self.hypothesis(
                observation,
                std::slice::from_ref(output),
                std::slice::from_ref(output),
            );
            let mut component = WordAdjudication {
                owner: observation.occurrence.clone(),
                targets: vec![target],
                incumbent: hypothesis.clone(),
                apple: Vec::new(),
                whisper: Vec::new(),
                conflict: false,
                attempted: false,
                fresh_attempted: false,
                trial: None,
                last_trial: None,
            };
            component.observe(hypothesis);
            self.word_adjudication.components.push(component);
        }
    }

    #[cfg(test)]
    pub(crate) fn next_word_trial(&mut self, stopping: bool) -> Option<WordTrial> {
        self.next_word_trial_in(stopping, None, None)
    }

    /// An actual trial decode can adjudicate other disputed words only when
    /// its PCM covers their complete source lineage. It is one new witness
    /// for each physical word, never repeated votes for the same component.
    pub(crate) fn next_word_trial_in(
        &mut self,
        stopping: bool,
        coverage: Option<&OccurrenceIdentity>,
        // Required padding and retained capture bounds. With coverage, the
        // returned frame must supply this context. Without coverage, exclude
        // already observed planned frames before spending a live trial.
        context: Option<(u64, std::ops::Range<u64>)>,
    ) -> Option<WordTrial> {
        self.select_word_trial(stopping, coverage, context, u64::MAX, false)
    }

    pub(crate) fn next_word_trial_before_horizon(
        &mut self,
        stopping: bool,
        coverage: Option<&OccurrenceIdentity>,
        context: Option<(u64, std::ops::Range<u64>)>,
        horizon: u64,
    ) -> Option<WordTrial> {
        self.select_word_trial(stopping, coverage, context, horizon, true)
    }

    fn select_word_trial(
        &mut self,
        stopping: bool,
        coverage: Option<&OccurrenceIdentity>,
        context: Option<(u64, std::ops::Range<u64>)>,
        horizon: u64,
        fresh_decode: bool,
    ) -> Option<WordTrial> {
        let index = self
            .word_adjudication
            .components
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                if c.owner.sample_end > horizon
                    || !c.conflict
                    || c.trial.is_some()
                    || if fresh_decode {
                        c.fresh_attempted
                    } else {
                        c.attempted
                    }
                    || self.is_sealed(&c.owner)
                    || (!stopping
                        && c.support()
                            .iter()
                            .filter(|h| h.family() == ObservationProducer::Whisper && h.complete)
                            .filter_map(|h| h.decode)
                            .collect::<BTreeSet<_>>()
                            .len()
                            < 2)
                {
                    return false;
                }
                if coverage.is_none() && context.is_none() {
                    return true;
                }
                let sources = self
                    .slots_of(&c.owner)
                    .unwrap_or(&[])
                    .iter()
                    .filter(|slot| c.targets.contains(&SlotTarget::from(*slot)))
                    .cloned()
                    .collect::<Vec<_>>();
                let ranges = self.word_source_ranges(&sources);
                if sources.is_empty() || sources.len() != c.targets.len() || ranges.is_empty() {
                    return false;
                }
                if let Some(coverage) = coverage
                    && (c.has_decode(coverage, Some((coverage.sample_start, coverage.sample_end)))
                        || !ranges.iter().all(|range| {
                            range.same_capture(coverage)
                                && range.sample_start > coverage.sample_start
                                && range.sample_end <= coverage.sample_end
                                && !self.decode_word_fence_incomplete(
                                    range,
                                    coverage.sample_start,
                                    coverage.sample_end,
                                    range.sample_start,
                                    range.sample_end,
                                )
                        }))
                {
                    return false;
                }
                context.as_ref().is_none_or(|(padding, retained)| {
                    let start = ranges
                        .iter()
                        .map(|range| range.sample_start)
                        .min()
                        .expect("nonempty source ranges");
                    let end = ranges
                        .iter()
                        .map(|range| range.sample_end)
                        .max()
                        .expect("nonempty source ranges");
                    let decode = (
                        start.saturating_sub(*padding).max(retained.start),
                        end.saturating_add(*padding).min(retained.end),
                    );
                    start >= retained.start
                        && end <= retained.end
                        && coverage.map_or_else(
                            || !c.has_decode(&c.owner, Some(decode)),
                            |range| range.sample_start <= decode.0 && range.sample_end >= decode.1,
                        )
                })
            })
            .max_by_key(|(_, c)| c.whisper.iter().map(|h| h.q).max().unwrap_or(0))
            .map(|(index, _)| index)?;
        self.open_word_trial(index, fresh_decode)
    }

    /// Reissuing a retained decode frame cannot add evidence to this trial.
    /// The scheduler asks before leasing PCM or submitting another native job;
    /// admission independently enforces the same rule on returned evidence.
    #[cfg(test)]
    pub(crate) fn word_trial_has_decode(
        &self,
        trial: &WordTrial,
        decode: &OccurrenceIdentity,
    ) -> bool {
        self.word_adjudication
            .components
            .iter()
            .filter(|component| component.trial.as_ref() == Some(trial))
            .any(|component| {
                component.has_decode(decode, Some((decode.sample_start, decode.sample_end)))
            })
    }

    /// The live scheduler proved that every window able to own these words
    /// returned. Keep unresolved incumbents and record expiry through the
    /// existing trial receipts; expiry supplies no new acoustic witness.
    pub(crate) fn close_word_adjudication_horizon(&mut self, owner: &OccurrenceIdentity) -> usize {
        let mut closed = 0;
        while let Some(index) = self
            .word_adjudication
            .components
            .iter()
            .position(|component| {
                &component.owner == owner && component.conflict && !component.attempted
            })
        {
            let Some(trial) = self.open_word_trial(index, false) else {
                break;
            };
            self.close_word_trial(&trial, "admission_horizon_closed");
            closed += 1;
        }
        closed
    }

    fn open_word_trial(&mut self, index: usize, fresh_decode: bool) -> Option<WordTrial> {
        let component = &self.word_adjudication.components[index];
        let sources = self
            .slots_of(&component.owner)
            .unwrap_or(&[])
            .iter()
            .filter(|slot| component.targets.contains(&SlotTarget::from(*slot)))
            .cloned()
            .collect::<Vec<_>>();
        if sources.len() != component.targets.len() {
            return None;
        }
        self.word_adjudication.next_trial = self.word_adjudication.next_trial.saturating_add(1);
        let trial = WordTrial {
            id: self.word_adjudication.next_trial,
            owner: component.owner.clone(),
            targets: component.targets.clone(),
            source_ranges: self.word_source_ranges(&sources),
            q: component.whisper.iter().map(|h| h.q).max().unwrap_or(0),
            fresh_decode,
        };
        let component = &mut self.word_adjudication.components[index];
        component.attempted = true;
        component.fresh_attempted |= fresh_decode;
        component.trial = Some(trial.clone());
        self.word_adjudication.active_trials.push(trial.clone());
        super::super::trail::record(
            &trial.owner,
            super::super::trail::TrailEvent::WordTrialOpened {
                trial: trial.clone(),
            },
        );
        Some(trial)
    }

    pub(crate) fn close_word_trial(&mut self, trial: &WordTrial, reason: &str) {
        if self.word_adjudication.closed_trials.contains_key(&trial.id) {
            return;
        }
        self.word_adjudication
            .active_trials
            .retain(|active| active != trial);
        for component in &mut self.word_adjudication.components {
            if component.trial.as_ref() == Some(trial) {
                component.trial = None;
                component.attempted = component.conflict;
                component.last_trial = Some(WordTrialReceipt {
                    trial: trial.clone(),
                    reason: reason.into(),
                });
            }
        }
        self.word_adjudication
            .closed_trials
            .insert(trial.id, trial_digest(trial));
        super::super::trail::record(
            &trial.owner,
            super::super::trail::TrailEvent::WordTrialClosed {
                receipt: WordTrialReceipt {
                    trial: trial.clone(),
                    reason: reason.into(),
                },
            },
        );
    }

    pub(crate) fn restore_word_trial(&mut self, trial: &WordTrial) -> Result<(), &'static str> {
        if self.word_adjudication.active_trials.contains(trial) {
            return Ok(());
        }
        if let Some(digest) = self.word_adjudication.closed_trials.get(&trial.id) {
            return if *digest == trial_digest(trial) {
                Ok(())
            } else {
                Err("closed trial identity differs")
            };
        }
        let current = self.slots_of(&trial.owner).unwrap_or(&[]);
        let sources = current
            .iter()
            .filter(|slot| trial.targets.contains(&SlotTarget::from(*slot)))
            .cloned()
            .collect::<Vec<_>>();
        if sources.len() != trial.targets.len()
            || self.word_source_ranges(&sources) != trial.source_ranges
        {
            return Err("recorded trial source ranges differ");
        }
        let component = self
            .word_adjudication
            .components
            .iter_mut()
            .find(|c| {
                c.owner == trial.owner
                    && c.targets == trial.targets
                    && c.conflict
                    && c.trial.is_none()
                    && if trial.fresh_decode {
                        !c.fresh_attempted
                    } else {
                        !c.attempted
                    }
            })
            .ok_or("recorded trial has no current unresolved component")?;
        if trial.id != self.word_adjudication.next_trial.saturating_add(1) {
            return Err("recorded trial sequence differs");
        }
        self.word_adjudication.next_trial = trial.id;
        component.attempted = true;
        component.fresh_attempted |= trial.fresh_decode;
        component.trial = Some(trial.clone());
        self.word_adjudication.active_trials.push(trial.clone());
        Ok(())
    }

    pub(super) fn word_trial_input_refusal(
        &self,
        observation: &ObservationIdentity,
        words: &[WordPin],
    ) -> Option<&'static str> {
        let trial = self.word_evidence_input(observation)?.trial.as_ref()?;
        let current = self.slots_of(&trial.owner).unwrap_or(&[]);
        if observation.occurrence != trial.owner
            || self.is_sealed(&trial.owner)
            || !trial
                .targets
                .iter()
                .all(|target| current.iter().any(|slot| SlotTarget::from(slot) == *target))
            || !self
                .word_adjudication
                .components
                .iter()
                .any(|c| c.trial.as_ref() == Some(trial))
        {
            return Some("stale_target");
        }
        if words.is_empty()
            || words.iter().any(|pin| {
                let range = OccurrenceIdentity::new(
                    &trial.owner.session,
                    trial.owner.capture_epoch,
                    pin.sample_start,
                    pin.sample_end,
                );
                current.iter().any(|slot| {
                    !trial.targets.contains(&SlotTarget::from(slot))
                        && self.word_slot_targets_pin(slot, &range)
                })
            })
        {
            return Some("trial_target_mismatch");
        }
        None
    }

    pub(crate) fn admit_word_trial(
        &mut self,
        trial: &WordTrial,
        observation: &ObservationIdentity,
        words: &[WordPin],
        original: &[WordPin],
    ) -> MutationReceipt {
        // Recognition context can retime an existing word, but can never
        // create a target. The refusal below also protects every neighbor.
        let current = self.slots_of(&trial.owner).unwrap_or(&[]);
        let targets_pin = |word: &&WordPin| {
            let range = OccurrenceIdentity::new(
                &trial.owner.session,
                trial.owner.capture_epoch,
                word.sample_start,
                word.sample_end,
            );
            current.iter().any(|slot| {
                trial.targets.contains(&SlotTarget::from(slot))
                    && self.word_slot_targets_pin(slot, &range)
            })
        };
        let pins = words
            .iter()
            .filter(targets_pin)
            .cloned()
            .collect::<Vec<_>>();
        let raw = original
            .iter()
            .filter(targets_pin)
            .cloned()
            .collect::<Vec<_>>();
        self.stage_word_evidence(observation, &raw, None, "unknown");
        self.word_adjudication
            .input
            .as_mut()
            .expect("staged trial")
            .trial = Some(trial.clone());
        let refusal = self.word_trial_input_refusal(observation, &pins);
        let receipt = self.admit_word_slots(observation, &pins);
        let resolved = self
            .word_adjudication
            .choices
            .iter()
            .rev()
            .find(|choice| choice.observation == *observation)
            .is_some_and(|choice| choice.reason == "trial_confirmed");
        self.close_word_trial(
            trial,
            refusal.unwrap_or(if resolved { "resolved" } else { "unresolved" }),
        );
        receipt
    }

    #[cfg(test)]
    pub(crate) fn has_word_conflicts(&self) -> bool {
        self.word_adjudication
            .components
            .iter()
            .any(|c| c.conflict && (!c.attempted || c.trial.is_some()))
    }

    /// Formatting consumes settled words; it cannot choose an alternative
    /// or turn an inconclusive acoustic trial into a semantic decision.
    pub(crate) fn word_labels_settled(&self, owner: &OccurrenceIdentity) -> bool {
        !self
            .word_adjudication
            .components
            .iter()
            .any(|c| &c.owner == owner && c.conflict)
    }

    pub(super) fn word_finality(&self, owner: &OccurrenceIdentity) -> Vec<WordFinality> {
        self.word_adjudication
            .components
            .iter()
            .filter(|c| &c.owner == owner)
            .map(|c| WordFinality {
                targets: c.targets.clone(),
                selected: c.incumbent.clone(),
                unresolved: c.conflict,
                trial: c.last_trial.clone(),
            })
            .collect()
    }

    pub(super) fn word_trials_pending(&self, owner: &OccurrenceIdentity) -> bool {
        self.word_adjudication
            .components
            .iter()
            .any(|c| &c.owner == owner && c.conflict && (!c.attempted || c.trial.is_some()))
    }

    pub(super) fn close_word_components(&mut self, owner: &OccurrenceIdentity) {
        self.word_adjudication
            .components
            .retain(|c| &c.owner != owner);
        if !self.word_adjudication.retain_closed_history {
            self.word_adjudication
                .choices
                .retain(|choice| &choice.observation.occurrence != owner);
        }
        if self
            .word_adjudication
            .input
            .as_ref()
            .is_some_and(|input| &input.observation.occurrence == owner)
        {
            self.word_adjudication.input = None;
        }
    }

    /// v2 trails used an unpruned choice-vector cursor. Preserve that historic
    /// replay contract without retaining closed revision payloads in live runs.
    pub(crate) fn retain_closed_word_history_for_legacy_replay(&mut self) {
        self.word_adjudication.retain_closed_history = true;
    }
}

#[cfg(test)]
mod boundary_group_tests {
    use super::*;

    #[test]
    fn a_group_cannot_borrow_its_first_words_complete_boundary() {
        let owner = OccurrenceIdentity::new("mixed-word-boundaries", 1, 0, 160_000);
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("boundary-group-test", 1.0, 1);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: 10_000.0,
                        energy_integral: 100.0,
                        mean_rms_dbfs: -20.0,
                        peak_dbfs: -10.0,
                        vad_open_sample: Some(0),
                        vad_close_sample: Some(160_000),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration
                )
                .is_qualified()
        );
        let observation =
            ObservationIdentity::new(ObservationProducer::Whisper, 1, 0, owner.clone());
        ledger.admit_word_slots(
            &observation,
            &[
                WordPin::new(16_000, 32_000, "pierwsze").with_decode_window(0, 64_000),
                WordPin::new(48_000, 64_000, "ucięte").with_decode_window(0, 64_000),
            ],
        );
        let sources = ledger.slots_of(&owner).unwrap();
        assert_eq!(sources.len(), 2);
        let components = &ledger.word_adjudication.components;
        assert!(components[0].incumbent.acoustic_boundaries_complete);
        assert!(!components[1].incumbent.acoustic_boundaries_complete);
        let group = ledger.component_for_sources(&owner, sources);
        assert_eq!(group.whisper.len(), 1);
        assert!(group.whisper[0].complete);
        assert!(!group.whisper[0].acoustic_boundaries_complete);
    }
}
