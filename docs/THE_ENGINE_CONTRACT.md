# THE ENGINE — quality-report contract

|                   |                                                                                                        |
| ----------------- | ------------------------------------------------------------------------------------------------------ |
| id                | `the-engine/v1`                                                                                        |
| corpus schema     | `codescribe-corpus-parity/v3`                                                                          |
| primary key       | `pcm_time`                                                                                             |
| executable mirror | `core/quality/engine_contract.rs`                                                                      |
| surfaces          | **Seal Atlas** in Voice Lab (`voice-lab` tab); gold HTML `docs/quality-reports/seal-atlas.take01.html` |

Do not re-derive this from a convenient implementation detail. This file owns
the product invariant; `ENGINE_CONTRACT` in Rust is its executable mirror. If
they disagree, runtime truth must be reported as drift and both surfaces must
be reconciled in the same cut. A stale implementation does not silently repeal
the product contract.

## Product goal

Place on the canvas is given by **energy in time** — mechanical waves from the speaker's vocal cords, while they speak. Not by tokens.

Four live layers, everything in the buffer, **~10 ms to paste**:

> energy × time → the true sentence, live in the buffer, ~10ms to paste

Preview, colours, successive hypotheses and seals are internal mechanics. The user buys the sentence, immediately, ready to paste. 20 seconds of delay kills even a perfect transcript: it is no longer presence.

Preview is strictly overlay-only paint. Raw final/correction/range-patch/
annotation events are observations or diagnostics. Only ledger mutation/seal
receipts may create committed projections or delivery text.

## Relay

Apple → Whisper → Lexicon + Light+ → Responses formatter → human

This is a band, not a queue of correctors. Ban is **per layer, per span**. The layer that already passed this span is out. The next one may enrich the **same** time window.

- **Apple** draws now (thin, sharp pencil). Its text is a fast hypothesis pinned to PCM time, not a protected word floor.
- **Whisper** enters the buffer on **~4 s observations with ~1 s overlap**, bounded by available speech evidence. Never full audio in the automatic pipeline (`full_file_pass = button_only_proposal`). It may fill omissions or replace weaker Apple wording inside the same proven span. It must not hallucinate into silence or rebuild the session from zero.
- **Lexicon / Light+** are L2 and tune deterministically after Whisper settles. Light+ is currently wired on progressive seals and as the delivery floor.
- **Responses formatter** is L3 (`previous_response_id`). It has a trash bucket: it may throw away approved verbal debris, but it may not rearrange the plate.
- **Human** is last, after seal.

### Exactly four machine layers

L0 — Apple; L1 — Whisper; L2 — Lexicon + Light+; L3 — Responses formatter.

| Layer  | Owner                        | Contract                                                           |
| ------ | ---------------------------- | ------------------------------------------------------------------ |
| **L0** | Apple                        | Fast, PCM-pinned live hypothesis.                                  |
| **L1** | Whisper                      | Deeper overlapping observation of the same proven spans.           |
| **L2** | Lexicon + Light+             | Deterministic vocabulary and sentence shaping; currently wired.    |
| **L3** | Existing Responses formatter | Session-context formatting through the configured Formatting lane. |

Inline describes scheduling of the existing Responses formatter over stable
spans. It does not name a small model, a second client, or a second formatting
product. The human is the recipient after these four machine layers, not a
fifth machine layer.

Silero is orthogonal VAD and PCM-time evidence. It may contribute speech
boundaries, silence duration, pause evidence, and pre-roll; richer paralingual
labels are optional and require a measured provider beyond plain Silero VAD.
Silero does not occupy a numbered text layer.

### Sideband evidence contract

`EngineEvent::SidebandEvidence` carries content-free observations on the same
PCM axis as the ordered span ledger:

- identity is `(session, capture_epoch, sample_start, sample_end, sequence)`
  plus `sample_rate_hz`;
- provenance is typed as `silero_vad`;
- supported claims are `speech_start`, `speech_end`, and a measured pause
  duration whose only non-speech classification is `unknown_non_speech`;
- plain Silero does **not** support laughter, cough, music, speaker, language,
  or named noise labels. Those require a separate measured provider;
- an edge is a zero-width range at the exact threshold-crossing sample; a pause
  is the exact half-open gap from a measured speech end to the next measured
  speech start;
- the event is never `InsertAnnotation`, never mutates committed text, and its
  absence never blocks audio, sealing, delivery, or transcript assembly;
- L3 may consume only measured pause duration, and only as context for
  punctuation or paragraph boundaries. It may not turn the evidence into words
  or sound annotations.

Example pause event (JSON field names match the serialized contract):

```json
{
  "type": "sideband_evidence",
  "evidence": {
    "sequence": 3,
    "range": {
      "session": "session-abc",
      "capture_epoch": 2,
      "sample_start": 16000,
      "sample_end": 24000
    },
    "sample_rate_hz": 16000,
    "provenance": "silero_vad",
    "evidence": {
      "kind": "pause",
      "duration_samples": 8000,
      "non_speech": "unknown_non_speech"
    }
  }
}
```

The Apple lane emits this evidence whenever its existing single
`SileroIngress` is present (`CODESCRIBE_SILERO_FUSION=1` or the configured
hands-free epoch lifecycle needs speech edges). There is no second VAD and no
new sideband flag. If Silero cannot load, `EpochGate` disarms and Apple runs as
one continuous stream with no sideband events.

`CODESCRIBE_SILERO_FUSION` is still default-OFF in `ENV_REGISTRY.toml`, but it
is no longer a diagnostic-only lane: under the one-throne corridor an
occurrence exists only when calibrated energy **and** a Silero-bounded region
agree, and `seal_utterance_final` lets a region qualify only when it was
Silero-bound (`may_qualify = silero_bound`). With the lane off, no occurrence
can ever qualify and no utterance can commit. The controller therefore treats
a disarmed lane as an admission blocker (`admission_seal_lane_disarmed`) and
refuses to open the microphone rather than recording into a ledger that
cannot seal. Flipping the registry default is an operator decision; the
earlier enclosing-range concern (fusion widening a pending Apple range beyond
the Layer 1 request identity) still needs its falsifier before that flip.

Final BAM is superseded and has no automatic content producer. Normal stop
drains already admitted work and assembles the ordered span ledger; it does not
start a fifth rewrite. SessionFinalised is lifecycle-only and may not mutate
text.

`NEVER REWRITE FROM ZERO.` The **PCM axis and ordered span ledger** are
append-only. Text hypotheses inside an authorized, not-yet-session-sealed span
remain correctable. Key = PCM sample counter, not token position.

## Three bars — not synonyms

| Bar                                                   | Means                                                                                                                                                                                                                  |
| ----------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `utterance_final` / observer-final                    | This observer finished its current raw hypothesis for the fragment. **Not the document, Bus, delivery, or an immutable token floor.** A later authorized observer may relabel the same proven span through the ledger. |
| `utterance_sealed`                                    | The span identity and `[sample_start, sample_end)` placement are frozen. Its text is stable for presentation but remains correctable by an admitted downstream observation before session seal.                        |
| terminal ledger seal / `transcript_sealed` projection | A terminal ledger seal receipt closes the committed Bus writer. Arbitrary text cannot seal it. Full HQ / Cloud may only propose a variant.                                                                             |

`committed` does **not** mean "this is already the document". It means: **this layer finished its work here; the next layer takes the same time slice.**

## What is _not_ true

Before `transcript_sealed` the whole document is **not** mutable.

- Closed spans stay on their places on the PCM axis.
- Utterances may not be reordered. Text may not be built from zero.
- The current tail may still evolve.
- Whisper may fill holes and replace weaker evidence inside the same authorized span before session seal.
- The formatter works in parallel on closed fragments and keeps their order.
- Stop closes only the tail and assembles ready fragments.

A first-wins final string is not enough. The real document is the ordered span ledger with provenance. Session seal closes the assembled result — it does not replace the architecture with one frozen variable.

## Quality-report column roles

| Column                  | Role                                                                              |
| ----------------------- | --------------------------------------------------------------------------------- |
| `raw` / `live`          | live hypothesis                                                                   |
| `post` / `layer1`       | Whisper span-bound correction or gap-fill                                         |
| `sealed`                | sealed span                                                                       |
| `delivered` / `session` | session document                                                                  |
| `ai` / `cloud` / `hq`   | `HumanTriggeredProposal` — WER against a proposal does not promote it to document |

## Forbidden (reports and agents)

- `rewrite_from_zero`
- `reorder_spans`
- `hallucinate_into_silence`
- `full_file_in_automatic_pipeline`
- `auto_replace_after_transcript_sealed`
- `treat_committed_as_document`
- `treat_whole_text_mutable_until_session_seal`
- `treat_apple_text_as_immutable_floor`
- `infer_span_identity_from_text_similarity`
- `infer_named_sound_from_silero`
- `deduplicate_intentional_repetition_by_content`
- `treat_mean_energy_db_as_identity`
- `claim_layered_on_when_no_windows_reach_the_provider`
- `drop_acoustic_observation_without_receipt`
- `declare_a_pcm_range_the_payload_does_not_carry`
- `present_mean_energy_as_span_identity`

## Founding invariant — restored 2026-08-21

The sentence is not a string that one recognizer owns.

The sentence is an ordered projection of observations over audio time.

The stable object is:

```text
session
  → capture epoch
  → PCM sample axis
  → ordered spans
  → observations with provenance
  → current canvas projection
  → transcript seal
```

The unstable object before session seal is:

```text
the current wording attached to an authorized span
```

This distinction is the engine.

Replay is re-delivery of the same **observation** (producer, request,
generation, occurrence). It is not "the same PCM range". Apple and Whisper on
one range are two observations of one occurrence; Whisper may correct Apple
there. Two disjoint ranges with the text "Iwo" are two occurrences. Overlap
may clip a phrase only when word pins prove which text belongs to the exclusive
tail; otherwise the text stays visible as read-only evidence and must not mint
a duplicate token. Unanchored text stays visible without mutation authority.

### Audio truth

- Mechanical speech energy exists before transcription.
- Capture PCM is the evidence retained by the product.
- Its monotonic sample counter is the shared clock.
- A provider timestamp is mapped onto that clock.
- A provider timestamp never replaces that clock.
- A token offset is not a clock.
- A character position is not a clock.
- Similar words are not identity.
- A model confidence score is not identity.
- A final callback is not identity.
- Identity is minted from capture/session/span evidence.
- Mean `energy_db` is quality evidence on the PCM axis, never a collision-proof ID.
- `dB × ms` names coordinates and hop evidence, not a scalar hash of average loudness.
- Two identical tokens on disjoint `[sample_start, sample_end)` ranges are two observations.
- Replaying one range must not mint another token. Text-suffix overlap must not collapse them.
- Executable admit/seal path: `core/pipeline/acoustic_ledger.rs`
  (`AcousticLedger::admit` and `AcousticLedger::seal`). Live Apple observations
  enter that authority through `admit_ledger_label`.
- Text overlap never establishes occurrence identity or admission authority.

### Ledger identity — identity is not evidence

A word is not a string. A word is an observation of a captured PCM interval
together with the intensity measured on that interval. Two lexically identical
words occupying different intervals are two different observations and both
must survive.

The engine therefore carries **two** separate objects for the same span. They
must never be collapsed into one another.

**`OccurrenceIdentity` — the structural key.** Comparable for equality and
content-free:

```text
session          — capture session id
capture_epoch    — successful physical-open epoch inside that session
sample_start     — inclusive, on the capture PCM counter
sample_end       — exclusive, on the same counter
```

- Equality is the whole 4-tuple. Nothing else is physical identity.
- A same-range replay is the same occurrence, not a new one. Two disjoint
  ranges carrying equal text are two occurrences and both survive.
- Zero-width or reversed ranges are unanchored evidence and carry no mutation
  authority.

**`ObservationIdentity` — one hypothesis about that occurrence.** It adds the
producer, request, and generation to `OccurrenceIdentity`. Generation orders a
hypothesis; it never changes how many physical occurrences exist.

**Acoustic evidence — the quality proof.** Measured, lossy, optional:

```text
hops        — the capture energy hops overlapping the identity range
mean_db     — convenience aggregate over those hops
grain       — word / phrase / utterance, as the recognizer actually reported
timing      — exact_sample_range | compacted_speech_relative | synthetic
```

- Evidence proves an identity is _anchored in voiced audio_. It does not name
  the identity.
- `mean_db` is a scalar average. It is **not** collision-proof and may never be
  hashed, compared, or promoted into a key
  (`present_mean_energy_as_span_identity`).
- Absent evidence is an honest `unanchored` label. It leaves the text visible
  and strips the right to mutate a neighbour; it never deletes the text.
- Whisper's mel is fuel for one forward pass and is gone after the decode. It
  is not a durable word number and must not be presented as one. Any richer
  fingerprint must first state its cost, its privacy class, its retention, and
  its comparison rule.

**Authority order.** Authority is established from identity plus evidence
_first_. Text similarity may be used only afterwards, and only to align inside
one already-authorized identity. A textual match never establishes, extends, or
transfers authority (`infer_span_identity_from_text_similarity`).

**Conservation.** `AcousticLedger::admit` records one decision per offered
observation, and `AcousticLedger::seal` closes the physical occurrence.
`EngineEvent::LedgerMutation` and `EngineEvent::LedgerSeal` carry those receipts
to `PresentationEmitter` / `TranscriptReducer`; Transcript Bus and Swift only
observe the committed projection. Text producers may relabel an authorized
occurrence, but may not mint, merge, or erase physical speech.

### Apple truth

- Apple optimizes time-to-first-useful-text.
- Apple can be excellent and still be wrong.
- Apple can lose the first token.
- Apple can collapse a technical phrase into common words.
- Apple can choose the wrong inflection.
- Apple can emit cumulative finals with awkward geometry.
- Apple can provide real per-word pins.
- Apple can provide only utterance-grain timing.
- Apple can exhibit clock-lie.
- Apple text is therefore evidence, never ownership.
- An Apple commit pins a hypothesis to a region of time.
- It does not make the literal hypothesis immutable.

### Whisper truth

- Whisper exists in the automatic pipeline to repair live hypotheses.
- Whisper does not exist merely for a manual rescue button.
- Whisper observes approximately four seconds at a time.
- Consecutive observations overlap approximately one second.
- The overlap carries linguistic context across boundaries.
- The overlap must not duplicate canvas content.
- Request/span identity resolves overlap replay.
- Whisper may correct a word family or inflection.
- Whisper may replace a malformed phrase.
- Whisper may restore code-switching or a technical name.
- Whisper may append speech absent from the canvas.
- Whisper may remove its own span's hallucinated text.
- Whisper may not use an unrelated window to alter a neighbor.
- Whisper may not write into verified silence.
- Whisper may not replace the complete session automatically.

### Layer 1 window algorithm — ~4 s observation, ~1 s overlap

The cadence is normative, not decorative. A window is a **contiguous slice of
the capture PCM axis**, never a concatenation of non-adjacent fragments.

1. **Mint the window.** Advance a cursor on the capture sample counter. A
   window is `[cursor, cursor + 4 s)` clipped to admitted speech evidence; the
   next window starts at `cursor + 3 s`, so consecutive windows share ~1 s.
   Window identity is an `AcousticSpanIdentity` over exactly that range.
2. **Carry what you declare.** The PCM handed to the provider is the literal
   `[sample_start, sample_end)` slice. A payload whose sample count disagrees
   with its declared range is refused _before_ inference, with a named receipt
   — never silently, and never by rewriting the range to fit the buffer
   (`declare_a_pcm_range_the_payload_does_not_carry`). Coalescing several
   utterances into one job is legal only when their ranges abut; a gap is
   either included as real audio or the job is split.
3. **Map back to one clock.** Provider timestamps are provider-local. Each
   returned segment is mapped onto the capture counter and re-anchored inside
   the request range. If the mapping cannot be proven — VAD-compacted decode
   with no surviving index, a segment that escapes the request range, a
   degenerate zero-width result — the segment is marked `unanchored` and kept
   as read-only evidence. It is never dropped silently and never granted
   mutation rights.
4. **Resolve the overlap by identity, not by text.** For the shared ~1 s, the
   later window's observations whose identity range is already covered by an
   admitted earlier identity are **replay** and are refused with
   `replayed_range_identity`. Observations in the non-overlapping remainder are
   new. Two lexically identical observations on two distinct ranges are two
   observations; the overlap resolver may not compare their strings.
5. **Intentional repetition.** Repetition is decided on ranges only. N distinct
   ranges carrying the same text yield N delivered observations. A content
   match against a _new_ identity is a WARN receipt and the text still lands.
6. **Clock-lie.** A span whose character rate exceeds
   `CLOCK_LIE_CHARS_PER_SEC` over its declared range is flagged. A flagged span
   keeps its text and loses the right to authorize a replacement of a
   neighbour; it does not lose the text itself.
7. **Word-grain vs utterance-grain.** Word pins are used where the recognizer
   actually returned them. Utterance grain is reported as utterance grain and
   is never expanded into invented per-word ranges. Bounded replacement inside
   an utterance-grain span addresses the whole span or nothing.
8. **Gap fill.** Speech present in the window and absent from the canvas is
   appended at the identity that carries it, in PCM order. An append whose
   anchor cannot be placed on a proven identity escalates to the stop path
   instead of guessing a position.
9. **Bounded replacement.** A replacement is admitted only when the evidence
   identity and the target identity share `session` and `capture_epoch` and
   their ranges intersect. Change ratio and LCS may rank candidates inside that
   one authorized identity. They may not be the gate.
10. **Drain on stop.** Stop closes the open window, drains admitted work, and
    assembles the ordered ledger. It starts no new decode and re-decodes
    nothing already covered.

### Safety truth

- Safety protects correspondence between canvas and audio.
- Safety does not protect the first textual guess.
- A small textual diff is not automatically safe.
- A large textual diff is not automatically unsafe.
- A correction is safe only when its audio authority is proven.
- Text alignment happens after authority is established.
- Change ratio is a heuristic, not a constitutional boundary.
- `never delete` is not a valid global safety law.
- `never replace committed words` is not a valid global safety law.
- `preserve intentional repetition` is a valid law.
- `reject replay of the same span identity` is a valid law.
- Those two laws are compatible only through structural identity.

### Seal truth

- A layer final closes that layer's turn, not the document.
- An utterance seal freezes span identity and time order.
- It does not canonize Apple's exact characters.
- A transcript seal closes automatic mutation.
- After transcript seal, automated providers may only propose.
- Explicit Retranscribe is a new user-authorized inference action.
- Retranscribe may produce a whole-file result.
- Its existence does not excuse a broken live Layer 1.
- If Retranscribe recovers meaning lost by Delivery, live refinement failed.

## Product modes

### Apple-first local power

Target semantics:

```text
microphone
  → Apple immediate observations
  → live canvas
  → local FP16 Whisper overlapping observations
  → span-bound corrections
  → lexicon
  → optional formatting
  → delivery
```

- Apple owns first paint.
- Local Whisper owns no document.
- Local Whisper must be available before Layered reports ready.
- Model cold load must not block capture callbacks.
- Inference must not backpressure the microphone.
- One model instance may serve sequential jobs.
- Each request retains its own identity and evidence.
- Failure degrades explicitly, not silently.

### Apple-first cloud

Target semantics are identical.

- Transport may be WebSocket or another authorized stream.
- Audio egress requires explicit consent.
- Cloud partials remain volatile until admitted.
- Cloud finals use the same span authority as local finals.
- Cloud may not receive broader mutation rights than local.
- Local may not receive broader mutation rights than cloud.
- Provider failure preserves the best grounded canvas.
- Provider failure emits a typed degradation receipt.

### Apple-only

- Apple-only is a deliberate privacy/availability mode.
- It is not the intended maximum-quality mode.
- It must be labeled as lacking Layer 1 refinement.
- It must not masquerade as Local Power.
- It must not display Layered ON.

### Historical Whisper-first route — superseded, no current authority

The pre-C6 Whisper-first VAD/scheduler route is retained only as dated design
archaeology. It is not a live alternative dispatcher and must not be restored.
In the structural lineage beginning at executable cut `484095ce`, `transcription_session` dispatches only to
`apple_stream_transcription_session`; Whisper may contribute an authorized
Layer 1 observation on retained PCM inside that Apple-ledger session.

## Current structural truth — C11 working cut (2026-08-25)

`484095ce` was the last executable-code cut before docs successor `d57196ab`.
C11 is the next structural executable cut; its actual commit is recorded only
in the durable report. Compiler and runtime are `NOT_ASSESSED`.

- `RecordingController` is the only in-app microphone owner.
- `StreamingRecorder::start_event_session` computes the next `capture_epoch`
  with checked arithmetic before open and assigns it only after
  `recorder.start()` succeeds.
- `transcription_session` dispatches only to
  `apple_stream_transcription_session`.
- Silero supplies boundary, time, and energy evidence; it owns no text.
- Apple, Whisper, Lexicon/Light+, and Responses formatting observe or relabel an
  occurrence already authorized by `AcousticLedger`.
- `AcousticLedger` alone admits and seals physical occurrences.
- `PresentationEmitter` / `TranscriptReducer` commit ledger events. Transcript
  Bus and Swift are projections, and delivery follows explicit `DeliveryRoute`.
- Fusion-sliced Apple words are admitted per exact Silero range before raw final
  telemetry; callback-wide labels are not replicated across slices.
- Preview uses an overlay-only command and cannot write delivery or Bus state.
  Raw final/correction/range-patch/annotation events do not mutate the document.
- The Bus has one committed writer family, `publish_revision`, and terminal
  ledger seal closes it. Draft/arbitrary-text seal APIs and the raw-event delta
  adapter no longer exist.
- Normal product stop has no automatic whole-file pass. Explicit Retranscribe
  remains a separate operator action.

This is structural source evidence, not a compiler or runtime claim.

### Historical acoustic-identity defects — measured 2026-08-22 on `a95e1272`, superseded as current authority

These reproductions remain useful archaeology, but describe the pre-C6 tree and
have no current architectural authority. Current replacements are stated next
to the resolved defects; this section is not a work queue.

- **The energy clock has no consumer that decides anything.**
  `CaptureLevelAccumulator::push_samples` records an energy hop per capture
  block (`core/audio/capture_receipt.rs`), and `session_energy_db(start, end)`
  is read in exactly one place — `word_spans_from_draft`
  (`app/presentation/transcript_bus.rs`), an observer that turns it into a
  coverage receipt. Layer 1 acceptance, the overlap resolver, the seal machine,
  L2 lexicon, L3 formatting, and delivery read it zero times.
- **The energy clock is epoch-blind while every span identity is epoch-keyed.**
  `session_energy_db` takes `(u64, u64)` only. `begin_session_energy_clock()`
  is called once per session while `capture_epoch` advances inside a session,
  so an energy lookup cannot distinguish two epochs sharing a sample range.
- **Layer 1 discards the segment ranges it just validated.**
  `compute_tail_patch_job_with` receives a `TailProviderPayload` whose
  `segments: Vec<TimedTailSegment>` each carry a `TailSampleRange`, then passes
  only `payload.text` into `compute_tail_patch_with_context`
  (`core/pipeline/streaming/session.rs`). The mutation decision is taken by
  token LCS plus a change-ratio cap on flat strings; the ranges never reach it.
- **Coalesced Layer 1 windows declare a range they do not carry.**
  `build_flush` (`core/pipeline/streaming/layer1_window.rs`) concatenates the
  PCM of several pieces while declaring `[first.sample_start, last.sample_end)`.
  Any gap or pad overlap between pieces breaks the equality that
  `TailProviderRequest::validate_pcm` enforces, and the whole window fails as a
  generic provider error. Reproduction: the module's own
  `flushes_after_five_segments` fixture yields a declared 70 400 samples
  against 31 999 carried samples. A single-piece window is unaffected.
- **Resolved: the window map back to member utterances is PCM, not char-offset.**
  `ConcatSpan { utterance_id, start, end }` addressed the concatenated committed
  _string_, and `remap_concat_events` / `split_outcome_for_members`
  redistributed Layer 1 output on those character positions. Both the type and
  the remap island are gone (W3B). A coalesced window now carries
  `member_occurrences: Vec<(u64, OccurrenceIdentity)>`, and
  `complete_whisper_window` (`core/pipeline/streaming/apple_live_session.rs`)
  keeps only provider segments whose sample range lies wholly inside one
  member's occurrence. A candidate that straddles a join is admitted to neither
  member instead of being rewritten into the first span.
- **The cadence constant and the runtime disagree.**
  `ENGINE_CONTRACT.whisper_window` says `approximately_4s_with_approximately_1s_overlap`.
  `Layer1Coalesce` flushes on `TARGET_SEGMENTS = 5`, `MAX_AUDIO_SECS = 16.0`,
  or a `PAUSE_SECS = 1.2` gap, and produces disjoint windows with no overlap.
  `full_file_pass_is_never_automatic` asserts the spelling of the constant, not
  the behaviour, so the disagreement is invisible to the gate.
- **Pre-C6 repetition defect, resolved structurally by C6.** The Apple
  segment-less final path deleted repetition by text. When an
  Apple final arrives without usable segments, `seal_utterance_final`
  (`core/pipeline/streaming/apple_live_session.rs`) matches the callback
  against the canvas with `revision_tolerant_known_prefix`, a banded
  edit-distance search that maximizes the consumed prefix over _every_ start
  position in the canvas tail, with an edit budget of `max(n / 5, 1)`.
  Reproduction: a canvas carrying four `Iwo` and a cumulative final carrying
  five yields `known_prefix = 5`, `novel_text = ""`, and the fifth acoustic
  occurrence is discarded. The same probe matches a canvas region with no
  temporal relationship to it. This is `deduplicate_intentional_repetition_by_content`
  and `infer_span_identity_from_text_similarity` in that historical path. On
  `484095ce`, `revision_tolerant_known_prefix` has no executable occurrence;
  `seal_utterance_final` binds the callback to new session-clock PCM and routes
  Apple and Lexicon observations through `admit_ledger_label` into
  `AcousticLedger`.
- **Apple final segments that straddle the cursor are dropped, not trimmed.**
  The overlap normalization in `seal_utterance_final` drops a whole segment on
  `start_ts < cursor - epsilon`, so a segment that begins before the cursor and
  extends past it loses its non-overlapping tail. One aggregate WARN
  (`apple_final_window_overlap_normalized`) is emitted for the callback; no
  per-observation receipt names what was removed.
- **Live capture epoch ownership is explicit.** `StreamingRecorder` computes
  the next epoch with checked arithmetic before opening the device, commits it
  only after a successful open, and threads that value into the Apple state.
  A new operator-session bind resets the counter; stop/discard does not.
  Offline one-file replay seams use caller-domain epoch `1`.
- **Pre-C6 receipt surface, superseded by `AcousticLedger`.** Structural
  receipts were computed and thrown away.
  `SpanIdempotenceLedger` records `replayed_range_identity`,
  `replayed_request_identity`, `non_progressing_timestamps`, `decode_failure`,
  and `content_similar_preserved`. `span_idempotence_receipts()` has no
  consumer outside its own module, so `structural replays rejected` and
  `intentional repetitions preserved` were never reported. On `484095ce`,
  `AcousticLedger::admit` records the decision, `decide_observation` refuses a
  repeated observation identity, and ledger mutation/seal events reach the
  reducer.
- **`TailTimingQuality::CompactedSpeechRelative` is never constructed.** Only
  `ExactSampleRange` is emitted, including for the in-process path that decodes
  VAD-compacted audio and maps back through `map_compacted_sample_range`. When
  that mapping returns `None` the segment is dropped by `filter_map` with no
  receipt.
- **The executable mirror is missing five prose forbiddens.**
  `ENGINE_CONTRACT.forbidden` did not carry
  `treat_apple_text_as_immutable_floor`,
  `infer_span_identity_from_text_similarity`,
  `deduplicate_intentional_repetition_by_content`, or
  `claim_layered_on_when_no_windows_reach_the_provider`, and carries
  `small_inline_llm` which the prose list does not. Reconciled in this cut.
- **`LayerSummary` still names superseded producers.**
  `final_bam_replacements` and `inline_llm_replacements` remain live fields on
  the session receipt for a producer the ledger declares superseded and a layer
  the contract forbids.

## Model contract

- Runtime Whisper is large-v3-turbo FP16/F32 only.
- Q8 is retired absolutely.
- Q8 is not a fallback.
- Q8 is not an explicit-path exception.
- Q8 is not an embedded-build exception.
- Q8 dequantization code must not exist in the loader.
- Config, tokenizer, mel, and weights form one bundle.
- Tokenizer must parse.
- Mel must match the pinned SHA-256.
- Safetensors must contain at least one nonempty tensor.
- Model tensors use F16 or F32.
- `alignment_heads:I64` is the single named format exception.
- U32, I32, arbitrary integer tensors, scales, and biases are refused.
- Tensor shapes, byte sizes, offsets, gaps, overlaps, and payload length validate.
- Metadata must match upstream safetensors schema.
- A corrupt preferred weights filename may not shadow a valid alternate.
- A corrupt newer HF snapshot may not shadow a valid older snapshot.
- Runtime, downloader, release preflight, and embedded build must share validation.
- Filename presence alone never means installed.
- Invalid partial downloads never become final files.
- Invalid final destinations are repaired or quarantined.
- Valid existing artifacts are reused offline.

## Settings contract

- `settings.json` is the durable product source of truth.
- `.env` may seed absent promoted values.
- `.env` may not remain a second independent writer.
- UI readback uses the same effective value as recording start.
- UI writes become visible to the next recording without relaunch.
- `ASR mode`, `STT engine`, and `Layered` are distinct dimensions.
- Local Power means local Layer 1 capability is intended.
- Cloud means audio egress is consent-gated.
- Apple Only means no Layer 1 provider.
- `Final Pass off` concerns stop-path whole-file inference.
- `Layered off` concerns during-hold refinement.
- The two switches are orthogonal.
- A stale `final_pass_mode=smart` token must not reactivate hated Full Pass.
- A Layered toggle ON must be backed by an armed lane receipt.
- A missing model produces a visible not-ready/degraded state.
- Installed model status comes from full validation, not file names.

## Acceptance recordings

The following are contract fixtures, not anecdotes:

### Meaning-loss fixture

Spoken intent:

```text
Whisper musi łatać partiale.
```

Failure observed:

```text
mój model pt. Musi latać
```

Acceptance:

- Apple may show the weak hypothesis initially.
- Whisper receives the corresponding PCM observation.
- The canvas is corrected before Delivery.
- Delivery retains the repaired meaning.
- Manual Retranscribe must not be the first place meaning returns.

### Onset fixture

Spoken first token:

```text
IWO
```

Acceptance:

- The first speech token survives capture and seal.
- Demux receives the same first token as the audio.
- Failure is classified as onset/pre-roll or ASR adjudication.
- Demux grammar is not blamed for a token absent from the bus.

### Repetition fixture

- Five intentional repetitions occupy five distinct span identities.
- All five survive projection and delivery.
- Replaying one identity does not create a sixth copy.
- Text-equality deduplication is forbidden.
- The count holds when the recognizer restates cumulatively: a canvas carrying
  four occurrences and a cumulative final carrying five deliver five, and the
  fifth is admitted on its own range, not on the length of the restatement.
- The count holds when the five occupy one Apple commit with utterance grain
  and no per-word pins.

### Conservation fixture

- `count(delivered observations bound to distinct identities)` equals
  `count(admitted acoustic observations)` for the epoch.
- Every difference between the two counts resolves to exactly one receipt
  naming preserve, correct, or refuse, and the identity it applies to.
- Deletion, insertion, merge, split, reorder, and substitution each require the
  same-span acoustic authority; a receipt-less one fails the take.
- `manual_human` active-name evidence fixes the spelling for its matching
  identities. Downstream layers may preserve it; normalizing it to another
  spelling is a refusal, not a correction.

### Model fixture

- Complete valid FP16 loads and decodes real audio.
- Real Q8 is refused before tensor load.
- Corrupt mel repairs on retry.
- Corrupt tokenizer repairs on retry.
- Invalid preferred weights falls through to valid alternate.
- Invalid newest HF snapshot falls through to valid older snapshot.
- Malformed metadata is refused during discovery.

## Required receipts

Every live session reports:

- selected STT engine
- resolved ASR product mode
- effective Layered phase
- Layer 1 armed/disarmed reason
- provider kind
- model identity when local
- model validation result
- windows admitted
- windows coalesced
- windows unresolved
- provider jobs started
- provider jobs completed
- corrections applied
- corrections refused
- structural replays rejected
- gaps appended
- intentional repetitions preserved
- jobs abandoned
- drain timeouts
- first covered sample
- last covered sample
- transcript seal timestamp
- delivery timestamp
- observations admitted on the PCM axis
- observations delivered bound to a distinct identity
- observations unanchored (kept, no mutation right)
- observations refused, by named reason
- windows refused before inference, by named reason
- energy-evidence lookups that returned no voiced hop

The last six close the conservation loop. Admitted minus delivered must equal
the sum of the named refusals; a residue with no name is the failure the
receipt exists to expose. A window refused before inference is reported as its
own class and never folded into `provider jobs completed` or into a generic
skip bucket — that folding is how `claim_layered_on_when_no_windows_reach_the_provider`
survives a green session.

The zero-work receipt is diagnostic:

- ON + zero admitted windows is failure.
- OFF + zero admitted windows is expected.
- unavailable + zero admitted windows is explicit degradation.
- no log line is not an acceptable state.

## Anti-drift rules

- Never turn a heuristic into an operator law.
- Never attribute an agent inference to a named operator.
- Never preserve a known-wrong rule for compatibility without labeling it.
- Never let a green test sanctify superseded behavior.
- Never let an old report outrank current runtime.
- Never let current broken runtime redefine the product goal.
- Product intent tells us what to build.
- Runtime tells us what currently works.
- Tests prove only the behavior they actually assert.
- Contracts must state target and current gap separately.
- Any change to authority updates this file and `TRANSCRIPT_LANES.md`.
- Any change to configuration updates `STT_CONTRACT.md` and `ENV_REGISTRY.toml`.
- Any change to receipts updates quality schemas and tests.
- Any accepted correction must be reproducible from its evidence.
- Any rejected correction must have a named reason.
- Any temporary OFF must name the missing falsifier/evidence required for ON.

## Transcript Bus and observer contract

- `PresentationEmitter` is the transcript reducer of record.
- The Transcript Bus observes committed reducer events.
- The Bus never opens a microphone.
- Diagnostic tools never open a competing recorder.
- One in-app `RecordingController` owns microphone capture.
- Dictation, Agent, and Assistive select consumers, not recorders.
- Bus path resolution is shared with the application.
- Resolver order is contractual.
- `CODESCRIBE_TRANSCRIPT_BUS_PATH` is the explicit override.
- XDG state participates only where documented.
- `CODESCRIBE_DATA_DIR` participates where documented.
- `~/.codescribe` is the final fallback.
- An observer may not invent an undocumented alternate key.
- A follower stores byte offsets, not decoded-character estimates.
- An incomplete UTF-8 sequence remains buffered as bytes.
- Poll boundaries may split any multibyte code point.
- A follower may act on side effects only after seal.
- Draft events may drive conversation preview only.
- Agent name assignment requires an unambiguous addressing phrase.
- A casual greeting may not permanently rename a follower.
- Failure to hear a name absent from sealed text is an ASR failure.
- Follower liveness and microphone ownership are separate facts.
- A live process writing to a dead terminal is not an effective observer.

## Delivery contract

- Delivery follows explicit operator intent.
- OS focus is not delivery authority.
- The capture-start Agent thread owns that take.
- Browsing another thread cannot steal in-flight speech.
- Clipboard, paste, canvas, and Agent are distinct routes.
- Auto-paste requires positive confirmation of the latched target.
- A successful activation request is not positive confirmation.
- A timeout with Codescribe still frontmost is not confirmation.
- Ambiguous activation fails closed.
- Fail-closed delivery preserves the user's clipboard.
- A failed paste presents a recoverable Paste Here/Copy route.
- Formatting may be vetoed without discarding raw text.
- Revert returns to the raw first version.
- Delivery text must equal the reducer's sealed projection plus authorized transforms.
- UI preview is not delivery truth.
- Raw engine text is not delivery truth.
- Manual Retranscribe is not delivery truth unless the user accepts it.
- Delivery receipts identify route, target, seal, and applied transforms.

## Performance contract

- Time-to-first-useful-text matters more than batch elegance.
- Capture callbacks never wait for Whisper inference.
- Capture callbacks never wait for network inference.
- Capture callbacks never wait for UI rendering.
- Layer 1 queues are bounded.
- Queue overflow degrades refinement, never capture.
- One in-flight local patch is an acceptable initial bound.
- Stop drain is bounded and measured.
- No hidden 8-second whole-file pass runs after ordinary Fn release.
- Model load happens once per residency epoch, not per audio packet.
- Local calls share model weights while retaining request state.
- FP16 preparation performs no Q8 dequantization.
- Cold-load, warm inference, stop drain, and delivery latency are separate metrics.
- Measurements name hardware and build.
- A single-machine benchmark is evidence, not a universal promise.
- RSS, Metal buffers, and TTL residency are reported separately.
- A hot observer process such as `voc` is not blamed on Codescribe without a joint sample.
- A sample without a running Codescribe process proves nothing about Codescribe heat.
- CPU percentage of one core is not total-machine percentage.
- Physical heat requires evidence beyond one process sample.

## Verification contract

Static gates:

- Rust format
- Clippy with warnings denied
- Semgrep
- environment registry validation
- gate ledger validation
- shell syntax and ShellCheck for changed scripts
- Markdown/Prettier for changed documents

Hermetic gates:

- workspace tests
- doctests
- model bundle fixtures
- direct loader refusal fixtures
- reducer and presentation tests
- span identity and replay tests
- intentional repetition tests
- bounded-drain tests
- settings serialization/readback tests

Host gates:

- real official FP16 load
- real Q8 refusal before tensor load
- real Apple progressive take
- real Layered armed receipt
- real correction before Delivery
- real onset fixture
- real repetition fixture
- real app relaunch after install
- deep codesign verification when release-impacting

Reporting rules:

- A pre-change green test is not post-change verification.
- A process surviving is not proof it is the newly installed image.
- Files on disk and running-process identity are separate checks.
- A locked-desktop UI timeout is observation failure, not automatic regression.
- Host-only evidence is labeled host-only.
- Missing secrets are labeled unavailable, not failed code.
- A skipped gate is never summarized as pass.
- A known unrelated failure remains visible.
- Changed-file green does not erase repo-wide red.
- Repo-wide red does not erase a proven focused result.
- Every claim names the command, artifact, or receipt that supports it.

## Completion contract

The engine is not complete when it merely compiles.

It is complete for a cut only when:

- target behavior is explicit
- current behavior is mapped
- authority boundaries are preserved
- code implements the intended lane
- tests attack likely counterexamples
- settings expose actual runtime truth
- receipts make degradation visible
- normal-stop latency remains bounded
- model installation is valid and repairable
- the installed app runs the intended commit
- the user-visible failure case is improved
- documentation describes the same state
- unrelated Living Tree work is preserved
- the coherent cut is committed
- outward actions match operator authorization

## Supersession ledger

Superseded:

- Apple text is the immutable live floor.
- Committed means document-final.
- Sealed span text is append-only.
- Whisper may only add missing suffixes.
- A small textual diff proves safe identity.
- Never delete is a universal safety law.
- Layered is merely an optional experiment.
- Manual Retranscribe is an adequate substitute for live repair.
- Layer 1 behaves identically on Apple and VAD paths.
- Settings ON proves runtime arming.
- Filename presence proves model installation.
- Green CI proves PR completion.

Restored:

- Audio time is truth.
- Apple creates fast temporal pins.
- Whisper continuously re-observes overlapping audio.
- Canvas text evolves within proven span authority.
- Span identity, ordering, and provenance are invariant.
- Automatic whole-session rewrite is forbidden.
- Final BAM is superseded; no automatic content producer owns a fifth layer.
- `SessionFinalised` is lifecycle-only and never edits the document.
- Q8 never enters runtime.
- FP16 is complete, validated, and exercised.
- Layered ON returns when every accepted mutation path is evidenced.

## Historical convergence plan — superseded by the C6 ledger migration

The 2026-08-22 cut order below is retained as measured archaeology, not as
current implementation guidance. It led to the three-way identity split:

The model shipped is a three-way split, not the single `AcousticSpanIdentity`
step 1 sketched. The correction is the point: `order` on the physical identity
would let a replay mint a new occurrence by arriving late.

- `OccurrenceIdentity` — session, capture epoch, sample range. Nothing else.
  No text, no producer, no order.
- `ObservationIdentity` — producer, request/window, generation, occurrence.
  `order` lives here.
- `MutationReceipt` — `preserve` / `correct` / `insert` /
  `keep_visible_unanchored` / `refuse`, one per offered observation, in input
  order. Conservation is auditable because the receipt count always equals the
  observation count.

That historical cut also established the following facts:

- **Step 3.** A coalesced Layer 1 window splits at every PCM gap instead of
  declaring `[first.start, last.end)` over audio it dropped. One flush per
  contiguous run, each declaring exactly what it carries.
- **Historical step 6.** `revision_tolerant_known_prefix` briefly lost authority
  while remaining as an alignment hint. C6 subsequently removed it. It has no
  executable occurrence on `484095ce`; do not restore it.
- **Step 4, partially.** Layer 1 already fenced on PCM identity and already
  validated segment containment, ordering and non-overlap. The gap was that the
  spans did not protect the text; the surviving span count now reaches the
  repetition cleanup. Identity-first ranking _inside_ an authorised span is not
  done — LCS still ranks across the whole payload text.

Two defects found while cutting, neither in the original 12:

- **Light+ deleted every immediately repeated word.** `collapse_tokens` turned
  five spoken occurrences of a name into one, on string equality alone. Removed.
  Hesitations and punctuation runs still collapse — a hesitation is a
  non-lexical sound and punctuation is characters, neither is an occurrence.
- **The decoder-loop remover ran on any run of three identical words**, with no
  acoustic evidence, on every processed chunk. It is now gated on the span
  count: a run with one span per copy is speech; only a run longer than the
  audio can account for is collapsed.

Historical open findings at that snapshot, stated rather than implied:

- A cumulative Apple final under-declares its window by construction — its text
  restates the whole phrase while the window carries only the newest audio. No
  range rule can bound its alignment, so the finality bar does: whole committed
  spans, bounded by the callback's own length. A textual match further back than
  that bound is out of reach, but a match _inside_ it is still decided by text.
- Live Apple state no longer starts from a production-capable epoch-zero base
  constructor. The recorder-issued epoch is required through the constructor
  chain; only recorder construction/rebind boundaries and test fixtures retain
  honest zero sentinels.
- Steps 2, 5, 7, 8, 9, and 10 were unstarted in that dated snapshot. This is not
  current status and does not authorize restoration of the deleted
  VAD/scheduler pipeline. Current authority is `AcousticLedger`; an independent
  C9 gate, not this documentation cut, owns the W2 closure verdict.

## How a quality HTML must behave

The private quality HTML is a **Seal Atlas**, not a WER table with a banner.

Gold visual: [`docs/quality-reports/seal-atlas.take01.html`](quality-reports/seal-atlas.take01.html)
(take 01 „no to dobra", replay `cff0817b…`, 44100 Hz, 2650112 samples).

HTML handshake (meta, `.stat` cards, lanes): [`docs/quality-reports/CONTRACT.md`](quality-reports/CONTRACT.md).
`codescribe-corpus` writes `quality/seal-atlas.{profile}.html`. Qube is a footnote.

1. **One take, one clock.** X-axis is the capture PCM sample counter. Apple sealed spans, `SealedSpan.words`, Whisper segments, and Silero p(speech) share that axis. No reconstructed timeline.
2. **Production Silero.** Curve from the engine's embedded ONNX + default `VadConfig`, 32 ms chunks — `vad_atlas_probe`, not a toy VAD.
3. **Words from the live dump.** `CODESCRIBE_SEAL_ATLAS_DUMP` after session-end seal. Never rebuilt from the final string.
4. **Two grain classes, labeled.**
   - _word-grain_ — SFSpeech actually returned per-word pins (take 01: spans 3 and 9).
   - _utterance-grain_ — one “word” covering the whole Apple commit-to-commit window. Per-word payload is real where it exists and **not guaranteed**.
5. **Clock-lie** (`clock-lie`) is a first-class finding. Span 2 of take 01: 41 characters pinned to 100 ms (410 chars/s). Physically impossible. Silero can still say “speech 100%” — the range is not the range of that speech. Same class as `seal window unresolved`.
6. **Utterance-grain includes silence tails.** Span 8: 36% speech. The span range is the distance between Apple commits, not the speech outline. This is why W13-3B mints identity from Silero silence edges.
7. **Letters = interpolation.** Grapheme ticks inside a word are an even split of the word range, drawn as such. Not a measurement. Forced-aligner would be required for real grapheme times — we do not pretend to have one.
8. Whisper is drawn as `whisper_words` mapped **back** onto the same clock. HQ / Cloud / AI-formatted stay proposals.
9. `codescribe-corpus` machine JSON stays fail-closed on privacy. The private Atlas HTML is the only place bodies sit next to audio.
10. A WER table may exist as a footnote. It must not replace the atlas and must not present a full-file pass as the live engine.

## Voice Lab handshake

The operator does not open these HTML files from Finder. **Voice Lab**
(`vetcoders-tools` / `voice-lab`) is the console. Tab **Seal Atlas** lists
private HTML under:

`~/.vibecrafted/artifacts/vetcoders/codescribe` (`CODESCRIBE_ARTIFACTS_DIR`)

`codescribe-corpus` must write atlas HTML **into that tree**. Voice Lab then:

1. Classifies by title + relative path (case-insensitive):
   - contains `seal atlas` or `seal-atlas` → kind `seal_atlas` (sorted first)
   - contains `quality` **and** `contract` → kind `quality_contract`
   - else → `quality_report` (the old Qube WER page lands here — not the throne)
2. Pulls fact cards from `<div class="stat"><b>VALUE</b><span>LABEL</span></div>`
   (take 01: 20/20 word-grain, 11 spans, 2 per-word, 1 clock-lie, Silero 0.5).
3. Iframes the file (`sandbox=allow-scripts`). Absolute paths never leave the
   catalog JSON.

A report that fails this handshake is invisible in the lab, whatever its WER.
The gold take 01 HTML already satisfies it. Qube `Codescribe Quality Report`
does not — that title is the thing we are retiring.

## Supervisor findings

Source of truth: `core/quality/supervisor.rs`.
Schema: `codescribe-supervisor-findings/v1`.

Voice Lab three-judge emits a `supervisor` object next to the WER footnote.
Daily is the session document. Candle HQ and cloud `:8444` stay
`HumanTriggeredProposal`. WER is agreement with a proposal, not accuracy.

A finding is only a finding when it names:

- `kind` from the engine catalog
- `claim` that can be false
- `falsifier` — what would disprove it
- `action` — engine cut, lexicon tune, lab-judge hygiene, or operator review

Judge hygiene kinds the lab used to commit (these are P0/P1 when they fire):

- `hq_treated_as_document`
- `cloud_treated_as_document`
- `wer_promoted_to_document_score`
- `omitted_programming_vocabulary`
- `last_session_paired_with_live_overlay`
- `leftover_websocket_polarity`
- `proposal_agreement_misread_as_accuracy`

The catalog also names every engine-side class already in the tree (contract
forbiddens, clock-lie, speech_gap, Teacher attention, confidence flags,
delivery gates, Whisper silence residue). Missing evidence does not invent a
hit. Relative-zero FP starts here: do not crown HQ, do not omit
`vocabulary=programming` on `:8444`, do not pair a live overlay with the
previous `last_session.wav`.
