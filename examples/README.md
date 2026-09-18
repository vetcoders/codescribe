# Codescribe Examples

This directory contains practical examples demonstrating how to use the codescribe audio, config, and transcription modules.

## Available Examples

### `live_max_consultation.rs`

Explicit live-provider acceptance for two Max turns. Requires an existing Max
formatting configuration and its normal credentials; provider usage may be billed.

```bash
cargo run --example live_max_consultation -- --run
```

Without `--run`, it exits before reading settings or credentials. The example
copies settings into a temporary directory and uses the production provider,
Max owner and history store. It asks for a neutral random code to be remembered,
then requires that exact code on the second turn and four settled messages in
the same consultation. It verifies that the source settings bytes did not change.
Existing process overrides still apply; optional live `.env` and prompt files
are not copied. This is therefore not a claim of parity with customized prompts.

The normal Keychain permission prompt may require a human decision. Do not
disable the test-harness Keychain guard or copy secrets into a test fixture.
This example has no executable tools and does not read the clipboard, record
audio, paste text, or prove GUI/bus/cross-host acceptance. Temporary history is
removed on normal exit. Request timeouts are 90 seconds per turn; system
Keychain interaction occurs before those requests and may wait for permission.

### `config_demo.rs`

Demonstrates the `Config` module: loading config from `.env`/defaults, parsing the `Language` enum, and saving a single value back via `save_to_env`.

```bash
cargo run --example config_demo
```

### `demo_full_pipeline.rs`

Full pipeline demo covering local Whisper STT transcription, AI formatting (normal mode), and AI assistive mode (kurier/enhancer) on a given audio file.

```bash
cargo run --release --example demo_full_pipeline -- <audio_file>
cargo run --release --example demo_full_pipeline -- --assistive <audio_file>
```

Requires the complete fp16 Whisper model at `~/.codescribe/models/whisper-large-v3-turbo` (override with `--model`) and `LLM_ENDPOINT`/`LLM_MODEL` (or `LLM_FORMATTING_*` overrides) for the formatting step. Quantized Whisper payloads are refused.

### `e2e_stt.rs`

End-to-end STT smoke check against sample audio files, verifying the model/tokenizer are present before transcribing.

```bash
cargo run --example e2e_stt
```

Sample audio paths are configurable via `CODESCRIBE_E2E_AUDIO_MEDIUM`, `CODESCRIBE_E2E_AUDIO_SHORT`, and `CODESCRIBE_E2E_LANG`.

### `roundtrip_live.rs`

Interactive round-trip demo: speak into the mic, transcribe with Whisper, synthesize with TTS, play back through the speaker, transcribe again, and compare the two transcripts.

```bash
cargo run --release --example roundtrip_live
cargo run --release --example roundtrip_live -- --text "Hello world"
```

### `test_audio_long.rs`

Batch-transcribes one or more long audio files with the local Whisper engine and reports load/transcription timing.

```bash
cargo run --release --example test_audio_long -- [--model PATH] <audio1> ...
```

### `test_audio.rs`

Transcribes one or more audio files with language detection, printing detected language and transcription timing per file.

```bash
cargo run --release --example test_audio -- <audio1> <audio2> ...
```

### `test_clipboard_snapshot.rs`

Demonstrates clipboard snapshot/restore and smart-paste behavior (`ClipboardSnapshot`, `paste_text_smart`, `paste_and_restore`).

```bash
cargo run --example test_clipboard_snapshot
```

### `transcribe_file.rs`

Quick transcription utility for a single (typically large) audio file, with optional explicit language.

```bash
cargo run --release --example transcribe_file -- /path/to/audio.wav [language]
```

## Environment Variables

Several examples respect these environment variables:

- `AUTO_SILENCE` - Enable/disable silence detection (default: true)
- `CODESCRIBE_E2E_AUDIO_MEDIUM`, `CODESCRIBE_E2E_AUDIO_SHORT`, `CODESCRIBE_E2E_LANG` - sample inputs for `e2e_stt`
- `LLM_ENDPOINT`, `LLM_MODEL`, `LLM_FORMATTING_*` - formatting provider config for `demo_full_pipeline`

VAD internals are hardcoded in `core/vad/config.rs` (Silero defaults).

## Requirements

- macOS (uses CoreAudio via cpal)
- Microphone access permissions
- Rust 1.88+ (the workspace MSRV) with tokio runtime

---

Created by Vetcoders (c)2025
