# Codescribe Examples

This directory contains practical examples demonstrating how to use the codescribe audio recording module.

## Available Examples

### 1. Basic Recording (`record_test.rs`)

Demonstrates basic Recorder usage with auto-silence detection.

```bash
cargo run --example record_test
```

**Features shown:**

- Creating a Recorder with default config
- Starting/stopping recording
- Auto-silence detection (stops after 0.8s of silence)
- Saving to WAV file
- Getting recording duration and diagnostics

### 2. Streaming Mode (`record_streaming.rs`)

Demonstrates advanced usage with live snapshots for streaming STT.

```bash
cargo run --example record_streaming
```

**Features shown:**

- Custom configuration (disabling auto-silence)
- Taking periodic snapshots while recording
- Manual control over recording lifecycle
- Use case: Real-time transcription with streaming API

## Environment Variables

Both examples respect these environment variables:

- `AUTO_SILENCE` - Enable/disable silence detection (default: true)

VAD internals are hardcoded in `core/vad/config.rs` (Silero defaults).

## Requirements

- macOS (uses CoreAudio via cpal)
- Microphone access permissions
- Rust 1.70+ with tokio runtime

---

Created by vetcoders (c)2025
