# TRANSCRIPT LANES — current execution anatomy

This document is the current structural execution map for Codescribe live
transcription after the C11 source cut (2026-08-25). `484095ce` was the last
executable-code cut before the documentation successor `d57196ab`; the actual
C11 commit is recorded only in its durable implementation report. Compiler,
test, replay, microphone, and release behavior are `NOT_ASSESSED` in C11.

The canonical product contract is [`THE_ENGINE_CONTRACT.md`](THE_ENGINE_CONTRACT.md).
The visual companion for this route is
[`OVERLAY_STREAMING.md`](OVERLAY_STREAMING.md).

## Authority in one paragraph

`RecordingController` is the only in-app microphone owner. `StreamingRecorder`
allocates `capture_epoch` only after a successful physical open.
`transcription_session` dispatches normal live capture only to
`apple_stream_transcription_session`. Apple is the first observer; Whisper may
observe retained PCM as Layer 1, and Lexicon/Light+ plus Responses may relabel
only an already-authorized occurrence. Silero supplies boundary, time, energy,
and pause evidence but owns no text. `AcousticLedger` alone admits and seals
physical occurrences. `PresentationEmitter` / `TranscriptReducer` commit
ledger events; Transcript Bus and Swift observe the resulting projection.
`DeliveryRoute` follows explicit operator intent, never OS focus alone.

## 0. The map at a glance

```text
explicit operator intent
  → RecordingController                         only in-app microphone owner
  → StreamingRecorder                           successful-open capture_epoch
  → transcription_session                       Apple-only live dispatcher
  → apple_stream_transcription_session
      ├─ Apple                                   L0 observation
      ├─ Whisper on retained PCM                 bounded L1 observation
      ├─ Lexicon + Light+                        authorized L2 relabel
      ├─ Responses formatter                     authorized L3 relabel
      └─ Silero                                  time / energy / boundary evidence
  → AcousticLedger::admit / AcousticLedger::seal
  → EngineEvent::LedgerMutation / EngineEvent::LedgerSeal
  → PresentationEmitter / TranscriptReducer
      ├─ committed ledger revision → overlay + transcript_buffer
      └─ ephemeral preview         → overlay only
  → Transcript Bus publish_revision (ledger receipts only)
  → Swift projection observer
  → DeliveryRoute selected from explicit operator intent
```

This is one live route, one capture clock, one occurrence ledger, and one Rust
document reducer. No observer opens another microphone or builds a parallel
transcript.

## 1. Machine layers and sideband evidence

| Layer | Current role | Authority boundary |
| --- | --- | --- |
| L0 — Apple | First live text observer inside the Apple session | Describes PCM-bound occurrences; does not own physical identity |
| L1 — Whisper | Tail-provider observation on retained PCM | May correct the matching authorized occurrence; does not own a parallel live dispatcher |
| L2 — Lexicon + Light+ | Deterministic retained-text relabeling | May relabel an authorized occurrence; equal strings are never identity |
| L3 — Responses formatter | Configured Formatting-lane observation | May format authorized text; may not mint physical speech |

Silero is outside the numbered text layers. Its single session ingress supplies
speech edges, range, energy, and pause evidence to the Apple session. It cannot
admit text, seal a document, or select delivery. `SessionFinalised` is lifecycle
only.

## 2. Normal live capture

| # | Station | Exact current surface | Contract |
| --- | --- | --- | --- |
| A1 | operator intent | `RecordingController::handle_hotkey_event` and mode handlers | Dictation, Agent, and Assistive select downstream behavior, not microphone ownership |
| A2 | physical open | `StreamingRecorder::start_event_session` | compute next epoch with checked arithmetic, open recorder, then assign the epoch |
| A3 | session bind | `StreamingRecorder::bind_session_authority` | bind one operator session to one `AcousticLedger`; reset only the session-local epoch counter |
| A4 | dispatch | `core/pipeline/streaming/session.rs::transcription_session` | delegate only to `apple_stream_transcription_session` |
| A5 | Apple ingress | `apple_stream_worker` | retain PCM, advance the session sample counter, feed the single Silero ingress, and poll the Apple bridge |
| A6 | observation | `seal_utterance_final`, `seal_sliced_by_silero` | bind Apple text to new session-clock PCM; each Silero slice is admitted for its exact range before any raw-final telemetry; Silero contributes evidence, never text authority |
| A7 | admission | `admit_ledger_label` | qualify evidence, schedule the observation frontier, and offer Apple/Whisper/Lexicon/formatter labels to the ledger |
| A8 | physical seal | `AcousticLedger::seal` / `seal_terminal` | close the occurrence only after the scheduled frontier or terminal boundary permits it |
| A9 | document commit | `PresentationEmitter` / `TranscriptReducer` | reduce `LedgerMutation` and `LedgerSeal` into the canonical document |
| A10 | projection | Transcript Bus, then Swift | `publish_revision` publishes and displays the committed reducer projection; raw finals/corrections/patches and previews are not Bus or delivery truth |

### Apple transport A/B

`CODESCRIBE_APPLE_STT_LIVE_MODE` chooses transport inside the same Apple
authority route:

- `stream` uses live Apple AudioBuffer delivery;
- `wav` (and the compatibility spelling `transcribe_live`) uses the older
  Apple temp-WAV request bridge.

The `wav` value changes no microphone, occurrence, reducer, or delivery
authority.

## 3. Layer 1 and explicit Retranscribe

Normal live Whisper work is a bounded Layer 1 observation over retained PCM.
The request and returned segments carry session, successful-open epoch, sample
range, request, and generation identity. A candidate can relabel only the
occurrence authorized by that identity. Replayed observation identity/range is
refused structurally; lexical equality does not merge two disjoint occurrences.

The current provider seam remains `TailProvider` with `inprocess`, `sidecar`,
and `remote` implementations. Product settings decide whether Local Power arms
the live Layer 1 observation. Per-take receipts, not Settings copy, establish
whether work was actually submitted, applied, refused, timed out, or abandoned.

Explicit Retranscribe is different: it is a new operator-authorized inference
over a selected completed artifact. It may decode a whole file, but it is not a
normal-stop continuation, is not live capture, and does not retroactively
become Transcript Bus truth unless the operator accepts its proposal.

## 4. Stop and session finality

Normal stop:

1. closes the open Apple stream or speech epoch;
2. seals any non-empty open partial through the same occurrence path;
3. drains only work already admitted during capture within the bounded budget;
4. seals remaining ledger state and emits the committed projection;
5. emits lifecycle finality; and
6. delivers through the route latched from explicit operator intent.

Normal stop starts no whole-file Whisper pass and no fifth text layer. Legacy
`FINAL_PASS_MODE` spellings remain migration tokens; explicit Retranscribe owns
whole-file inference.

## 5. Responses Formatting lane truth

The runtime settings loader resolves LLM lanes once and seals them in one
`RuntimeSettingsSnapshot`. Consumers use
`RuntimeSettingsSnapshot::llm_lanes()` to read the sealed `RuntimeLlmLanes`
for `main`, `formatting`, and `assistive`; they do not repeat environment,
settings, endpoint, model, provider, or credential resolution during a take.

L3 uses the resolved Formatting lane. Inline formatting describes scheduling
over stable authorized spans; it does not create another model, client, or
transcript reducer. Formatting and Assistive chains remain distinct.

## 6. Projection and delivery

```text
AcousticLedger decisions
  → EngineEvent::LedgerMutation / EngineEvent::LedgerSeal
  → PresentationEmitter / TranscriptReducer
  → Transcript Bus committed projection
  → CsTranscriptProjectionEvent
  → OverlayState.applyTranscriptProjection
  → DeliveryRoute
```

Swift replaces its view from the complete Rust-owned projection. It does not
fold Apple finals, Whisper patches, or lexicon changes into a second text state.
Dictation, Agent, and Assistive share the same transcript authority even when
their delivery destinations differ. Clipboard, paste, canvas, and Agent are
explicitly distinct routes.

`PublishCommittedRevision` is the only emitter-worker command allowed to write
`transcript_buffer`. `PaintEphemeralPreview` only advances the visual delta
baseline. The Bus has no draft or arbitrary-text seal API, and there is no raw-
event delta adapter.

## 7. Settings and runtime truth

| Surface | Meaning |
| --- | --- |
| `CODESCRIBE_ASR_MODE` | product intent: `local_power`, `cloud`, or `apple_only` |
| `CODESCRIBE_LAYERED_TRANSCRIPTION` | compatibility override for live Layer 1 arming; it does not select another dispatcher |
| `CODESCRIBE_APPLE_STT_LIVE_MODE` | Apple bridge transport A/B only |
| `STT_TAIL_PROVIDER` | Layer 1 provider implementation |
| `FINAL_PASS_MODE` / `CODESCRIBE_FINAL_PASS_MODE` | migration-only stop token; no normal-stop file pass |
| `RuntimeSettingsSnapshot::llm_lanes()` | sealed per-take LLM provider/model/endpoint/credential availability |
| `tail_patch_session_receipt` | per-take evidence of live Layer 1 exercise and terminal accounting |

Configured intent, runtime arming, provider exercise, and accepted ledger
mutation are four different facts. No UI toggle alone proves all four.

## 8. Authority laws

- Physical identity is `(session, capture_epoch, sample_start, sample_end)`.
- Observation identity adds producer, request, and generation.
- Equal words on disjoint ranges are distinct occurrences and both survive.
- Re-delivery of one observation identity/range does not create another word.
- Text similarity may align inside an already-authorized occurrence; it never
  establishes or transfers authority.
- Apple is first observer, not immutable text ownership.
- Silero can measure when and where speech occurred; it cannot say what was
  spoken.
- A later authorized observer may relabel the same occurrence before the
  applicable ledger seal.
- No machine layer may create, merge, erase, split, or reorder physical speech
  by comparing strings.
- Transcript Bus accepts only ledger-authenticated reducer revisions; terminal
  ledger seal, not arbitrary text, closes committed Bus truth.
- Swift is a projection observer of the Rust reducer.
- Delivery follows explicit operator intent.

## 9. Required receipts and falsifiers

At minimum, a live evidence report distinguishes:

- selected STT engine and resolved ASR product mode;
- Layer 1 armed/disarmed reason and provider kind;
- observations/windows submitted, completed, applied, refused, timed out, and
  abandoned;
- replay refusals and unanchored observations;
- occurrence admissions and seals;
- transcript seal and delivery route/timestamp; and
- any residue where admitted and projected occurrence counts do not reconcile.

Release falsifiers include intentional repetition on disjoint PCM, replay of
the same observation identity, segment-less Apple finals, clock-lie input,
Layer 1 zero-work while armed, provider failure, bounded stop drain, and
delivery-target mismatch. C8A does not execute these gates.

## 10. Historical field receipts — superseded as implementation maps

The following dated evidence remains useful for regression design but has no
current architectural authority:

| Date | Historical receipt | Current interpretation |
| --- | --- | --- |
| 2026-08-12 | Long Apple takes showed restart/cumulative-final loss and an expensive stop wait | keep the freeze, identity, and bounded-drain falsifiers; do not infer current ownership from the old file layout |
| 2026-08-13/14 | W13 measured Layer 1 starvation, duplicate delivery, capture drift, and full-stop formatting latency | retain the fixtures and performance bars; current occurrence admission belongs to `AcousticLedger` |
| 2026-08-21/22 | Pre-ledger diagrams described `ProgressiveSealMachine`, `progressive_seal.rs`, `StreamPostProcessor`, `stream_postprocess.rs`, a `Whisper-first` `VAD/scheduler` route, and `core/llm/lane_truth.rs` / `lane_truth_snapshot` | all named surfaces are historical or deleted, not current stations; do not restore them from this receipt |

Historical rows and ADRs may preserve the measured facts when they carry an
equally local dated/superseded marker. They may not be cited as an actionable
route on current HEAD.

## 11. Documentation authority and drift control

- [`THE_ENGINE_CONTRACT.md`](THE_ENGINE_CONTRACT.md) owns product invariants.
- This file owns current lane anatomy and must change with authority routing.
- [`OVERLAY_STREAMING.md`](OVERLAY_STREAMING.md) owns the companion live-flow
  visualization.
- [`STT_CONTRACT.md`](STT_CONTRACT.md) owns engine/configuration semantics.
- [`TRANSCRIPT_BUS.md`](TRANSCRIPT_BUS.md) owns committed bus events and path
  resolution.
- [`DELIVERY_ROUTE.md`](DELIVERY_ROUTE.md) owns destination selection.
- [`ENV_REGISTRY.toml`](ENV_REGISTRY.toml) owns supported environment tokens.

When prose and executable behavior differ, establish source/runtime truth and
update the relevant contract in the same coherent cut. Old measurements never
outrank current Git, Loctree, or source bodies.

---

_Vibecrafted with AI Agents by VetCoders (c)2024–2026 LibraxisAI_
