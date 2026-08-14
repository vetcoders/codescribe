# THE ENGINE ROADMAP

**Codescribe STT engine — current state vs. target, sealed.**

|           |                                                                                                                                             |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Status    | SEALED — direction decided by the operator; execution tracked per cut                                                                       |
| Date      | 2026-08-13                                                                                                                                  |
| Plan pack | `~/.vibecrafted/artifacts/vetcoders/codescribe/2026_0813/plans/w13-tail-and-format/` (ATLAS + 6 briefs + DRIVER + de-risk recon)            |
| Branch    | `fix/the-tail-patches` (Living Tree)                                                                                                        |
| Evidence  | Every claim in this document is backed by a measurement or a `file:line` citation from the 2026-08-12/13 sessions. No aspirational numbers. |

---

## 1. Introduction

This document exists because the design was ahead of the runtime — and the
gap kept being re-derived instead of closed.

The layered transcription model (fast on-device words + deep-context
correction + lexicon + AI formatting) was designed by the operator and
repeated, in his own count, ~50 times. Every element of it exists in this
repository. Almost none of it is connected the way the design draws it.
The system today is a set of healthy organs that are not wired into one
bloodstream.

This roadmap freezes three things:

1. **The doctrine** — the decisions that are settled and must not be
   re-litigated by future sessions or agents (§3).
2. **The gap** — current runtime state vs. target state, side by side,
   with evidence (§5).
3. **The work** — every implementation point of every cut, enumerated,
   with its non-fakeable acceptance measurement (§6).

If you are an agent entering this repo to work on the engine: read §3
before proposing anything. The direction questions are closed.

## 2. Executive summary

**One organizing idea:** the canvas's primary key changes from _token
position_ to **TIME**. One session clock (the PCM sample counter), words
pinned to seconds, utterances bounded by Silero silence edges, and the
transcript maintained as an append-only ledger of sealed (immutable)
utterances. Everything else in this roadmap is a consequence of that
inversion.

**Two engines, equal and complementary — this is settled.** Apple
SFSpeech delivers certain words instantly; Whisper supplements and deepens
them in flight with what needs wider context. History proves neither can
work alone: today Apple-alone starves words on degraded input; in early
2026 Whisper-alone produced hallucination and repetition storms on this
same engine. The layered model is not a compromise between them — it is
the invention that fuses them. The wave's goal is to FINISH the fusion,
not to crown either engine.

**Six cuts (W13-1 … W13-6)** deliver the finish:

| Cut   | One line                                                                                                                                  | State             |
| ----- | ----------------------------------------------------------------------------------------------------------------------------------------- | ----------------- |
| W13-1 | Inline-format buffer: sealed chunks stream to the formatting LLM during dictation (`previous_response_id` chain); stop pays only the tail | `[~]` in progress |
| W13-2 | Tail-patch behind a provider seam: local ws sidecar (default target), remote opt-in, in-process fallback                                  | `[ ]`             |
| W13-3 | **Keystone**: time-pinned canvas — Silero-bounded utterances, words pinned to seconds, sealed ledger                                      | `[ ]`             |
| W13-4 | Gap-append dedup by time-span (shrinks to a corollary of W13-3) + in-span hallucination fence                                             | `[ ]`             |
| W13-5 | Capture-level receipt + Audio menu truth (level, device, quality)                                                                         | `[ ]`             |
| W13-6 | Lexicon gets a voice (Whisper `initial_prompt`, Apple `contextualStrings`) + word/gap highlighting feeding Teach                          | `[ ]`             |

**What the user feels when this lands:** words stop vanishing; corrections
actually arrive; stop is near-instant; deliberate repetition is never
eaten; duplicated fragments disappear; the transcript shows what was
corrected and where speech was lost; and the engine hears project
vocabulary before it errs instead of being spell-checked after.

## 3. Doctrine (settled — do not re-litigate)

1. **Both engines are equal.** Apple = certain words now. Whisper = deeper
   context in flight. Any proposal shaped "make X primary and demote Y"
   is wrong by construction. (Operator, 2026-08-13, correcting BOTH
   directions of pendulum swing.)
2. **Time is the primary key.** PCM sample counter is the session clock;
   SFSpeech span clock is mapped onto it explicitly at ingestion (a 2 ms
   divergence is measured and documented at `progressive_seal.rs:360–373`).
3. **Append-only overlay, layer order preserved:** Apple → Whisper →
   lexicon → human. Sealed spans are immutable; the human layer stays on
   top after seal.
4. **Live-first; the stop path budget is sacred.** Work happens during
   dictation; stop pays only for the unsealed tail.
5. **Silero is a filter, not a microphone.** It detects words, not noise;
   its silence edges define utterance identity. ("Fundament stabilności"
   — operator, 2026-08-13.)
6. **The lexicon has a voice, not only an eraser.** Vocabulary reaches the
   decoders _before_ they err (initial prompt / contextual strings);
   post-hoc rewrite remains as the second line.
7. **Content is never destroyed.** A failure with a non-empty draft ends
   the session and keeps the transcript (the 282-characters incident rule,
   generalized on 2026-08-13 by `8bc1cc37`).
8. **Default flips, DMG publication, pushes and merges are operator
   buttons.** Agents deliver measurements for those decisions, never press
   them.

## 4. Evidence base (why this roadmap is shaped like this)

All measured 2026-08-12/13 unless noted.

- **The decisive A/B/C** (same take `2026-08-13/171939`, RMS −42.1 dB —
  degraded input): (A) live canvas: word salad — "maszynę", hallucinated
  "Dziękuję", "RIPOS", "Edyta", a 3× duplicated fragment; (B) the SAME
  in-app local Whisper (turbo fp16) on the whole file with proper
  boundaries: API-class output, every sentence intact; (C)
  whisper-v3-large via api.libraxis.cloud: same class as B. **Machine,
  model and resources are not the bottleneck; role assignment and window
  feeding are.**
- **Tail-patch lane history:** a month of 116 rejected / 0 applied
  (starvation, invisible until the session receipt was added, `c3933f42`);
  after the word-aligned LCS fix (`f224effd`) the lane applies 32/12/24
  per take but still logs up to 18 skips/take — the mid-phrase-window
  class.
- **External window cost:** a 3 s window through
  `/v1/audio/transcriptions` = **0.34–0.38 s** total; local model is
  native fp16 (dequantize 0.00 s), cold load 3.9 s after the 300 s TTL
  reaper; warm RTF ≈ 0.06. The historical "Whisper = 20–30 s" that
  justified demoting it was a one-time Metal-compile misdiagnosis.
- **Stop cost today:** full-text LLM format at stop = 8.6–13.8 s; nano
  via Responses streaming formats the same text in 8.5 s total with
  ~1–2 s chunks, and `previous_response_id` chaining works on both
  OpenAI and api.libraxis.cloud endpoints.
- **Capture drift:** full corpus (662 takes): weekly median RMS broke in
  week 30 (Jul 20–26: −38.3 → −43.9 dB) and slid to −46.5 by W33; monthly
  SNR 16.7 → 10.3 dB with a rock-stable noise floor. Invisible for three
  weeks because only per-event logs existed — the same telemetry class as
  the 116/0 starvation.
- **Duplication vs. theft (both measured on the operator's canvas):**
  gap-appends double text when the engine later re-delivers the same
  span; naive content dedup ATE a deliberately repeated sentence
  ("wpierdalało = zabierało"). Scope by time, never by similarity.
- **Early-2026 mirror disease:** Whisper-alone produced
  over-hallucination and repetition storms (window overlap, decode
  derailment — see `whisper-window-alignment-fragility` incident record).

## 5. Current state vs. target — the master table

| Subsystem               | Current (evidence)                                                                                                                                                                                                                                                                                                                                          | Target (cut)                                                                                                                                                                                                             |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Utterance identity      | Apple phrase boundaries; ids minted on `PhraseFinal`/frozen partials (`seal_utterance_final`, apple_live_session.rs:1040)                                                                                                                                                                                                                                   | Silero silence edges on the PCM clock mint ids; Apple cumulative finals sliced by time (W13-3)                                                                                                                           |
| Word timestamps         | Apple per-word segments cross the bridge into `EngineEvent::UtteranceFinal.segments` — then die outbound in `CsEventSink::on_event` `..` destructure (recording.rs:686–707); Whisper segments dropped at `compute_tail_patch_job` (session.rs:258 takes `.text` only)                                                                                       | Words pinned to spans end-to-end; segments survive into the ledger and the UI (W13-3, W13-6 highlighting)                                                                                                                |
| Sealing                 | `ProgressiveSealMachine` IS production seal authority on the Apple lane (`AppleSealState.progressive`, wired `8d65f610`/`d64c3876`) — but `SealedSpan` has end-only time, no per-word payload; the `try_rewrite` fence has zero callers (patches bypass it via `ReplaceRange`)                                                                              | Ledger of sealed spans with `[start,end)` + per-word payload; ALL rewrites go through the fence; sealed span = immutable (W13-3)                                                                                         |
| Whisper windows         | 3 s shards mid-phrase; `extract_speech` compaction corrupts segment timebase (vad/mod.rs:99–123); `TailPatchRequest` lacks window start                                                                                                                                                                                                                     | Utterance-bounded windows cut at Silero edges, exact offsets by construction, timestamps preserved (W13-2 payloads + W13-3)                                                                                              |
| Replacement authority   | Small-edit floors + conservative gates veto Whisper's better truth (18 skips/take)                                                                                                                                                                                                                                                                          | Per-word fusion by span overlap inside the unsealed utterance; corrections land; skips carry a reason code (W13-3)                                                                                                       |
| Duplicates / repetition | Gap-append doubles raw; dedup once ate deliberate 5× repetition                                                                                                                                                                                                                                                                                             | Sealed span cannot be re-delivered (structural); deliberate repetition = different span, always survives; in-span loop fence for engine hallucinations (W13-4)                                                           |
| Whisper hosting         | In-process; RAM/battery in app; cold 3.9 s after TTL                                                                                                                                                                                                                                                                                                        | Provider seam: local ws sidecar default target (qube-ws pattern — ends the SIGPIPE class), remote opt-in via STT_API_KEY slot, in-process fallback; per-window latency receipts (W13-2)                                  |
| AI formatting           | One-shot full text at stop: 8.6–13.8 s; `LayerSource::InlineLlm` exists unwired                                                                                                                                                                                                                                                                             | Sealed chunks stream to the LLM during dictation; `previous_response_id` chain; stop formats only the tail and closes coherently with full-chain context; fail-open per chunk; anti-invention guard (W13-1)              |
| Lexicon                 | seed.jsonl (2401) + programming.jsonl (155) compiled in and applied ONLY as post-hoc whole-word rewrites on enumerated variants — loses to generative mangling; `build_whisper_initial_prompt` ("Vocabulary:", 224-token budget) fully wired into the scheduler and file mode but **default OFF** (`loader.rs:2296`) and drawing from protected+custom only | The voice ON (operator flips with WER A/B numbers in hand), budget-aware selection incl. domain picks; Apple `contextualStrings` recon/wire; corrections + speech-gaps highlighted on canvas, one click to Teach (W13-6) |
| Capture telemetry       | None (drift found 3 weeks late by archaeology)                                                                                                                                                                                                                                                                                                              | `capture_level_receipt` per session (median RMS + device), WARN below floor registered as NON-terminal, Audio menu section with input truth (W13-5)                                                                      |
| Failure UX              | Fixed 2026-08-13 (`8bc1cc37`): terminal failure with draft ends session, keeps transcript, honest toast                                                                                                                                                                                                                                                     | Keep; regression-pinned                                                                                                                                                                                                  |

## 6. The cuts — every implementation point

State alphabet: `[ ]` todo · `[~]` running · `[?]` done-unverified ·
`[!]` blocked · `[x]` verifier-green. **Only the delivery-verifier flips
`[~]`→`[x]`; an agent's claim never does.**

### W13-1 — Inline-format buffer (Backspace Magic) — `[~]`

Current: formatting is a single full-text LLM pass at stop (8.6–13.8 s
measured). `LayerSource::InlineLlm` exists with no producer.

Implementation points:

1. Sealed sentence/segment triggers an async format request for that chunk
   while recording continues; results keyed by chunk span.
2. Chunks chain via `previous_response_id` (Responses API; both OpenAI and
   api.libraxis.cloud proven); chain resets per session.
3. Stop composes formatted chunks + freshly formatted tail only; the final
   link closes coherently with full-chain context.
4. Fail-open per chunk: LLM error/timeout ⇒ raw text + receipt; session
   never blocked.
5. Anti-invention guard: a formatted chunk whose word-set materially
   exceeds its input is rejected (raw kept + receipt) — the formatter may
   punctuate and case, never add words (a content-adding formatter was
   observed live 2026-08-13).
6. Feature flag, default OFF (operator button).
7. Reuse the existing formatter LLM client — no new HTTP client.

Verifier: on a ≥60 s dictation with the flag ON, measured stop-to-paste
< 3 s and output equal to full-text formatting modulo chunk-boundary
punctuation; receipts show chunks formatted in flight.

### W13-2 — Tail-patch provider seam (sidecar / remote / in-process) — `[ ]`

Current: one in-process path behind `whisper_tail_patch_transcribe`
(core/stt/mod.rs); STT_API_KEY slot reports "Unsupported" in
key_liveness; cold 3.9 s after TTL inside the app process.

Implementation points:

1. One seam, three incarnations selected by config
   (`STT_TAIL_PROVIDER=sidecar|remote|inprocess`); no call-site branching.
2. Local sidecar: ws transport (qube-ws pattern from vista-kernel — ws
   ends the pipe/SIGPIPE class), localhost only, spawned and supervised by
   the app; PCM window in → text + **timestamped segments** out; model
   loading reuses the fp16 path. Sidecar never touches the mic (PCM over
   ws) — verify the TCC-disclaim class does not apply.
3. Remote: multipart `/v1/audio/transcriptions` client, localhost-first
   URL resolution, STT_API_KEY slot made supported; never a hard
   dependency. Measured: 0.34–0.38 s per 3 s window.
4. Fallback: sidecar dead/unreachable ⇒ in-process takes the window with a
   receipt; the lane never silently starves (the `c3933f42` receipts keep
   working).
5. Per-window receipt: provider + elapsed ms.
6. Local-only remains a first-class product mode (operator hard
   requirement).

Verifier: offline replay per incarnation; sidecar run shows per-window
latency < 1 s and applied > 0; killing the sidecar mid-take completes the
take via fallback with receipts proving the switch.

### W13-3 — Time-pinned canvas (KEYSTONE) — `[ ]`

Current: token position is the primary key; time is dropped at three
located points (see §5 rows 2–5). De-risk recon (evidence-grade, in the
plan pack: `recon-w13-3-derisk.md`) settles the build-vs-reuse questions.

Implementation points:

1. **One clock:** PCM sample counter as session timeline; map the SFSpeech
   span clock at ingestion (divergence documented at
   progressive_seal.rs:360–373). Silero edges computed on the PCM counter.
2. **Silero-bounded utterances on the Apple lane:** feed the existing
   Supervisor-mode boundary machine (`VadGateMode::Supervisor` +
   `VadIterState`, chunker.rs:119/:1400, embedded model) at the
   `apple_stream_worker` PCM ingress (apple_live_session.rs:1114–1135);
   mint utterance ids from silence edges in `seal_utterance_final`
   (:1040); slice Apple's cumulative finals by time using their
   `TranscriptSegment` spans. **Hardest part — plan for it explicitly:**
   Apple's restart/freeze/novel-suffix guards (:1225, :940–1005) assume
   Apple's own boundaries.
3. **Extend, don't rewrite, the seal machinery:** `ProgressiveSealMachine`
   is the production authority — add span start + per-word payload to
   `SealedSpan` (additive); wire ALL patch writes through the currently
   orphaned `may_rewrite`/`try_rewrite` fence; port the idempotence rules
   of the dormant `SessionIngest` ledger (typed `AudioRange`,
   `RejectedSealedUtterance`) or re-parent it to the local lanes.
4. **Thread Whisper timestamps:** carry `RawTranscript.segments` through
   `compute_tail_patch_job` (today session.rs:258 takes `.text` only); add
   window start to `TailPatchRequest`; either skip `extract_speech`
   compaction for anchored windows or emit a kept-window index map so
   timestamps survive it.
5. **Per-word fusion at seal:** inside one utterance span, match by span
   overlap + normalized word key; agreements confirm; disagreements
   resolve by evidence (confidence, degraded-input bias measured in §4);
   lexicon applies once to the fused text; then SEAL.
6. Skip receipts gain a reason field (`head_garbage`, `no_time_overlap`,
   `low_confidence`, …).
7. **First execution step:** replay a real pl-PL take and histogram Apple
   per-word span sanity — only synthetic timestamp fixtures exist today
   (live_stream.rs:433–448).

Verifier: the starved fixture (18 skips on build 614) re-run: skip count
down ≥50% with applied same-or-higher and zero regressions in existing
tail-patcher tests; sealed ledger replay shows immutable spans and
committed text trailing live preview by ≤1 s.

### W13-4 — Duplicates die structurally; hallucinations get a fence — `[ ]`

Current: gap-appends double raw text; content-similarity dedup is banned
(it stole deliberate repetition).

Implementation points (post-W13-3 re-scope — most of the original cut
falls out of the ledger):

1. Sealed span cannot be re-delivered: incoming text overlapping a sealed
   span is suppressed once with receipt `gap_append_superseded` —
   append-only holds (drop the duplicate, never rewrite the canvas).
2. Deliberate repetition = different time span ⇒ passes untouched, always.
3. In-span engine-loop fence: repetition-loop detection
   (`has_repetition_loop` candidate from qube-ws) flags and truncates
   engine-hallucinated loops _within_ one span; receipt-only, no silent
   drops.
4. Two regression fixtures decide: the operator's duplicated-canvas take
   (segment appears exactly once) and a deliberate 5× repetition take
   (all five present).

Verifier: both fixtures green, archived.

### W13-5 — Capture truth (receipt + Audio menu) — `[ ]`

Current: zero aggregate capture telemetry; the W30 break was found three
weeks late; the take that produced the decisive A/B/C ran at −42 dB
unnoticed.

Implementation points:

1. Running RMS accumulated per buffer on the capture path (follow the
   `AUDIO_INPUT_DEVICE` env contract, 4 files).
2. `capture_level_receipt` at finalization next to the tail-patch session
   receipt: median RMS dB, peak dB, device, sample rate, channels.
3. WARN `capture_level_low` below ~−52 dB (configurable; floor derived
   from the corpus: golden era ≈ −38, break ≈ −44) — registered in the
   NON-terminal warning class; must never kill the dictation UI (the
   `28881bdd` class) and never touch `USER_TERMINAL_WARNING_CODES`.
4. Audio menu section: current device, native rate/channels, last-session
   level, coarse quality verdict; read-only from the open capture path —
   zero new permission prompts.
5. Feeds forward: Silero thresholds on degraded input (W13-3) calibrate
   against this receipt.

Verifier: normal-level replay ⇒ receipt, no WARN; attenuated take
(< −52 dB) ⇒ WARN fires, dictation UI stays alive, mic released at stop;
Audio menu screenshot on a live device.

### W13-6 — Lexicon voice + highlighting — `[ ]`

Current: 2 585 lexicon rows (seed 2401 + programming 155 + operator 8 +
protected 21) compiled into the binary and applied ONLY as post-hoc
enumerated rewrites; the strong mechanism —
`build_whisper_initial_prompt` (stream_postprocess.rs:399, "Vocabulary:",
224-token budget) wired into `scheduler.rs:476` (per-lane) and
`singleton.rs:357` (file mode) — sits behind `stt_initial_prompt_enabled`
whose **default is false** (pinned by loader.rs:2296), drawing from
protected+custom only. Loader gates (case-equal skip; custom plain
word→word rejection after the function-word poisoning incident) are
healthy and stay.

Implementation points:

1. Budget-aware deterministic prompt selection: protected > custom >
   session-relevant seed/programming picks; receipt logs selected terms +
   token count.
2. WER A/B on ≥3 real fixtures (bench-stt probe already exists,
   scripts/bench-stt.sh:615/:632): prompt ON vs OFF — the default flip
   goes to the operator WITH numbers. (The "custom LM for pl is dead"
   verdict does not cover this mechanism — initial_prompt is a decoder
   hint on vanilla weights.)
3. Apple lane voice: recon SFSpeech `contextualStrings` in the bridge;
   wire behind the same config if cheap, else a written verdict.
4. Highlighting on the time-pinned canvas: lexicon-corrected words marked;
   "pustki" — spans where Silero detected speech but no words landed —
   marked as gaps; a highlighted span can be sent to Teach as a lexicon
   candidate in one click.
5. No weakening of loader gates; no function-word poisoning regression.

Verifier: prompt-ON replay of the `171939` take yields canonical terms
("reports", "editors") in engine output BEFORE any rewrite, receipt
proving the prompt was sent; WER A/B table archived; highlight + gap
marker screenshot.

## 7. Sequencing

```
W13-1 ──────────────────────────────────────┐
W13-2 ──► W13-3 (keystone) ──► W13-4 ───────┼──► ⛔ operator buttons
W13-5 ──────────────────────────────────────┤    (default flips, DMG, PR)
W13-6 voice-half ───────┐                   │
W13-6 highlight-half ◄──┴── after W13-3 ────┘
```

- W13-1 ∥ W13-2 ∥ W13-5 ∥ W13-6(voice): disjoint file domains.
- W13-2 → W13-3: shared `core/stt` domain; W13-3 consumes W13-2's
  timestamped payloads.
- W13-3 → W13-4: the ledger deletes most of W13-4; re-scope before
  dispatch.
- W13-3 → W13-6(highlight): highlighting needs the time-pinned canvas.

## 8. Risks

| Risk                                                         | Grounding                                                                    | Mitigation                                                                                         |
| ------------------------------------------------------------ | ---------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Apple-boundary machinery vs. Silero identity (THE hard part) | All freeze/restart/novel-suffix guards assume Apple boundaries (recon Q2)    | Slice cumulative finals by time; land behind a lane flag; replay harness before live               |
| pl-PL per-word timestamp fidelity unverified                 | Only synthetic fixtures test it (live_stream.rs:433–448)                     | First execution step of W13-3 = histogram on a real take; fallback: proportional span distribution |
| Seal latency vs. live feel                                   | Seal waits for the utterance's Whisper pass (~0.4–1 s after silence edge)    | Preview lane unaffected; only committed status trails; stop pays one utterance max                 |
| Continuous speech without silence                            | Silero finds no edge                                                         | Max-length cut at the weakest Silero dip                                                           |
| Degraded input starves Silero too                            | Corpus: current sessions run ~−45 dB                                         | W13-5 receipt calibrates thresholds; operator fixes gain with data                                 |
| LLM formatter invents content                                | Observed live 2026-08-13                                                     | W13-1 anti-invention guard; fail-open to raw                                                       |
| Living Tree concurrency                                      | Concurrent sessions clobbered a shared test log 2026-08-13 (false "0 tests") | Isolated `SWIFT_TEST_LOG` per session; re-read before edit; commit in small packs                  |
| Sidecar supervision scope creep                              | —                                                                            | Seam + remote + fallback land first; sidecar may follow (brief §10)                                |
| Stale memory as false ground truth                           | The "seal machine orphan" memory survived 3 days past its wiring             | Verify wiring claims via `loct who-imports` on HEAD before acting                                  |

## 9. Acceptance discipline

Every cut carries a delivery-verifier (listed per cut above). Gates for
every commit: `cargo check --workspace`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` (pin env — the
operator dotenv leaks into tests), and for Swift surfaces `make
app-bindings` + `make test-swift` with an isolated `SWIFT_TEST_LOG`.
Green gates are necessary, not sufficient: the verifier measurement flips
the state, not the CI color.

## 10. Operator buttons (open decisions, delivered with numbers)

1. Inline-format buffer default ON/OFF — after the W13-1 stop-to-paste
   measurement.
2. Tail provider default (sidecar vs in-process) — after W13-2 latency
   receipts.
3. `stt_initial_prompt_enabled` default — after the W13-6 WER A/B.
4. Input gain / device decision — after W13-5 receipts confirm the W30
   drift on live sessions (and the operator's answer to "what changed
   Jul 20–26").
5. DMG publication and merges — as always.

## 11. Glossary

- **Canvas / ledger** — the transcript as an append-only sequence of
  sealed utterance records keyed by time spans.
- **Seal** — the transition making an utterance immutable after fusion +
  lexicon; also the "format now" signal for W13-1.
- **Span** — `[start, end)` in session time (PCM sample clock).
- **Gap-append** — recovered text appended to fill a hole in delivery.
- **Pustka** — a span where Silero detected speech but no words landed;
  prime Teach candidate.
- **Voice (of the lexicon)** — vocabulary delivered to a decoder before
  transcription (Whisper `initial_prompt`, SFSpeech `contextualStrings`),
  as opposed to post-hoc rewriting.
- **Best-truth swap** — per-word fusion inside an unsealed utterance,
  replacing weaker evidence with stronger regardless of which engine
  produced it.

## 12. Independent feasibility verdict (2026-08-13 evening)

A triple-agent research study (grok + claude + codex, independent lanes,
adversarial synthesis; run `rese-260813-190311-53919`) reviewed this
roadmap against the codebase and world SoTA. Verdict: **GO WITH
AMENDMENTS** — the doctrine is confirmed and SoTA-aligned (the same shape
appears in Apple time-ranged finality, streaming-Whisper stable prefixes,
WhisperX segmentation and two-pass ASR); all amendments target contracts,
not direction. Binding contract amendments (full text: plan-pack ATLAS,
Amendment 3):

- new **W13-0** first: real pl-PL clock/timestamp falsification + frozen
  golden replay fixtures before any fusion;
- canonical time = **integer sample ranges**
  (`session, capture_epoch, sample_start, sample_end`); seconds only at
  adapters;
- **typed evidence** (source, revision, stability, timing quality, raw
  confidence) precedes any confidence-based fusion; until calibration,
  commit agreements and clear gap fills, receipt the rest;
- **W13-2 → 2A/2B** (timed provider contract gates W13-3; sidecar hosting
  follows), **W13-3 → 3A/3B** (provenance before conservative fusion, with
  a bounded-context A/B), **W13-6 → 6A/6B** (voice early, highlighting
  after provenance);
- W13-4 auto-removal only on **non-content evidence**; continuous
  repetition protected;
- W13-5 warnings keyed to **active-speech** level (+ clipping/dropout/
  noise/SNR), not all-audio medians;
- no Apple backend migration this wave; vocabulary A/B must report
  U-WER + false insertions.

Rejected unanimously: full-file replacement, single-engine authority,
content-similarity dedup, unbounded prompting, and
unit-green-as-delivery-proof.

## 13. Comparative bench vs lbrx-stt-engine (2026-08-14) — the bar is low and the live lane is still under it

Same-host comparison against `lbrx-stt-engine` (whisper-large-v3-mlx-q8,
MLX; three transports: HTTP :8444, NDJSON :8445 `stt-jsonl-v1`,
WS :8446 `stt-ws-v1`). Grading context, stated plainly: that API is a
**neglected, low-intensity** file-mode service — slow-moving, with its own
hallucinations — not a state-of-the-art target. Codescribe receives an
order of magnitude more engineering. That is exactly what makes this bench
binding: **even a neglected engine beats our live canvas on content.**

### Measured (W13-0 golden takes, warm engine)

| Take   |   Audio | lbrx wall |   RTF | word-sim canvas↔lbrx | Decisive content deltas                                                                   |
| ------ | ------: | --------: | ----: | -------------------: | ----------------------------------------------------------------------------------------- |
| 171939 | 135.7 s |    4.02 s | 0.030 |                0.660 | canvas tail garbled ("czytą lebymiałą", "Pt. River ton"); lbrx tail clean and grammatical |
| 191351 | 337.4 s |    9.60 s | 0.028 |                0.686 | lbrx catches "voice isolation" 3×; canvas 0× (pl-PL code-switching blindness)             |
| 193523 |  27.9 s |    1.27 s | 0.045 |                0.789 | lbrx "WorkTrees" 3× correct; canvas 0× (Workplace/Warp3s manglings)                       |

lbrx word counts track the canvas (136/149, 364/344, 36/35) — no mass
hallucination, no mass loss; the delta is concentrated exactly in the
classes this roadmap names: vocabulary, code-switching, tail integrity.

### Honest defects of the reference engine (measured, not assumed)

- **Its segment timestamps lie by compaction**: reported coverage ends at
  70.6 s of 135.7 s and 199.6 s of 337.4 s while the transcribed content
  demonstrably reaches the end of both takes. This is the same clock-lie
  class W13-0 froze (compact drop 0.377 / 0.311). Any integration maps by
  **integer sample ranges, never by its reported seconds** — Amendment 3
  confirmed by a second, independent engine.
- Its own manglings exist ("Wipeshotted", "WordLine", "konkurencji" for
  "równoległości") — file-mode Whisper is a ruler with scratches, not
  truth. The only truth reference remains the human transcript (U-WER).
- File-mode warm RTF 0.028–0.045 is a batch number, not a live-latency
  claim.

### Diagnosis (holistic)

The gap is **not model capability** — we embed the same Whisper family.
The gap is the live lane: window feeding, patch authority, and buffer
integrity. Field evidence, same morning (Monika, 2026-08-14): 42% of
Layer-1 tail patches rejected (80 applied / 59 rejected across 10
sessions), and a dual-writer `transcript_buffer` split-brain (reducer 228
chars, final Apple partial 264, RAW 791 ≈ 3×264 — the same sentence
delivered almost three times). No ledger can save a buffer with two
writers; the single-writer emitter fix is a prerequisite cut.

The machinery to close the gap **already landed** in the W13 settlement
(`13b1eed8`, all defaults OFF): Silero-boundary fusion (3B, synthetic
starvation −67%), span idempotence (4), typed tail providers (2A/2B),
lexicon voice (6A). "Catching up" is therefore not new architecture — it
is wiring, measurement, and the operator's flip matrix, in this order:

1. single-writer emitter + final snapshot barrier (new cut, field P0);
2. real take-614 fusion A/B → `CODESCRIBE_SILERO_FUSION` decision;
3. span idempotence observed on a real session → flip decision;
4. optional: lbrx as a **remote tail provider** — its
   `hello/ack/vad/transcript.final` stream protocol is shape-compatible
   with the W13-2B remote slot, moving ~4 GB of Whisper RSS out of the
   app on hosts where the service runs anyway.

### Bench discipline going forward

The golden-take bench gains an lbrx column. The bar that ends this
section's shame: **layered-ON ≥ lbrx file-mode on U-WER vs human, at live
latency, on all three golden takes.** Apple-similarity stays a ruler,
never the gate (§12).

---

_Provenance: distilled from the 2026-08-12/13 measurement sessions, the
W13 plan pack (ATLAS incl. Amendments 1–3, briefs W13-1…W13-6, DRIVER,
de-risk recon with file:line evidence), the triple-agent feasibility
study `rese-260813-190311-53919`, and the operator's engine doctrine as
recorded in the session registry._

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders (c)2024-2026 LibraxisAI
