# WHISPER LIVE (Embedded Whisper + Streaming Transcription)

> **Status:** provisioning facts retained from 2026-01-16 · **Current anatomy:**
> 2026-08-25 after the C11 source cut, as the bounded L1 observer inside the
> Apple-ledger session. `484095ce` was the last executable cut before docs-only
> successor `d57196ab`; C11 compiler/runtime behavior is `NOT_ASSESSED`.
>
> **Tagline:** Whisper can stay local and ships embedded when a complete model
> snapshot is available; normal live work observes retained PCM without owning
> the microphone, dispatcher, occurrence ledger, or document reducer.

## Role in the canonical four-layer pipeline

Whisper is **L1 — contextual observer** inside the canonical
[four-layer engine contract](./THE_ENGINE_CONTRACT.md). Normal live capture
dispatches only to `apple_stream_transcription_session`. Apple supplies the
first observation; an armed Whisper tail provider receives a bounded retained-
PCM window carrying session, successful-open epoch, sample range, request, and
generation identity. Its result is offered to the same `AcousticLedger` as the
Apple observation and may relabel only that authorized occurrence.

Normal stop never performs a hidden Whisper file pass. Full-file decoding
belongs to explicit Retranscribe, a separate operator action over a selected
completed artifact. Its output remains a proposal until the operator accepts
it; it is not a continuation of live capture or automatic Transcript Bus truth.

The contract has exactly four machine layers. Silero remains orthogonal VAD/time
evidence, with richer annotations optional and provider-bound. Final BAM is
superseded and has no automatic producer; `SessionFinalised` is lifecycle-only.
The Responses formatter is the fourth layer: an occurrence-bound proposal/repair
observer constrained by ledger admission, never a second reducer or delivery
dispatcher.

**Hard invariant that gates every Whisper write:** _NEVER REWRITE FROM ZERO._
Whisper may relabel only a proven occurrence identity. Text alignment and
change ratios may rank a candidate after authority is established, but they
never mint, merge, transfer, or erase occurrence authority. Unproven or cross-
span work fails closed. See the engine contract for the full rule.

## TL;DR

Codescribe’s Whisper layer power-ups:

1. **FP16-only Whisper model** (`whisper-large-v3-turbo`, mlx-community weights; q8 is rejected before load)
   - readiness requires a parsed config and tokenizer, pinned mel SHA-256, and a
     structurally complete safetensors file containing only supported runtime dtypes
   - build policy embeds Whisper whenever the model is available at build time
   - runtime lookup from `CODESCRIBE_MODEL_PATH`, configured model dirs, bundled app resources, or the Hugging Face cache is a fallback path for `CODESCRIBE_NO_EMBED=1` builds or recovery
2. **Bounded live Layer 1 observation** while the user is recording
   - `transcription_session` still dispatches only to the Apple session
   - the tail provider observes retained PCM associated with one authorized occurrence
   - `AcousticLedger` decides admission and seal; `PresentationEmitter` /
     `TranscriptReducer` commits the document projected to Transcript Bus and Swift
3. **Full WAV is always teed to disk** — L1 may read retained PCM without a
   second microphone; the saved WAV remains available for explicit
   Retranscribe/HQ and diagnostics, not an automatic fifth layer

## What we shipped

### 1) Embedded Whisper (Current Policy)

- **Embedded-first:** `core/build.rs` embeds Whisper by default when a complete model snapshot is available.
  - Prefer the embedded payload for shipped behavior.
  - If embedding is disabled with `CODESCRIBE_NO_EMBED=1` or the model is absent at build time, resolve from `CODESCRIBE_MODEL_PATH`, configured model dirs, app resources, or HF cache.
  - Both paths stay local and use Metal once loaded.
- **Global Singleton:** A process-wide engine instance loads once and stays resident.

Key behavior:

- **Shipped build:** embedded Whisper is the canonical path.
- **Fallback build/runtime:** runtime model lookup remains available when embedding is intentionally unavailable.

### 2) Live transcription authority

The live product path is occurrence-ledger and reducer owned. `AcousticLedger` decides admission
and seal; `PresentationEmitter` / `TranscriptReducer` owns committed document text. See
[`TRANSCRIPT_LANES.md`](TRANSCRIPT_LANES.md) for the canonical live topology and the rule that
decoder context overlap is resolved by request/span identity, never textual similarity.

## What’s new around Whisper Live

- **Typed tail-provider observation** (`core/stt/tail_provider.rs`,
  `core/stt/tail_patcher/`) — a request and its returned segments retain PCM
  identity before the candidate reaches `admit_ledger_label` and
  `AcousticLedger`.
- **IPC server** (`app/ipc/`) — stable runtime interface for GUI/clients. Raw
  `UtteranceFinal`, `Correction`, `ReplaceRange`, and `InsertAnnotation` events
  may remain observable diagnostics; they are not document commands. Committed
  text comes only from ledger-receipt projection.
- **Quality loop/report** (`bin/codescribe_quality`, `bin/codescribe_loop`) — automated scoring and
  batch diagnostics. Layer receipts identify Whisper proposals, L2 shaping,
  L3 formatting outcomes, and orthogonal timing evidence so regression hunts
  target the right owner.
- **Cloud STT** — an optional consent-gated provider implementation behind the
  same Layer 1 contract. Transport choice does not grant broader occurrence
  authority than the local provider.

## Layer mapping for this file

| Section below                                    | Layer it lights up                                               |
| ------------------------------------------------ | ---------------------------------------------------------------- |
| Embedded Whisper (build + runtime lookup)        | Layer 1 (Tail Patch) backend resolution                          |
| Live observation path                            | Ledger admission/seal and reducer-owned committed text           |
| Typed tail-provider request and ledger admission | PCM-bound Layer 1 observation and occurrence-safe relabeling     |
| Cloud STT alternatives                           | Pluggable Layer 1 backend                                        |
| Lexicon substitution + Light+                    | L2 — deterministic and currently wired at seal/delivery          |
| Inline formatting scheduler                      | L3 — existing Responses Formatting lane; no separate small model |

The remaining sections describe retained Whisper provisioning and live observation surfaces.
Canonical ownership and routing stay in [`TRANSCRIPT_LANES.md`](TRANSCRIPT_LANES.md).

## How it works (high level)

Live audio observations remain attached to PCM/session identity through the ledger path, then
committed reducer events drive presentation and delivery. The complete lane graph and overlap law
live in [`TRANSCRIPT_LANES.md`](TRANSCRIPT_LANES.md); this document does not redefine them.

## Where in the code

### Embedded payload + singleton engine

- `core/stt/whisper/embedded.rs` — embedded Whisper payload exposed to the engine when compiled in
- `core/stt/whisper/singleton.rs` — global engine singleton (prefers embedded payload, falls back to runtime model lookup)
- `core/stt/whisper/engine.rs` — Candle/Whisper inference and active long-window decoder internals

### Live streaming recorder

- `core/audio/recorder.rs`
  - CPAL input stream at **native device rate** (often `48000Hz`)
  - callback hook for raw `f32` samples
  - exposes `Recorder::actual_sample_rate()`
- `core/audio/streaming_recorder.rs`
  - connects recorder callback → `mpsc::channel` (non-blocking)
  - retains PCM/session evidence for the ledger-owned live path
- `app/controller/mod.rs`
  - uses `StreamingRecorder` and prefers the streaming transcript on `stop()`
  - can retain the WAV for logs, diagnostics, and explicit Retranscribe without
    turning it into an automatic normal-stop replacement

## Build & distribution

### Install from source (embedded-first Whisper)

```bash
make install          # ensures runtime model/cache availability and installs the CLI
```

### Bundle / DMG

```bash
make app PROFILE=local-release
make dmg-signed
```

Notes:

- DMG / app builds now prefer embedded Whisper when the model is available in the build context.
- `make install-no-embed` or `CODESCRIBE_NO_EMBED=1` disables optional embedding and requires runtime Whisper lookup.

## Troubleshooting / FAQ

### “Whisper cannot be found at runtime”

Checklist:

- set `CODESCRIBE_MODEL_PATH` to a valid Whisper directory, or
- warm the HF cache with `make install` / `make download-model`
- verify the resolved path has `config.json`, `tokenizer.json`, `mel_filters.npz`, and safetensors weights

### “How do I know which provisioning path I’m on?”

- Default build with model available: embedded Whisper payload
- Explicit `CODESCRIBE_NO_EMBED=1`: runtime lookup
- Missing model during build: runtime lookup fallback for that artifact

### “Why does live recognition care about actual sample rate?”

Microphones usually run at `48kHz`. We record at the device’s native rate for compatibility,
and Whisper internally resamples to `16kHz`.

**Important:** streaming must pass the **real** `sample_rate` to the engine — otherwise you
get hallucinations and low confidence (classic “gibberish” pattern).

## Benchmarks (rule of thumb)

- Model load: first init depends on local path/cache, then the engine stays resident
- Live transcription: overlaps with recording

---

**Made with (งಠ_ಠ)ง by the ⌜ Codescribe ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026**
