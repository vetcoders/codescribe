# WHISPER LIVE (Embedded + Streaming Transcription)

> **Status:** DONE ✅ (2026-01-16)
>
> **Tagline:** Whisper is welded into the binary, and transcription happens _during recording_.

## TL;DR

CodeScribe’s core power-up is:

1. **Embedded Whisper model** (`whisper-large-v3-turbo-mlx-q8`) in the release binary
   - **Zero disk I/O** for local STT
   - Model loads once into GPU/Metal and stays in-process
2. **Live (streaming) transcription** while the user is recording
   - Audio is chunked and transcribed in the background
   - On `stop()` we only “close” the last fragment → **near-instant time-to-paste**

## What we shipped

### 1) Embedded Whisper (Strict Policy)

- **ALWAYS Embedded:** The model (`whisper-large-v3-turbo-mlx-q8`) is welded into the release binary.
  - **Zero Exceptions:** We never bundle the `models/` folder in the `.app`.
  - **Zero Disk I/O:** Model loads directly from memory to Metal GPU.
  - **Native Power:** Minimal abstraction layers (approx. 4) compared to typical 32+ in heavy JS/Python bridges.
- **Global Singleton:** A process-wide engine instance loads once and stays resident.

Key behavior:

- **Release:** Strict embedded mode. If the model isn't found at build time, the build fails.
- **Development:** Local debug builds can still resolve external paths for rapid iteration, but the release pipeline enforces embedding.

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

### Embedded + singleton engine

- `src/whisper/embedded.rs` — embedded model bytes and accessors
- `src/whisper/singleton.rs` — global engine singleton (loads embedded model and exposes `transcribe*()`)
- `src/whisper/engine.rs` — Candle/Whisper inference, chunking, overlap dedup (`append_with_overlap_dedup`)

### Live streaming recorder

- `src/audio/recorder.rs`
  - CPAL input stream at **native device rate** (often `48000Hz`)
  - callback hook for raw `f32` samples
  - exposes `Recorder::actual_sample_rate()`
- `src/audio/streaming_recorder.rs`
  - connects recorder callback → `mpsc::channel` (non-blocking)
  - chunking (default: `15s` chunks + `2s` overlap)
  - background transcription via `tokio::spawn_blocking`
  - dedup between chunks via `append_with_overlap_dedup`
- `src/controller.rs`
  - uses `StreamingRecorder` and prefers the streaming transcript on `stop()`
  - can still save the WAV for logs and/or cloud fallback

## Build & distribution

### Install from source (embedded model)

```bash
make download-model   # ensures models/whisper-large-v3-turbo-mlx-q8 exists for embedding
make install          # builds + installs an ~888MB binary with embedded model
```

### Bundle / DMG (embedded-only)

```bash
make bundle
make dmg-full
```

Notes:

- DMG ships the `.app` with **the embedded model only** (no `Resources/models/*` duplication)
- A “too small” release binary is treated as a build error (guardrail in `scripts/build-release.sh`)

## Troubleshooting / FAQ

### “My binary is small — is the model embedded?”

If the release binary is far below ~500MB, embedding likely didn’t happen.

Checklist:

- ensure `models/whisper-large-v3-turbo-mlx-q8/` exists (download step)
- ensure `CODESCRIBE_NO_EMBED` is not set
- rebuild with `cargo build --release`

### “Why does streaming care about actual sample rate?”

Microphones usually run at `48kHz`. We record at the device’s native rate for compatibility,
and Whisper internally resamples to `16kHz`.

**Important:** streaming must pass the **real** `sample_rate` to the engine — otherwise you
get hallucinations and low confidence (classic “gibberish” pattern).

## Benchmarks (rule of thumb)

- Model load: ~7s (first time after app start, embedded → GPU)
- Live transcription: overlaps with recording
- After `stop()`: usually just final chunk, typically well below 1s

---

**Made with (งಠ_ಠ)ง by the ⌜ CodeScribe ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026
Maciej & Monika + Klaudiusz (AI) + Junie (AI)**
