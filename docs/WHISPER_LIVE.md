# WHISPER LIVE (Embedded Whisper + Streaming Transcription)

> **Status:** DONE ✅ (2026-01-16) · **Re-framed:** 2026-05-26 as Layer 1 + Layer 2 supplement.
>
> **Tagline:** Whisper stays local, ships embedded by default, and patches the live overlay in the background — it is no longer the first thing the user sees.

## Role in the layered pipeline (ADR 2026-05-26)

Whisper is now **Layer 1 — Tail Patch** and feeds **Layer 2 — Lexicon + LLM Polish** inside the
[Layered Incremental Transcription Pipeline](./ADR/2026-05-26-LAYERED_INCREMENTAL_TRANSCRIPTION.md).
Live first-pass text in the overlay comes from **Layer 0 — Apple Speech Recognizer**
(`CODESCRIBE_STT_ENGINE=apple`); Whisper runs on the same audio tail in the background, diffs
against Layer 0's committed buffer, and emits `EngineEvent::ReplaceRange { source: TailPatch }`
events that visibly patch tokens Apple missed — mixed-language inserts, rare terminology, proper
nouns. The legacy "Whisper-as-primary" path stays as automatic fallback when Apple Speech
is unavailable (no permission, no macOS Speech framework).

**Hard invariant that gates every Whisper write:** _NEVER REWRITE FROM ZERO._ Tail Patch may
only `ReplaceRange` inside the utterance window Layer 0 already committed. If the diff distance
exceeds the safety threshold, the patch is dropped (annotation emitted) and Layer 0 output stands.
See the ADR for the full contract.

## TL;DR

Codescribe’s Whisper layer power-ups:

1. **Embedded-first Whisper model** (`whisper-large-v3-turbo-mlx-q8` by default)
   - build policy embeds Whisper whenever the model is available at build time
   - runtime lookup from `CODESCRIBE_MODEL_PATH`, configured model dirs, bundled app resources, or the Hugging Face cache is a fallback path for `CODESCRIBE_NO_EMBED=1` builds or recovery
2. **Live (streaming) transcription** while the user is recording
   - Audio is chunked and transcribed in the background
   - In the layered model: Whisper events arrive as `ReplaceRange` patches **after** Apple's live
     deltas — the user sees Layer 0 first, then watches Whisper magically correct mixed-language /
     terminology tokens within ~1 s of utterance end
   - In fallback (no Apple): Whisper takes over the live preview path, behaving like pre-ADR builds
3. **Full WAV is always teed to disk** — Layer 1 reads from this persistent tail (no extra mic load),
   Layer 4 (Final BAM) reuses the same WAV at session end

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

### 2) Streaming transcription (during recording)

We removed the old bottleneck:

```text
Audio callback → buffer → stop() → WAV write/read → transcribe entire audio → LLM
```

And replaced it with:

```text
Audio callback → non-blocking channel → chunking worker → spawn_blocking(Whisper) → transcript buffer
                                                         ↓
                                                     overlap dedup

stop() → transcribe last pending samples → return final transcript → LLM/paste
```

Practical win:

- **~35s recording:** `stop()` is ~0.5s (last chunk only) instead of ~4s (whole audio)

## What’s new around Whisper Live

- **Stream postprocess** (`core/pipeline/stream_postprocess.rs`) — semantic gating and cleanup of
  chunk output. In the layered model this feeds Layer 1's diff input — patches are made against
  the post-processed text, not the raw decoder output.
- **IPC server** (`app/ipc/`) — stable runtime interface for GUI/clients; Whisper Live can be
  consumed and extended outside the tray flow. After the ADR, the IPC contract also carries
  `ReplaceRange` and `InsertAnnotation` events for clients that render the layered view.
- **Quality loop/report** (`bin/codescribe_quality`, `bin/codescribe_loop`) — automated scoring and
  batch diagnostics. The layered telemetry adds per-layer counters (utterances patched, LLM calls,
  annotations inserted) so regression hunts can target the right layer.
- **Cloud STT** — optional Layer 1 backend (libraxis cluster / OpenAI whisper-1 / `mlx-audio` +
  `openai/whisper-large-v3`). Latency vs. privacy trade-off lives in Settings; not live preview.

## Layer mapping for this file

| Section below | Layer it lights up |
| --- | --- |
| Embedded Whisper (build + runtime lookup) | Layer 1 (Tail Patch) backend resolution |
| Streaming transcription, chunker, overlap dedup | Layer 1 background pass on utterance tail |
| Stream postprocess, semantic gate | Pre-diff cleanup feeding Layer 1's `ReplaceRange` decision |
| Cloud STT alternatives | Pluggable Layer 1 backend |
| (NEW, Phase 2) Lexicon + small LLM passes | Layer 2 (Polish) — see ADR §Layer specifications |

Everything below this point is the same Whisper-Live tech that existed before the ADR — it is
**not removed**, just relocated in the architecture: Whisper became the silent partner that makes
Apple's first pass true.

## How it works (high level)

```mermaid
flowchart TD
    A[CPAL input callback (audio thread)] -->|try_send f32 samples| B[mpsc channel]
    B --> C[StreamingRecorder worker (tokio task)]
    C -->|accumulate| D[chunk buffer]
    D -->|every ~15s with ~2s overlap| E[spawn_blocking]
    E --> F[Whisper singleton engine (Metal)]
    F --> G[chunk text]
    G --> H[append_with_overlap_dedup]
    H --> I[transcript_buffer]
    I --> J[controller stop(): finalize + paste / LLM]
```

## Where in the code

### Embedded payload + singleton engine

- `core/stt/whisper/embedded.rs` — embedded Whisper payload exposed to the engine when compiled in
- `core/stt/whisper/singleton.rs` — global engine singleton (prefers embedded payload, falls back to runtime model lookup)
- `core/stt/whisper/engine.rs` — Candle/Whisper inference, chunking, overlap dedup (`append_with_overlap_dedup`)

### Live streaming recorder

- `core/audio/recorder.rs`
  - CPAL input stream at **native device rate** (often `48000Hz`)
  - callback hook for raw `f32` samples
  - exposes `Recorder::actual_sample_rate()`
- `core/audio/streaming_recorder.rs`
  - connects recorder callback → `mpsc::channel` (non-blocking)
  - chunking (default: `15s` chunks + `2s` overlap)
  - background transcription via `tokio::spawn_blocking`
  - dedup between chunks via `append_with_overlap_dedup`
- `app/controller/mod.rs`
  - uses `StreamingRecorder` and prefers the streaming transcript on `stop()`
  - can still save the WAV for logs and/or cloud final transcript replacement

## Build & distribution

### Install from source (embedded-first Whisper)

```bash
make install          # ensures runtime model/cache availability and installs the CLI
```

### Bundle / DMG

```bash
make bundle
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

### “Why does streaming care about actual sample rate?”

Microphones usually run at `48kHz`. We record at the device’s native rate for compatibility,
and Whisper internally resamples to `16kHz`.

**Important:** streaming must pass the **real** `sample_rate` to the engine — otherwise you
get hallucinations and low confidence (classic “gibberish” pattern).

## Benchmarks (rule of thumb)

- Model load: first init depends on local path/cache, then the engine stays resident
- Live transcription: overlaps with recording
- After `stop()`: usually just final chunk, typically well below 1s

---

**Made with (งಠ_ಠ)ง by the ⌜ Codescribe ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026**
