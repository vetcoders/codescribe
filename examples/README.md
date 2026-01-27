# CodeScribe Examples

This directory contains practical examples demonstrating how to use the CodeScribe audio recording module.

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

- `CODESCRIBE_VAD_THRESHOLD` - Speech probability threshold 0.0-1.0 (default: 0.5)
- `CODESCRIBE_VAD_SILENCE_SEC` - Silence duration before auto-stop (default: 1.2)
- `CODESCRIBE_VAD_ENABLED` - Enable/disable silence detection (default: true)

Example:
```bash
CODESCRIBE_VAD_THRESHOLD=0.4 CODESCRIBE_VAD_SILENCE_SEC=1.5 cargo run --example record_test
```

## Requirements

- macOS (uses CoreAudio via cpal)
- Microphone access permissions
- Rust 1.70+ with tokio runtime

---
Created by M&K (c)2025 The LibraxisAI Team
