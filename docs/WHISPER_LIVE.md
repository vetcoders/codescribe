# WHISPER LIVE (Runtime Model + Streaming Transcription)

> **Status:** DONE ✅ (2026-01-16)
>
> **Tagline:** Whisper stays local, loads at runtime, and transcription happens _during recording_.

## TL;DR

CodeScribe’s core power-up is:

1. **Runtime-managed Whisper model** (`whisper-large-v3-turbo-mlx-q8` by default)
   - build policy disables Whisper embedding
   - runtime resolves the model from `CODESCRIBE_MODEL_PATH`, configured model dirs, bundled app resources, or the Hugging Face cache
2. **Live (streaming) transcription** while the user is recording
   - Audio is chunked and transcribed in the background
   - On `stop()` we only “close” the last fragment → **near-instant time-to-paste**

## What we shipped

### 1) Runtime Whisper (Current Policy)

- **Runtime-managed:** `core/build.rs` hard-disables Whisper embedding.
  - Prefer `CODESCRIBE_MODEL_PATH` when explicitly set.
  - Otherwise resolve from configured model dirs, app resources, or HF cache.
  - The local path still stays on-device and uses Metal once loaded.
- **Global Singleton:** A process-wide engine instance loads once and stays resident.

Key behavior:

- **Shipped build:** runtime model lookup is the canonical path.
- **Experimental builds:** optional embedded helpers still exist, but they are not the product default.

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

- **Stream postprocess** (`src/stream_postprocess.rs`) — semantic gating and cleanup of chunk output
  before final paste/LLM, reducing low‑quality fragments in live mode.
- **IPC server** (`src/ipc/`) — stable runtime interface for GUI/clients; Whisper Live can be
  consumed and extended outside the tray flow.
- **Quality loop/report** (`src/quality_loop.rs`, `src/quality_report.rs`) — automated scoring and
  batch diagnostics for streaming accuracy and regressions.
- **Cloud STT** — optional post-capture replacement for the committed transcript; it is not live preview.

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

### Runtime lookup + singleton engine

- `core/stt/whisper/embedded.rs` — optional embedded hooks (normally unavailable in current builds)
- `core/stt/whisper/singleton.rs` — global engine singleton (resolves runtime model and exposes `transcribe*()`)
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

### Install from source (runtime Whisper)

```bash
make install          # ensures runtime model/cache availability and installs the CLI
```

### Bundle / DMG

```bash
make bundle
make dmg-signed
```

Notes:

- DMG / app builds still rely on runtime Whisper lookup.
- `make install-no-embed` disables optional non-Whisper embedded support assets and requires `CODESCRIBE_MODEL_PATH`.

## Troubleshooting / FAQ

### “Whisper cannot be found at runtime”

Checklist:

- set `CODESCRIBE_MODEL_PATH` to a valid Whisper directory, or
- warm the HF cache with `make install` / `make download-model`
- verify the resolved path has `config.json`, `tokenizer.json`, `mel_filters.npz`, and safetensors weights

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

**Made with (งಠ_ಠ)ง by the ⌜ CodeScribe ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026
Maciej & Monika + Klaudiusz (AI) + Junie (AI)**
