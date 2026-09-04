# ADR 2026-05-26 — Layered Incremental Transcription Pipeline

> **Historical model reference:** the original ADR named a Q8 Whisper artifact.
> Current runtime policy is FP16-only; resolver, filesystem, direct-engine and
> embedded paths reject quantized Whisper payloads before tensor load.

> **Status:** PROPOSED → ACCEPTED (operator-authored vision, 2026-05-26)
> **Replaces:** Whisper-as-primary live STT model (see `WHISPER_LIVE.md`, `OVERLAY_STREAMING.md`)
> **Owns invariant:** **NEVER REWRITE FROM ZERO.** All layers act incrementally on what was already shown to the user.
> **Trigger:** operator's bench session 2026-05-26 — Apple Dictation latency/UX baseline vs Whisper recall depth.

## Context

Codescribe shipped with Whisper-first live transcription. The benchmark of 7 STT stacks on a 711.5 s
Polish screencast (`~/.codescribe/bench-stt-2026-05-26/`) made two truths simultaneously visible:

1. **Apple Dictation** wins on **live UX** — partial deltas land in the overlay before whole words are
   spoken (neural beam search + on-device Apple Neural Engine), but it degrades the moment the
   speaker mixes a second language or low-probability terminology ("framework vibecrafted", "Bytów do
   New York", animal-clinic terms). It is a "petarda" for the dominant language; a near-deaf wall for
   anything else.
2. **Whisper (large-v3 / turbo-mlx-q8)** wins on **recall depth** — picks up the mixed-language tokens
   and rare terminology, but it works in 30-second chunks with batch latency, and our current chunker
   plus VAD pre-filter in CLI batch mode silently drops segments below confidence threshold.

Neither engine is "wrong". They solve different layers of the same end-user need: **stream pretty
text first, then make it true.**

Independently, the codebase already contains the building blocks for the hybrid the operator has been
describing for weeks:

- `core/stt/apple_stt/mod.rs` — 522 LOC `AppleSpeechAnalyzerAdapter` implementing
  `TranscriptionAdapter`, defaulted through `CODESCRIBE_STT_ENGINE=auto`, with graceful fallback to Candle.
- At decision time, `core/stt/whisper/*` was the production Whisper path
  (embedded turbo-mlx-q8 + Silero VAD).
- `core/audio/streaming_recorder.rs` — chunker emits utterance events **and** the recorder
  always tees a full WAV to disk (`wav_path: PathBuf`, `recorder.rs:203/678`). Full audio is never
  lost, even when the chunker hands out short utterances.
- `core/pipeline/contracts.rs` — `EngineEvent` already has incremental-update vocabulary
  (`Preview { rev, text }`, `Correction { rev, text, previous_text }`, `UtteranceFinal { … }`),
  `TranscriptDelta` carries a backspace-based protocol (`\u{0008}` deletes one tail char).
- `core/vad/silero_ort.rs` + `core/vad/discriminator.rs` — Silero is on; the discriminator already
  classifies utterance boundaries with `utterance_gap_threshold_sec` and `tail_silence_threshold_sec`.
- Lexicon (`stt-engine` route `/audio/lexicon/refresh`, 12597 rules in the libraxis cluster) and a
  small LLM (`programmer` / `Bielik-11B`) are reachable via libraxis cloud or the local mlx-batch
  surface.

What is missing is **orchestration glue** and one new contract event.

## Decision

Adopt a **five-layer incremental transcription pipeline**, with Apple as the live primary engine and
Whisper + lexicon + LLM as background supplements that never overwrite what the user already saw —
they extend, patch in place, and annotate.

```mermaid
flowchart TB
    subgraph CAPTURE["Capture (existing — unchanged)"]
        MIC[🎤 Mic 48k mono] --> SR[StreamingRecorder<br/>tees full WAV always<br/>core/audio/streaming_recorder.rs]
        SR --> CHUNK[Chunker<br/>Silero-driven utterances<br/>core/audio/chunker.rs]
        SR --> WAV[(full WAV<br/>persisted on disk)]
    end

    subgraph L0["Layer 0 — Apple Live (in overlay)"]
        APPLE[SFSpeechAnalyzer<br/>core/stt/apple_stt/mod.rs]
        OVL[Overlay glass tafla<br/>macos/Codescribe/Screens/Overlay/OverlayState.swift]
    end

    subgraph L1[Layer 1 — Whisper Tail Patch]
        TAIL[Tail patcher<br/>NEW core/stt/tail_patcher/<br/>fed by utterance boundary + WAV tail]
        DIFF[Token-level diff vs Layer 0 buffer<br/>fill missing/wrong tokens only]
    end

    subgraph L2[Layer 2 — Lexicon + Small LLM Polish]
        LEX[Lexicon pass<br/>stt-engine /audio/lexicon route<br/>or local copy]
        LLM[Small LLM inline pass<br/>~600-char chunks<br/>type-fixes + sentence/paragraph shaping]
    end

    subgraph L3[Layer 3 — Silero Paralingual Monitor]
        PAUSE["Pause detector → '…'"]
        SOUND["Non-speech classifier<br/>'[śmiech]', '[wiertarka]', '[kasłnięcie]'"]
    end

    subgraph L4["Layer 4 — Final BAM (session-end)"]
        BAM[Whole-session contextual pass<br/>NEW core/pipeline/final_bam.rs<br/>polish + organize within already-shown text]
    end

    CHUNK -. utterance.start .-> APPLE
    APPLE -- partial deltas --> OVL
    APPLE -- utterance.committed --> TAIL

    CHUNK -. utterance audio .-> TAIL
    WAV -. tail samples .-> TAIL
    TAIL --> DIFF
    DIFF -- ReplaceRange events --> OVL

    DIFF -. polished utterance .-> LEX
    LEX --> LLM
    LLM -- ReplaceRange events --> OVL

    CHUNK -. pause/non-speech .-> PAUSE
    CHUNK -. non-speech segment .-> SOUND
    PAUSE -- InsertAnnotation --> OVL
    SOUND -- InsertAnnotation --> OVL

    WAV -. on stop .-> BAM
    BAM -- "ReplaceRange (within bounds)" --> OVL
```

### Layer specifications

**Layer 0 — Apple Live (primary live engine).**

- Activated by default through `CODESCRIBE_STT_ENGINE=auto` on supported macOS, or explicitly via
  `CODESCRIBE_STT_ENGINE=apple`.
- Streams partial recognition tokens to the overlay as fast as `SFSpeechRecognitionTask` emits them.
- Runs entirely on the user's machine; no network, no cost, no recording leaves the device.
- Owns the **first-pass UX promise:** the user sees their words appear as they speak, in the
  dominant language, with near-zero perceived latency. This is the "petarda" baseline operator's
  daily flow has come to depend on.
- Limitation it accepts: mono-language, low-probability tokens get dropped. Those holes are filled
  by Layer 1, never by Layer 0 retrying.

**Layer 1 — Whisper Tail Patch (background supplement).**

- Triggered by `chunker` utterance boundary (Silero-driven) — same boundary Apple's
  `utterance.committed` event would land on.
- New module `core/stt/tail_patcher/` runs Whisper (Candle / mlx-audio / OpenAI cloud — configurable)
  on the **audio slice that produced this utterance**, fed from the recorder's persistent WAV
  tail (no re-capture, no extra mic load).
- Performs **token-level diff** against Layer 0's committed buffer for the same utterance window.
  Emits `EngineEvent::ReplaceRange { utterance_id, start, end, text }` only for the tokens that
  differ — typically: mixed-language inserts, technical terms, rare vocabulary, proper nouns.
- UX: overlay cursor visibly steps back a few words, the patch lands, and the user sees the
  improvement materialise. The operator named this the "magical correction" — it is permitted and
  welcomed precisely because it is bounded and visible.
- **Never** rewrites the whole utterance. If diff distance exceeds a safety threshold, emit
  `EngineEvent::Annotation { kind: TailPatchSkipped, reason }` and leave Layer 0 output unchanged.

**Layer 2 — Lexicon + Small LLM Polish.**

- Runs after Layer 1 settles for a given utterance (small debounce, e.g. 300 ms).
- Two sub-passes:
  - **Lexicon** — applies the project lexicon (compatible with `stt-engine`'s 12597-rule
    `/audio/lexicon/refresh` corpus, or a local subset shipped with codescribe). Word-level
    substitutions, casing fixes, code-term canonicalisation.
  - **Small LLM** — single inline call (~600-char chunks) against a small/cheap model
    (`Bielik-11B` via libraxis cluster, or local `mlx-batch-svetliq`). Two responsibilities:
    1. Type-class fixes (homophones, common Polish typing errors, punctuation Whisper drops).
    2. **Sentence/paragraph shaping** — inserts paragraph breaks, structures lists where the
       speaker enumerated, opens dialogue when the speaker quoted someone. Output stays
       within the same utterance window — the LLM is not allowed to extend.
- Both passes emit `EngineEvent::ReplaceRange` events with the same invariant as Layer 1.

**Layer 3 — Silero Paralingual Monitor.**

- Always-on alongside Layers 0–2; uses the same Silero stream the chunker already drives.
- Two responsibilities:
  - **Pause annotation.** When `discriminator` sees a within-utterance pause longer than
    `paralingual_pause_threshold_sec` (default 1.2 s) but shorter than
    `tail_silence_threshold_sec`, emit `InsertAnnotation { text: "…" }` at the current caret
    position. Distinguishes hesitation from end-of-thought.
  - **Non-speech classification.** Silero already exposes per-window probabilities; a small
    classifier head (new `core/vad/paralingual_classifier.rs`) labels non-speech segments as
    `[śmiech]`, `[kasłnięcie]`, `[wiertarka]`, `[hałas tła]`. Emits `InsertAnnotation { kind: Paralingual, text }`
    at the segment timestamp. **MVP scope:** binary speech-vs-noise classifier first; specific
    labels come from a follow-up dataset (operator's screencast corpus is a starting point).

**Layer 4 — Final BAM (session-end contextual pass).**

- Triggered on `stop()` / hold-release / toggle-stop.
- Runs against the **full session WAV** (recorder always tees one to disk) and the **already-shown
  text buffer** (the union of all Layer 0–3 events).
- Allowed to: polish phrasing, fix cross-utterance references, structure into sections, add
  formatting that only makes sense with the whole picture (e.g. promoting a recurring word into a
  heading, joining split sentences).
- **Forbidden to:** rewrite from scratch, replace text the user has not seen, change the meaning
  of any sentence the user already saw committed. Operates exclusively via `ReplaceRange` events
  inside the bounds of the already-shown buffer.
- Emits a final `EngineEvent::SessionFinalised` so sinks can mark the buffer as immutable.

## Hard invariants

These are non-negotiable. Any layer that violates them is broken and ships back to spec before
landing on `main`.

1. **NEVER REWRITE FROM ZERO.**

   - No layer is allowed to `set_text(new_full_buffer)` after the user has seen anything.
   - The only legal mutations are `Append`, `ReplaceRange { start, end, text }`,
     `InsertAnnotation { position, text }`, and `Backspace { count }` (legacy `TranscriptDelta`).
   - Rationale: the user invested attention in what they read. Wiping and retyping breaks
     trust, breaks copy-paste mid-flow, and breaks the "petarda" promise that made them adopt
     Codescribe instead of Apple Dictation alone. Operator's words: _"tracimy twarz"_.

2. **Layer 0 owns the first commit.** No later layer is allowed to render text before Layer 0 has
   committed an utterance. If Layer 0 is unavailable (no Apple Speech permission, no macOS Speech
   framework), the runtime falls back to Whisper-as-primary (current behaviour) and Layers 1–2
   become no-ops. Detect at startup, log it, expose in `/healthz`.

3. **Bounded patches.** Each `ReplaceRange` must reference an utterance window. Cross-utterance
   replacements are not permitted from Layers 1–3. Layer 4 is the only place allowed to touch
   cross-utterance ranges, and only within the already-shown buffer.

4. **Full WAV always retained until session end.** Recorder must not drop the persistent WAV
   while any layer can still consume it. Cleanup is at `SessionFinalised` + grace window.

5. **No layer reaches outside codescribe.** Layer 1 may call out to mlx-audio / OpenAI / libraxis
   for the Whisper pass; Layer 2 may call the LLM endpoint configured in Settings. But the
   orchestrator owns those calls — no layer hits the network on its own.

## Event contract additions

`core/pipeline/contracts.rs` gains:

```rust
pub enum EngineEvent {
    // … existing variants (Preview, Correction, UtteranceFinal, VadStart, VadEnd, NoSpeech, Drop, Stats) …

    /// Replace a bounded range inside an already-committed utterance.
    /// Emitted by Layers 1, 2, 4 — never by Layer 0.
    ReplaceRange {
        utterance_id: Uuid,
        start: usize,         // char offset within the committed utterance text
        end: usize,           // exclusive
        text: String,
        source: LayerSource,  // TailPatch | Lexicon | InlineLlm | FinalBam
    },

    /// Insert an annotation (paralingual cue or hesitation marker) at a position.
    /// Emitted by Layer 3.
    InsertAnnotation {
        utterance_id: Uuid,
        position: usize,
        text: String,         // "…", "[śmiech]", "[wiertarka]", …
        kind: AnnotationKind,
    },

    /// Mark the session text buffer as immutable. Emitted by Layer 4 on stop().
    SessionFinalised {
        session_id: String,
        layer_summary: LayerSummary,  // counts per-layer for telemetry
    },
}

pub enum LayerSource { TailPatch, Lexicon, InlineLlm, FinalBam }
pub enum AnnotationKind { HesitationPause, Paralingual { label: String } }
```

Sinks must Option-guard new variants (existing pattern); old sinks that ignore them keep working —
they simply show Layer 0 output.

## What is shipped today, what is missing

> **Snapshot of 2026-05-26, kept as the ADR's starting position.** For what the runtime
> actually executes now, read [Phase delivery status (2026-08-08)](#phase-delivery-status-2026-08-08)
> below — that table is the current truth; this one is the baseline it moved from.

| Capability                 | Today                                                               | Needed for layered model                                 |
| -------------------------- | ------------------------------------------------------------------- | -------------------------------------------------------- |
| Apple Speech adapter       | ✅ 522 LOC, `CODESCRIBE_STT_ENGINE=auto` default / `apple` override | Settings toggle                                          |
| Whisper adapter            | ✅ embedded turbo / runtime fallback                                | Background tail-patcher entry point                      |
| Full WAV tee               | ✅ always written                                                   | Lifecycle hook for Layer 4                               |
| Silero VAD + discriminator | ✅ live                                                             | Paralingual classifier head (Layer 3)                    |
| `EngineEvent` vocabulary   | ✅ Preview/Correction/UtteranceFinal                                | + `ReplaceRange`, `InsertAnnotation`, `SessionFinalised` |
| Lexicon                    | ⚠ libraxis-side (`stt-engine`)                                      | Local lexicon module callable from controller            |
| Small LLM call surface     | ✅ libraxis / mlx-batch reachable                                   | Inline polish wrapper with utterance-bounded prompts     |
| Overlay incremental render | ✅ append/backspace via `TranscriptDelta`                           | Add `ReplaceRange` + `InsertAnnotation` paths            |
| Orchestrator               | ❌                                                                  | New `app/controller/layered_orchestrator.rs`             |

**Scope estimate:** ~500–800 LOC net across `app/controller/`, `core/stt/`, `core/pipeline/contracts.rs`,
`core/vad/`. The shape is already in the codebase — this is glue and one new contract event family.

## Migration plan

Four phases. Each ships as an independent machete cut behind a feature flag
(`CODESCRIBE_LAYERED_TRANSCRIPTION=phase{1,2,3,4}`), defaulting to OFF until phase 4 lands.

> **Orthogonality (operator 2026-08-05):** `FINAL_PASS_MODE` / Smart is **stop-path
> full re-pass routing only**. It does not enable this flag. Live gap-fill is Layer 1
> behind `CODESCRIBE_LAYERED_TRANSCRIPTION`. Smart + layered can compose; neither
> silently rewrites the other. Off final-pass never forces Whisper at stop.

**Phase 1 — Layer 0 + Layer 1 (Apple primary + Whisper tail patch).**

- Wire Apple as default engine when available; Whisper-as-primary remains the fallback.
- New `core/stt/tail_patcher/` module + `EngineEvent::ReplaceRange { source: TailPatch }`.
- Overlay gains `ReplaceRange` render path (visible "cursor walks back, patch lands").
- **Wiring status (2026-08-08, W2-A `a6b1233d`):** Layer 1 is live on **both** live paths
  when phase ≥ 1 — the **VAD/scheduler** session (`vad_transcription_session`) and the
  default Apple **progressive** live session (`apple_stream_transcription_session`).
  On the Apple path every sealed `UtteranceFinal` resolves to its retained PCM window
  (W1-B bounded retention) and is handed to an async Layer 1 lane together with the exact
  committed string the `ReplaceRange` offsets are computed against. Bounded queue with
  counted drops (capture is never blocked), at most one job in flight, inference on
  `spawn_blocking`, and every queued engine event flushed before the patch is emitted so a
  `ReplaceRange` can never overtake the `UtteranceFinal` it patches. A boundary that cannot
  address retained audio stays counted as unresolved and is never handed to Whisper.
  The `CODESCRIBE_APPLE_STT_LIVE_MODE=wav` escape hatch remains, but it is no longer needed
  to reach Layer 1.
- Acceptance test: operator's bench audio reproduces — Layer 0 shows Polish live; Layer 1 fills
  "Bytów to New York" + "framework Vibecrafted" + "Hugging Face" within ~1 s of utterance end.

**Phase 2 — Layer 2 (Lexicon + Small LLM polish).**

- Local lexicon module (subset of libraxis 12597 rules, configurable).
- Inline LLM call (`Bielik-11B` default; configurable endpoint).
- `EngineEvent::ReplaceRange { source: Lexicon | InlineLlm }`.
- Acceptance: utterances containing known clinic terms get canonical spelling; rambling
  enumeration gets a list or paragraph break inserted within the same utterance window.

**Phase 3 — Layer 3 (Silero paralingual monitor).**

- Pause-to-`…` first (cheap, deterministic).
- Non-speech classifier follows; ships with binary speech-vs-noise, label set grows as the
  classifier improves. MVP labels gated behind confidence floor.
- `EngineEvent::InsertAnnotation`.

**Phase 4 — Layer 4 (Final BAM).**

- New `core/pipeline/final_bam.rs` runs on `stop()` against full WAV + shown buffer.
- Emits the final batch of `ReplaceRange` events, then `SessionFinalised`.
- Operator's stop trigger (`make install-app` / hotkey release) remains the human control surface.

### Phase delivery status (2026-08-08)

What this ADR proposed vs. what the runtime actually executes today.

**Superseded on the Layer 1 default (2026-08-09 / polarized 2026-08-14).**
`CODESCRIBE_LAYERED_TRANSCRIPTION` unset → `phase1` (`LAYERED_DEFAULT_PHASE = 1`).
Explicit `off`/`0`/`false` is the only disarm. The 2026-08-08 sentence
"`phase1` is still off by default" is historical. Everything **above**
phase1 (Layers 2–4 in this table, plus W13 fusion / idempotence /
highlights) is still off by default.

The 2026-08-08 table below is inventory, not the default. Read the
"Delivered?" column with that amendment.

| Phase                                 | Proposed module                          | Delivered?                               | Where it actually lives                                                                                                                                                                                                                                                               |
| ------------------------------------- | ---------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **1 — Layer 1 tail patch**            | `core/stt/tail_patcher/`                 | ✅ **delivered, default phase1**         | `core/stt/tail_patcher/` exists as proposed; wired into `core/pipeline/streaming/session.rs` (VAD/scheduler) and `core/pipeline/streaming/apple_live_session.rs` (Apple progressive, W2-A `a6b1233d`). Unset env → phase1 since 2026-08-09.                                           |
| **1 — overlay `ReplaceRange` render** | `app/ui/overlay/mod.rs`                  | ✅ delivered, moved                      | `OverlayState.applyReplaceRange` → `OverlayTranscriptSegment.replaceRange` (Swift)                                                                                                                                                                                                    |
| **1 — orchestrator**                  | `app/controller/layered_orchestrator.rs` | ❌ **not built — and not needed so far** | Both live paths call the shared `tail_patch_enabled` / `compute_tail_patch_job` / `emit_tail_patch_result` primitives directly. One gate, one `LayerSummary` shape, no separate state machine. Revisit only when Layers 2–4 need a single audio cursor (see Consequences)             |
| **2 — Lexicon**                       | `core/lexicon/`                          | ⚠️ **partial, different shape**          | No `core/lexicon/` module. Lexicon substitution lives in `core/pipeline/stream_postprocess.rs::apply_lexicon` and runs **at seal time** on the Apple progressive path (W1-A `d180add9`) — as the doctrine's final automated layer, not as a debounced Layer 2 sub-pass                |
| **2 — Inline LLM polish**             | `core/llm/inline_polish.rs`              | ❌ not built                             | No inline per-utterance LLM pass exists. Stop-path AI formatting (`core/llm/ai_formatting.rs`) is a different surface with a different contract                                                                                                                                       |
| **3 — Paralingual monitor**           | `core/vad/paralingual_classifier.rs`     | ❌ not built                             | `EngineEvent::InsertAnnotation` travels end-to-end (contracts → IPC wire → `OverlayState.applyInsertAnnotation`), but the only site that _constructs_ one is a unit test in `app/presentation/emitter.rs`. The transport is ready; nothing produces paralingual annotations           |
| **4 — Final BAM**                     | `core/pipeline/final_bam.rs`             | ❌ not built                             | `SessionFinalised` is emitted (carrying `LayerSummary`) by the live paths, not by a session-end contextual pass. Stop-path re-pass routing is `FINAL_PASS_MODE`, which is a _different_ mechanism — it re-runs Whisper on the full WAV, it does not do bounded cross-utterance polish |

**Reading rule for the phase specs above:** they describe intent, not inventory. A module path
in Phases 2–4 is a proposal until this table marks it delivered.

## Non-goals

- **Not building a live cursor-paste into arbitrary text fields.** Operator explicit:
  _"ja nie muszę mieć wklejane w karetkę, bo pewnie nikt nie zrobi mi backspace + podmianka live
  niezależnie gdzie ta karetka stoi"_. The whole theatre runs **inside the overlay**. Final paste
  to the active field happens once, at session end, after Layer 4 has committed.
- **Not rewriting Whisper from scratch.** This ADR keeps Candle/mlx-audio/OpenAI as
  interchangeable Layer 1 backends; the choice is configuration, not code.
- **Not replacing Apple Dictation system-wide.** Codescribe's overlay is a parallel surface, not a
  Dictation replacement. Apple's framework is one of the engines we orchestrate, not a competitor.
- **Not building bilingual auto-detect.** Layer 0 dominant-language detection follows
  `SFSpeechRecognizer.locale`; Layer 1 fills mixed-language tokens regardless. No language router.

## Consequences

**Positive.**

- User keeps Apple Dictation's perceived speed and gains Whisper's recall depth in the same flow.
- Failures degrade gracefully: Layer 0 down → Whisper-primary; Layer 1 timeout → Layer 0 output
  stands; Layer 2 unreachable → text stays raw; Layer 3 disabled → no annotations; Layer 4
  skipped → session stops at last Layer 2 commit.
- The "NEVER REWRITE" invariant protects the trust contract with the user. Every visible change
  is incremental, bounded, explainable.
- Mixed-language work (operator's vet practice + AI code mix, "Bytów to New York" travel
  bookings, "vibecrafted framework" terminology) becomes a first-class case, not an accident.
- Existing code paths keep working — feature flag means today's users see no change until phase 4.

**Negative / costs.**

- Orchestrator adds non-trivial state in `app/controller/` (per-utterance layer status machine).
- `ReplaceRange` events change the sink contract; legacy sinks that didn't expect them must be
  audited (the codebase has 3 main sinks: overlay, IPC broadcast, telemetry — all Option-guarded).
- Layer 2's LLM call adds latency (~200–800 ms per utterance via libraxis). Default is OFF; user
  opts in via Settings. Local Bielik can run alongside codescribe to remove the latency cost
  but adds RAM pressure.
- Layer 4 + Layer 1 + Layer 2 all want the same audio window — the orchestrator must own
  a single audio cursor, not three independent readers.
- Tail-patcher diff logic is the hardest piece. Wrong diff = visible flicker. Conservative
  threshold (don't patch if uncertain) is the default safe behaviour.

**Operational.**

- Telemetry gains per-layer counters (utterances patched, lexicon hits, LLM calls, annotations
  inserted, BAM edits). Visible in `/healthz` and Quality dashboard.
- Settings gains four toggles (Layer 1, 2, 3, 4) and one engine selector (Apple / Whisper).
- Docs update across `WHISPER_LIVE.md`, `OVERLAY_STREAMING.md`, `ARCHITECTURE.md` — same
  commit as this ADR lands.

## Open questions

- **Should Layer 0 fall back per-utterance or per-session?** If Apple silently degrades mid-session
  (network for cloud-augmented recognition, model swap), do we promote Whisper for the next
  utterance only, or commit to Whisper-primary for the rest of the session?
- **Lexicon source of truth.** libraxis's 12597-rule lexicon lives server-side. Bundling a subset
  with codescribe (under user control) vs. always calling out — what is the privacy default?
- **LLM model for Layer 2.** Bielik-11B is the strongest small Polish model today, but it's 11 B
  params — RAM cost on user machines is real. Smaller fallback (Qwen3-4B?) for resource-constrained
  installs?
- **Paralingual classifier training data.** Operator's bench corpus is 1 file. Need labelled
  laughter / cough / fan / typing dataset before Layer 3 ships labels beyond binary speech-vs-noise.
- **Cross-utterance Layer 4 scope.** How aggressively can BAM restructure? Operator said
  "dokłada coś od siebie" — what is the upper bound of "od siebie"?

## References

- `core/stt/apple_stt/mod.rs:1-522` — Apple adapter implementation
- `core/stt/mod.rs:20-100` — engine selection logic
- `core/audio/streaming_recorder.rs:69, 285-340` — WAV tee + chunker contract
- `core/audio/recorder.rs:203, 559-680` — `wav_path` persistence guarantee
- `core/vad/config.rs:40-69` — Silero VAD parameters (operator-tuned, not vanilla Snakers)
- `core/vad/discriminator.rs:90-150` — utterance segmentation
- `core/pipeline/contracts.rs:346-380` — current `EngineEvent` / `TranscriptionEngineMode`
- `macos/Codescribe/Screens/Overlay/OverlayState.swift` — `applyPreview` / `applyFinal`
  (append path) and `applyReplaceRange` → `OverlayTranscriptSegment.replaceRange`
  (bounded-patch path, returns `false` and drops the patch when the range does not
  address the committed segment). The AppKit `app/ui/overlay/mod.rs` this ADR was written
  against was excised when the UI moved to Swift.
- Bench corpus: `~/.codescribe/bench-stt-2026-05-26/` — 7 stacks compared on operator's audio
- Operator's vision verbatim (this conversation, 2026-05-26): five layers, cursor-back patches,
  paralingual annotations, final BAM, NEVER REWRITE FROM ZERO

---

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_
