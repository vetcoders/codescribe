# Apple SpeechAnalyzer STT Bridge

This directory contains the Apple STT backend for codescribe:

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

- `CODESCRIBE_STT_ENGINE=auto` uses Apple SpeechAnalyzer on supported macOS and falls back to Candle Whisper when unavailable.
- `CODESCRIBE_STT_ENGINE=apple` forces the Apple path while preserving runtime fallback to Candle.
- `CODESCRIBE_APPLE_STT_BRIDGE=/absolute/path/to/codescribe-stt-bridge` (optional dev override; wins over bundled helper and `PATH`)
- `CODESCRIBE_APPLE_STT_LOCALE=pl-PL` (optional; defaults to `pl-PL`)
- `CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD=1` (allow asset install via `AssetInventory`)

On unsupported hosts (non-macOS or macOS < 26), Codescribe logs a warning and falls back to Candle Whisper.
