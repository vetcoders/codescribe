# Apple SpeechAnalyzer STT Bridge

This directory contains the Apple STT backend for CodeScribe:

- `mod.rs` — Rust `TranscriptionAdapter` implementation + subprocess bridge client.
- `codescribe-stt-bridge.swift` — Swift bridge executable using `SpeechAnalyzer` / `SpeechTranscriber`.

## Why Subprocess Bridge

`SpeechAnalyzer` is Swift-first. Keeping Swift in a separate executable gives:

- fast integration with low Rust-side risk
- clear failure boundaries and easy fallback to Candle Whisper
- no Rust FFI surface to maintain across Apple SDK changes

## Build Bridge

```bash
swiftc -O -o codescribe-stt-bridge core/stt/apple_stt/codescribe-stt-bridge.swift
```

Put `codescribe-stt-bridge` on `PATH`, or set:

```bash
export CODESCRIBE_APPLE_STT_BRIDGE=/absolute/path/to/codescribe-stt-bridge
```

## Runtime Env

- `CODESCRIBE_STT_ENGINE=apple`
- `CODESCRIBE_APPLE_STT_LOCALE=pl-PL` (optional; defaults to `pl-PL`)
- `CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD=1` (allow asset install via `AssetInventory`)

On unsupported hosts (non-macOS or macOS < 26), CodeScribe logs a warning and falls back to Candle Whisper.
