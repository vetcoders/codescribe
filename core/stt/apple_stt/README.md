# Apple On-Device STT Bridge

This directory contains the Apple STT backend for codescribe:

- `mod.rs` — Rust `TranscriptionAdapter` + subprocess bridge client + final-pass verdict helper.
- `codescribe-stt-bridge.swift` — Swift bridge executable with dual-backend selection.

## Backend selection (per locale)

| Priority | Backend                              | When                                                                               |
| -------- | ------------------------------------ | ---------------------------------------------------------------------------------- |
| 1        | `SpeechTranscriber` (SpeechAnalyzer) | Locale is in ST supported+installed catalog                                        |
| 2        | `SFSpeechRecognizer` on-device       | ST lacks the locale **and** `supportsOnDeviceRecognition` is true (e.g. **pl-PL**) |
| 3        | Error                                | Neither backend can serve the locale                                               |

SFSpeechRecognizer is the current public dictation-class API and the product's
foundation for Polish — **not** a "legacy" path. Whisper remains the fallback
engine, tail-patch donor, and quality second opinion when Apple fails.

## Why Subprocess Bridge

Apple speech APIs are Swift-first. Keeping Swift in a separate executable gives:

- fast integration with low Rust-side risk
- clear failure boundaries and easy fallback to Candle Whisper
- no Rust FFI surface to maintain across Apple SDK changes

## Build Bridge

```bash
swiftc -O -o codescribe-stt-bridge core/stt/apple_stt/codescribe-stt-bridge.swift
```

`make app` builds this helper and bundles it in:

```text
Codescribe.app/Contents/MacOS/codescribe-stt-bridge
```

For local bridge development without rebuilding the app, set:

```bash
export CODESCRIBE_APPLE_STT_BRIDGE=/absolute/path/to/codescribe-stt-bridge
```

When the override is unset, the resolver checks the bundled helper beside the
current `.app` executable first, then falls back to `codescribe-stt-bridge` on
`PATH`.

## Runtime Env

- `CODESCRIBE_STT_ENGINE=auto` uses Apple on-device on supported macOS and falls back to Candle Whisper when unavailable.
- `CODESCRIBE_STT_ENGINE=apple` forces the Apple path while preserving runtime fallback to Candle.
- `CODESCRIBE_APPLE_STT_BRIDGE=/absolute/path/to/codescribe-stt-bridge` (optional dev override; wins over bundled helper and `PATH`)
- `CODESCRIBE_APPLE_STT_LOCALE=pl-PL` (optional; defaults to `pl-PL`)
- `CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD=1` (allow SpeechTranscriber asset install via `AssetInventory`)

On unsupported hosts (non-macOS or macOS < 26), Codescribe logs a warning and falls back to Candle Whisper.

## Bridge protocol

JSON stdin request / JSON stdout response, `protocol_version: 1`.

Additive fields (no wire version bump):

- `backend`: `speech_transcriber` | `sf_speech_recognizer` (probe + transcribe)

## Backend order (supported **and** installed)

1. **SpeechTranscriber** — only when the locale is in the ST catalog **and**
   the model assets are installed (optional download when
   `CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD=1`).
2. **SFSpeechRecognizer on-device** — when ST lacks the locale, or ST is in the
   catalog but assets are missing, and SF supports on-device recognition for
   that locale (notably **pl-PL**).
3. Honest error — only when neither backend can serve the locale.

A stalled SFSpeech callback is cancelled after ~2.5 s
(`CODESCRIBE_SFSPEECH_DEADLINE_SECS` override) so Whisper fallback is not
blocked for the full 30 s bridge timeout.
