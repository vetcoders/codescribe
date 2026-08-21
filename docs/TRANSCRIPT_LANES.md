# TRANSCRIPT LANES — every transcription path, linearized

**One line per path: where it starts → what it passes through → what it
crosses → what the user sees.** Anchors are `file :: symbol` (symbols survive
the Living Tree better than line numbers). Originally mapped against
`fix/the-tail-patches` HEAD `16e0b9c3`; corrected against
`fix/p0-p2-before-release` HEAD `361ece40`, 2026-08-21.

This file is the execution map. Product invariants live in
`THE_ENGINE_CONTRACT.md`. Where an older row below calls Apple text a floor,
immutable, append-only, or protected, the engine contract supersedes that
wording: **audio time and span identity are stable; text hypotheses are not**.

How to read:

- **LINE** — one transcription path, with stations in order.
- **`[Jn]`** — a junction: the station where lines cross (full list in §9).
- **⚑ OFF** — machinery that exists and is verifier-green but sits behind a
  default-OFF flag (an operator button). Drawn as a dashed line.
- **since** — the commit/wave that put the station into service.

---

## 0. The map at a glance

```
            ┌──────────────────────────── LIVE ────────────────────────────┐
MIC ▶ recorder ▶ [J1 PCM ring+spill]
                   │
   ┌───────────────┴────────────────┐
   │ LINE A (DEFAULT)               │ LINE B (CODESCRIBE_STT_ENGINE=whisper/onnx)
   │ Apple SFSpeech progressive     │ Silero VAD ▶ scheduler ▶ Whisper
   │ partials+finals                │ utterance finals + Refine corrections
   │      ▼                         │      ▼
   │ ProgressiveSealMachine         │ stream_postprocess (lexicon, gates)
   │      ▼                         │      ▼
   └──▶ [J2 CANVAS: reducer+emitter] ◀────┘
              ▲            │
   LINE L1 ───┘            ▼
   (local tail-patch       OVERLAY live text ▶ ...user watches letters land
    currently rides A;
    provider L1 is separate)
                           │ stop
                           ▼
        LINE S: [J3 truth adjudication] ▶ [J4 postprocess+lexicon]
                           ▼
              ┌────────────┴─────────────┐
              ▼                          ▼
        LINE F formatting LLM      LINE G assistive → To Agent
        ([J5 response chain])      (thread rail conversation)
              ▼
        [J6 history files] + [J7 overlay final: Copy/Insert/Revert/Format/To Agent/Auto-Paste]

LINE C (no mic): audio file ▶ cloud or local Whisper ▶ same [J4]→[J6] tail
```

---

## 1. LINE A — Apple progressive live (the instant

**preview** lane)

```
mic ▶ recorder ▶ [J1] ▶ apple_stream_worker PCM ingress ▶ SFSpeech bridge
  ▶ partial callbacks (volatile) + phrase finals ▶ ProgressiveSealMachine
  ▶ seal adjudication + guards ▶ [J2 canvas] ▶ emitter ▶ OVERLAY (live letters)
```

| #   | station       | code                                                                                                      | what happens                                                                                                                                                                        | since                              |
| --- | ------------- | --------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| A1  | capture       | `core/audio/recorder.rs`, `streaming_recorder.rs`                                                         | cpal stream from `AUDIO_INPUT_DEVICE` (native rate/channels) feeds the ring; `CODESCRIBE_AUDIO_SPILL` keeps the FULL take on disk while the RAM ring caps at 300 s                  | spill: W-A retention               |
| A2  | engine choice | `core/stt/mod.rs :: selected_engine`                                                                      | `CODESCRIBE_STT_ENGINE` (`apple` / `whisper` / `onnx`); unset ⇒ Apple when available, otherwise Candle Whisper                                                                      | W11 respec                         |
| A3  | lane routing  | `core/pipeline/streaming/session.rs :: transcription_session`                                             | Apple engine + progressive mode branches into `apple_live_session` BEFORE the VAD path                                                                                              | W2-A                               |
| A4  | PCM ingress   | `apple_live_session.rs :: apple_stream_worker`                                                            | PCM frames go to the bridge; the same frames feed L1 windows — the **sample counter minted here is the session clock** (doctrine §3.2)                                              | W2-A                               |
| A5  | recognition   | `core/stt/apple_stt/` + `codescribe-stt-bridge` (Swift)                                                   | SFSpeechRecognizer streams volatile partials and cumulative phrase finals; pl-PL on-device ~0.24 s; TCC is owned by the app process                                                 | W11                                |
| A6  | sealing       | `progressive_seal.rs :: ProgressiveSealMachine`                                                           | finals seal utterances; the SFSpeech span clock maps onto the PCM clock (2 ms divergence measured); `may_rewrite`/`try_rewrite` is the future time-fence                            | wired W2-B (`8d65f610`/`d64c3876`) |
| A7  | guards        | `apple_live_session.rs` (`phrase final adjudicated`, `novel final suffix rescued`, `freeze open partial`) | cumulative finals are adjudicated against sealed state; novel suffixes are rescued with synthesized windows; restart-retained partials are frozen — the anti-duplication front line | W1-B/W2-A                          |
| A8  | canvas        | **[J2]** `emitter.rs :: TranscriptReducer`                                                                | committed utterances + one active preview; **single writer** `store_transcript_snapshot` (the tick loop only animates)                                                              | `75c89f56`                         |
| A9  | user          | overlay live view                                                                                         | letters land as spoken; later observations may append, fill gaps, or replace weaker text inside the same proven PCM span; rewriting the session from zero remains forbidden         | corrected doctrine 2026-08-21      |

## 2. LINE B — VAD/scheduler live (Whisper-first lane)

```
mic ▶ recorder ▶ [J1] ▶ Silero VAD chunker ▶ utterance boundaries
  ▶ SttScheduler (Fast lane) ▶ Whisper singleton ▶ stream_postprocess
  ▶ [J2 canvas] ▶ emitter ▶ OVERLAY
        ↑ Refine lane: correction.rs partial passes (VAD-aligned windows)
```

| #   | station       | code                                                 | what happens                                                                                                                                                           | since         |
| --- | ------------- | ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| B1  | VAD filter    | `core/audio/chunker.rs` (Silero, embedded, zero-I/O) | detects WORDS, not noise; silence edges close utterances — “fundament stabilności”                                                                                     | doctrine §3.5 |
| B2  | scheduling    | `core/stt/scheduler.rs :: SttScheduler`              | Fast lane = utterance decode; Refine lane = correction re-decodes; per-lane `initial_prompt_for_lane` (⚑ OFF, W13-6A)                                                  | —             |
| B3  | decode        | `core/stt/whisper/singleton.rs`                      | in-process Whisper (turbo fp16 only; official OpenAI tokenizer + pinned mel asset); TTL reaper unloads 30 min after last finished decode (`whisper_residency_reclaim`) | fp16 only     |
| B4  | corrections   | `streaming/correction.rs`                            | Phase-2 Refine: partial passes triggered by finals/speech-ms, **VAD-aligned windows** (`plan_vad_aligned_windows_with_config`) so windows never begin mid-phrase       | W1-A          |
| B5  | postprocess   | `core/pipeline/stream_postprocess.rs`                | lexicon rewrite table (compiled-in seed/programming/operator/protected), hallucination + SemanticGate + empty-drop gates                                               | —             |
| B6  | canvas + user | **[J2]** → overlay                                   | same reducer/emitter contract as LINE A                                                                                                                                | —             |

## 3. LINE L1 — Layer 1 tail-patch

```
[J1 stopped PCM window] ▶ compute_tail_patch_job ▶ TailProvider
  ▶ Whisper re-decode of the window ▶ word-aligned LCS diff
  ▶ ReplaceRange on [J2 canvas]   (gap-fill, never full-replace)
```

| #    | station       | code                                                                                            | what happens                                                                                                                                                                                                                                                     | since                                   |
| ---- | ------------- | ----------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------- |
| L1.1 | gate          | `CODESCRIBE_LAYERED_TRANSCRIPTION` (current default **off**)                                    | temporary fail-closed deployment gate, not the product destination; explicit `phase1` arms the current local mutation lane                                                                                                                                       | current runtime 2026-08-21              |
| L1.2 | window        | Apple: `apple_live_session.rs`; VAD: `session.rs`                                               | Apple progressive carries exact request/span identity through its mutation fence; current VAD/scheduler path explicitly refuses mutation because it lacks the same pending-span fence                                                                            | split truth on `361ece40`               |
| L1.3 | provider seam | `core/stt/tail_provider.rs :: TailProvider`                                                     | typed payload with **integer sample identity** `(session, capture_epoch, sample_start, sample_end)` + evidence + receipts; `STT_TAIL_PROVIDER=inprocess` is the default; `sidecar`/`remote` ⚑ built (W13-2B `4a9fc3fd`), falling back to inprocess with receipts | W13-2A `16ffe025`                       |
| L1.4 | diff + apply  | `core/stt/tail_patcher/`                                                                        | current implementation uses word-aligned LCS and a change-ratio guard; target authority comes from PCM/span identity, never textual similarity alone; accepted corrections apply through one pre-final `ReplaceRange` fence                                      | corrected doctrine 2026-08-21           |
| L1.5 | receipts      | `tail_patch_session_receipt applied=/skipped=/abandoned=` + per-request `tail_provider_receipt` | starvation and bounded-stop abandonment are explicit; a missed drain emits `tail_patch_drain_timeout` before finality                                                                                                                                            | `c3933f42` + 2026-08-21 fail-closed cut |

Field truth, 2026-08-14: ~42% of patches were rejected in Monika's sessions —
the number the ⚑ Silero-fusion flip (§8) exists to fix.

## 4. LINE S — Stop path (every live line terminates here)

```
stop ▶ recorder.stop (drain + WAV) ▶ [J3 truth adjudication]
  ▶ residual-from-partials (live-first) ▶ [J4 postprocess + lexicon]
  ▶ LINE F (format) or LINE G (agent) ▶ [J6 history] + [J7 delivery]
```

| #   | station     | code                                                                 | what happens                                                                                                                                                                | since              |
| --- | ----------- | -------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------ |
| S1  | drain       | `stop_toggle_inner` PHASE 0–4 (`app/controller/mod.rs`)              | serialized stop; `stop_path_budget` log line prices every phase — the budget is sacred (doctrine §3.4)                                                                      | —                  |
| S2  | truth       | **[J3]** `app/controller/truth.rs :: adjudicate_recording_truth`     | ordered audio-span ledger is authority; Apple and provider observations are adjudicated per proven span; no whole-session rebuild                                           | corrected doctrine |
| S3  | residual    | `app/controller/final_pass.rs` + `final_pass_residual_from_partials` | normal product stop performs no hidden whole-file Whisper pass; historical `smart` remains a migration/runtime token; explicit Retranscribe alone owns whole-file inference | 2026-08-21         |
| S4  | postprocess | **[J4]** same `stream_postprocess` gates + lexicon                   | applied to the ADJUDICATED text (`Post-processed transcript … lexicon_rewrites=n`)                                                                                          | —                  |
| S5  | fork        | mode decision (hotkey held)                                          | raw → LINE F (formatting) and/or LINE G (assistive); AUTO format may fire on the overlay                                                                                    | —                  |

## 5. LINE F — Formatting LLM lane

```
raw transcript ▶ ai_formatting (per-lane endpoint/model/key)
  ▶ [J5 Responses chain: previous_response_id per mode]
  ▶ semantic guard ▶ [J7 overlay formatted] + [J6 history]
```

| #   | station    | code                                                                                  | what happens                                                                                                                                                                                                                                                     | since                   |
| --- | ---------- | ------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------- |
| F1  | lane truth | `core/llm/lane_truth.rs`                                                              | endpoint/model/key resolved per lane (Formatting ≠ Assistive ≠ Main — separate key slots, separate chains)                                                                                                                                                       | —                       |
| F2  | request    | `core/llm/ai_formatting.rs :: build_responses_input`                                  | wire contract: `instructions` param on the FIRST turn only; chained turns re-carry the prompt as a leading `developer` item (the chain does NOT persist instructions server-side)                                                                                | `26d0982d` + `5d62aacb` |
| F3  | chain      | **[J5]** `core/state/conversation.rs`                                                 | per-mode `previous_response_id`; the chain is REAL memory (2026-08-14: a stored id answered “what was this about” with a full recall of the take, hours later); ids are org/key-scoped — stale after key rotation ⇒ self-heal drops the id and retries unchained | self-heal in flight     |
| F4  | guard      | `Action quality guardrail` + `semantic_cosine` (`app/controller/quality_delivery.rs`) | divergence (< 0.86) vetoes auto-paste; RAW is always preserved beside the draft                                                                                                                                                                                  | —                       |
| F5  | user       | **[J7]** overlay formatted view                                                       | Copy / Insert→alacritty / **Revert** (armed with the raw first version after AUTO format, `16e0b9c3`) / Format / To Agent; Auto Paste toggle                                                                                                                     | `16e0b9c3`              |

## 6. LINE G — Assistive → To Agent lane

```
dictation ▶ same LINE A/B live ▶ stop ▶ assistive delivery
  ▶ agent thread (rail) ▶ provider (account-auth aware) ▶ reply in thread
```

| #   | station  | code                                   | what happens                                                                                                                                               | since      |
| --- | -------- | -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------- |
| G1  | routing  | `app/controller/assistive_delivery.rs` | dictation goes to the SELECTED thread; a new thread is created only through explicit New thread (routing contract)                                         | contract   |
| G2  | provider | `app/agent/openai_provider.rs`         | Responses + tools + SSE; chained turns re-carry the system prompt as a developer item (same wire contract as F2); account tokens fetched fresh per request | `e591d514` |
| G3  | auth     | `core/llm/account_auth/`               | OAuth sign-in verifies `api.responses.write` BEFORE saving (“connected” is never a lie); scope-starved tokens are rejected at login                        | `230443fc` |
| G4  | user     | thread rail conversation               | agent replies land in the thread; permission gate is risk-based, and native side-effectful tools fail closed in the voice lane                             | —          |

## 7. LINE C — File/cloud mode (no microphone)

```
audio file ▶ `codescribe transcribe` CLI / cloud final pass
  ▶ local Whisper full-file OR multipart STT endpoint (STT_ENDPOINT)
  ▶ [J4 postprocess] ▶ stdout / files [J6]
```

- Local full-file: same in-app Whisper, proper boundaries ⇒ **API-class
  output** (the decisive A/B/C proof — the machine and model were never the
  bottleneck; window feeding was).
- Cloud: `core/llm/client.rs` multipart (`[Multipart STT]` retries), also the
  target class of `STT_TAIL_PROVIDER=remote`.
- Bench ruler: the same-host `lbrx-stt-engine` column (§13 of the roadmap) —
  its timestamps compress silence (clock-lie class), so any integration maps
  by sample ranges, never by its reported seconds.
- `streaming/offline.rs` is **tests/offline_eval only** — not a runtime lane.

## 8. Dashed lines — built, verifier-green, ⚑ default-OFF (operator buttons)

| flag                                    | line it arms                                                                                                                             | code                                                    | evidence                                                          |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- | ----------------------------------------------------------------- |
| `CODESCRIBE_INLINE_FORMAT`              | W13-1 buforek: sealed spans stream to the formatter DURING dictation (`previous_response_id` chain per span), so stop pays only the tail | `core/llm/inline_format.rs`                             | seam stop 0.398 s vs 8.6–13.8 s; needs live ≥60 s take            |
| `CODESCRIBE_SILERO_FUSION`              | Silero boundary identity + conservative fusion feeding L1 windows; default OFF                                                           | `streaming/silero_fusion.rs`                            | synthetic starvation skips 18→6 (−67%); take-614 A/B still owed   |
| `CODESCRIBE_SPAN_IDEMPOTENCE`           | ledger-keyed replay rejection — a sealed span cannot be delivered twice (kills gap-append “×4” dupes; NEVER content similarity)          | `streaming/span_idempotence.rs`                         | named repetition tests green; real-session receipts pending       |
| `CODESCRIBE_OVERLAY_HIGHLIGHTS`         | lexicon-corrected words + VAD-speech-no-words gaps marked on canvas; highlighted span → Teach                                            | W13-6B (bridge + Swift)                                 | Rust+Swift tests green                                            |
| `CODESCRIBE_STT_INITIAL_PROMPT_ENABLED` | lexicon VOICE: `Vocabulary:` prompt to Whisper per window                                                                                | `stream_postprocess.rs :: build_whisper_initial_prompt` | A/B: U-WER −1.5 pp but false inserts 1→7 — flip not justified yet |
| `STT_TAIL_PROVIDER=sidecar\|remote`     | tail decode out of process / off host                                                                                                    | `tail_provider.rs` + `codescribe-stt-sidecar`           | fake-provider receipt 22 ms; production supervision unmeasured    |

## 9. Junctions — where lines cross

| J   | place                                                                            | who meets whom                                                       | contract                                                                                                                     |
| --- | -------------------------------------------------------------------------------- | -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| J1  | PCM ring + spill (`recorder`/`live_audio_buffer`)                                | mic capture × Apple ingress × L1 windows × stop WAV × crash recovery | the sample counter minted here is the ONE session clock                                                                      |
| J2  | canvas (`TranscriptReducer` + `BufferedEmitter` + `app/presentation/emitter.rs`) | Apple finals × L1 `ReplaceRange` × gap-appends × preview             | ONE writer; order and PCM identity are stable, while text inside an authorized unsealed span may be corrected                |
| J3  | truth adjudication (`truth.rs`)                                                  | live canvas × Whisper residual                                       | preserve the ordered span ledger; accept only evidence bound to its PCM range; never rebuild the session from zero           |
| J4  | postprocess (`stream_postprocess`)                                               | every text × lexicon × gates                                         | lexicon is the FINAL automated layer; the human layer stays on top                                                           |
| J5  | Responses chain (`state/conversation.rs`)                                        | formatting turns × assistive turns (separate streams)                | per-mode ids; first-turn `instructions`, chained developer item; chain = recoverable session memory                          |
| J6  | history (`core/state/history.rs`)                                                | every take                                                           | `_raw.txt` + `_formatted.txt` (or `formatting-failed`) + `.m4a` + `.truth.json` — content is never destroyed (doctrine §3.7) |
| J7  | overlay delivery (`overlay_paste.rs`, OverlayState.swift)                        | formatted draft × auto-paste × manual commit                         | semantic guard vetoes auto-paste only; Revert holds the raw first version                                                    |

## 10. What the user sees, surface by surface

| surface       | fed by                         | truth it shows                                                                                                                       |
| ------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| overlay LIVE  | LINE A/B via J2                | letters as spoken; live backspace corrections may replace weaker text inside the same proven span; session-wide rewrite is forbidden |
| overlay FINAL | LINE S→F via J7                | formatted draft + buttons (Copy / Insert / Revert / Format / To Agent); Auto Paste when guard allows                                 |
| paste target  | J7                             | formatted text into the latched foreign app; refuse (`CopyTargetUnavailable` / mismatch) ⇒ Paste Here slot, user clipboard untouched |
| thread rail   | LINE G                         | agent conversation, chained turn by turn                                                                                             |
| history dir   | J6                             | `~/.codescribe/transcriptions/<date>/` — raw, formatted, m4a, truth receipts                                                         |
| menu/tray     | controller state               | recording state; Audio truth section (W13-5: device, level, quality verdict)                                                         |
| warnings      | `contracts.rs` warning classes | ONLY `transcription_failed` is terminal/UI; receipts (capture level, tail patch, seal) are log-only                                  |

---

## 11. Superseding lane laws — 2026-08-21

These laws are normative. They replace stale `immutable floor`, `never delete`,
and `append-only text` interpretations wherever those appear in old comments,
tests, reports, or commits.

### 11.1 Authority

- PCM samples and their monotonic capture clock are primary evidence.
- A transcript token is an observation, not evidence by itself.
- Apple is the first low-latency observer.
- Whisper is a slower contextual observer.
- Lexicon is deterministic domain evidence.
- Formatting is a presentation transform, not speech evidence.
- Human edits have final authority after session seal.
- No model owns the document.
- No earlier model owns a word merely because it emitted first.
- No later model may mutate a span it cannot identify.

### 11.2 Canvas geometry

- Every accepted observation names a session.
- Every accepted observation names a capture epoch.
- Every accepted observation names `[sample_start, sample_end)`.
- Replayed delivery of the same identity is idempotent.
- Identical words on different spans are intentional until proven otherwise.
- Span order follows PCM time and cannot be reordered.
- Text inside an authorized span may evolve before `transcript_sealed`.
- A correction may replace inflection, morphology, spelling, or a whole phrase.
- A correction may remove a hallucinated word from its own span.
- A correction may add speech missing from the current canvas.
- A correction may not steal words from a neighboring span.
- A correction may not erase an uncovered region of speech.
- A correction may not build the entire session from zero.

### 11.3 Window cadence

- The target Whisper observation cadence is about four seconds.
- Adjacent windows overlap by about one second.
- Window edges must respect available speech-boundary evidence.
- Overlap exists to recover context, not to duplicate text.
- Overlap replay is resolved by request/span identity.
- Text similarity may help alignment after identity is established.
- Text similarity may never mint identity.
- VAD edges are evidence about speech activity, not transcript authority.
- Apple commit timing is evidence, not a perfect word boundary.
- Clock-lie or unresolved windows fail closed and emit receipts.

### 11.4 Layer 1 deployment truth

- The product destination is Apple-first with continuous Whisper refinement.
- `Layered off` is a temporary deployment state, not the vision.
- `phase1` is not allowed to mean a decorative UI toggle.
- Settings ON requires an armed runtime lane.
- Settings ON with zero submitted windows is a product failure.
- Settings OFF may never be inferred from model unavailability silently.
- Missing FP16 produces named degradation.
- Q8 is never a fallback.
- Cloud and local providers must implement the same observation semantics.
- Provider differences may affect transport and latency only.
- Provider choice may not change canvas authority.

### 11.5 Current implementation split

- Apple progressive has request identity and span maps on HEAD `361ece40`.
- Apple progressive has one pre-final rewrite fence.
- Apple progressive tracks structural replay identity.
- Apple progressive exposes applied/skipped/abandoned receipts.
- VAD/scheduler does not have the same pending-span rewrite fence.
- VAD/scheduler therefore preserves primary text and emits degradation.
- The old claim “L1 rides both paths identically” is false on this HEAD.
- The old claim “both paths are production-equivalent” is false on this HEAD.
- Default-off remains justified only for uncovered mutation paths.
- Default-off is not justification for disabling the safe covered path silently.

### 11.6 Stop semantics

- Releasing Fn closes capture.
- It does not authorize a whole-file rewrite.
- Admitted patch work receives a bounded drain opportunity.
- Work that cannot land before finality is counted as abandoned.
- Abandonment emits a named degradation receipt.
- The live result survives provider failure.
- Explicit Retranscribe reads the saved audio as a new user action.
- Retranscribe produces a proposal/result outside normal-stop authority.
- Final BAM, when built, operates on the ledger and bounded spans.
- Final BAM is not hated Full Final Pass under a new name.

### 11.7 Receipt minimum

- session id
- capture epoch
- sample range
- provider request identity
- provider kind
- source transcript hypothesis
- candidate transcript hypothesis
- alignment evidence
- accepted operation type
- accepted character/token range
- affected span identities
- rejection or degradation reason
- queue admission time
- inference completion time
- fence application time
- final disposition: applied, skipped, replayed, abandoned, or timed out

### 11.8 Release falsifiers

- A take where Apple loses a meaningful phrase must be repaired before Delivery.
- The canonical phrase `Whisper musi łatać partiale` must survive the live path.
- Manual Retranscribe recovering meaning that Delivery lost is a failed live run.
- Layered ON with `tail_patch_replacements=0`, `refusals=0`, and no submitted window is a failed arming test.
- A first token such as `IWO` lost before seal is an onset/pre-roll failure.
- Five intentional repetitions on distinct spans must remain five repetitions.
- Replaying the same span identity must not create a duplicate.
- An invalid/Q8 model must be refused before tensor load.
- A corrupt model artifact must not become permanently complete.
- A valid alternate weights file must not be shadowed by an invalid preferred name.
- A valid older HF snapshot must not be shadowed by an invalid newer snapshot.
- Malformed safetensors metadata must fail discovery before engine load.

## 12. Documentation authority and drift control

- `THE_ENGINE_CONTRACT.md` defines product invariants.
- This file defines the executable lane map.
- `STT_CONTRACT.md` defines engine/configuration behavior.
- `ENV_REGISTRY.toml` defines supported environment keys.
- Runtime logs and Transcript Bus receipts prove actual execution.
- Historical reports are evidence, never silent authority.
- A comment marked `operator law` requires a direct operator decision reference.
- Agent interpretations may not be attributed to Maciej or Monika.
- Superseded rules remain searchable but must be labeled superseded.
- Tests for superseded behavior must be rewritten or deleted.
- Green tests for the wrong contract are regressions, not reassurance.
- Every contract-changing patch updates code, tests, settings, UI, and docs together.

_Provenance: distilled from `docs/THE_ENGINE_ROADMAP.md`, the W13 settlement
ledger, `/Users/maciejgad/Downloads/Kora_codescribe.md`, and structural/runtime
verification on `fix/p0-p2-before-release@361ece40`, 2026-08-21._

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders (c)2024-2026 LibraxisAI
