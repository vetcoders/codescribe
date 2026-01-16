# CodeScribe - Team Setup (Pure Rust Era)

## Quick Start

### 1. Prerequisites

- macOS 14+ (Apple Silicon recommended)
- Rust 1.83+ with `wasm32-unknown-unknown` target
- Trunk (`cargo install trunk`)
- Tauri CLI (`cargo install tauri-cli`)

### 2. Build & Run

```bash
# Clone
git clone git@github.com:VetCoders/CodeScribe.git
cd CodeScribe

# Build WASM frontend
cd tauri-app && trunk build && cd ..

# Build and run app
cargo tauri build --no-bundle
open target/release/bundle/macos/CodeScribe.app
```

### 3. Development Mode

```bash
# Terminal 1: Trunk dev server
cd tauri-app && trunk serve --port 8080

# Terminal 2: Run debug binary
./target/debug/codescribe-app
```

## Permissions Required

Grant in: System Settings > Privacy & Security

1. **Microphone** - for audio recording
2. **Accessibility** - for global hotkeys
3. **Input Monitoring** - for hotkey capture

## Hotkeys

| Key                   | Action                             |
|-----------------------|------------------------------------|
| Hold **Ctrl**         | Start recording (hold-to-talk)     |
| Release **Ctrl**      | Stop → finalize last chunk → paste |
| Double-tap **Option** | Toggle recording (hands-free)      |

## Model

**Embedded in the binary (release)**: `whisper-large-v3-turbo-mlx-q8` (~888MB)

- No runtime model download
- No `Resources/models/*` bundling in the `.app`
- Model bytes are loaded directly into Metal (zero disk I/O)

**Developer note:** to build an embedded release locally you still need the model folder present
so it can be embedded at build time:

```bash
make download-model
```

Location (dev/build-time): `models/whisper-large-v3-turbo-mlx-q8/`

## CLI Usage

```bash
# Transcribe audio file
codescribe transcribe audio.wav

# With AI formatting
codescribe transcribe audio.wav --format

# Specify language
codescribe transcribe audio.wav --language pl
```

## Configuration

File: `~/.codescribe/.env`

```env
USE_LOCAL_STT=1

# Whisper
WHISPER_LANGUAGE=pl

# AI formatting (optional)
AI_FORMATTING_ENABLED=1
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
LLM_API_KEY=sk-proj-xxx
```

## Troubleshooting

### App doesn't start

- Check Console.app for crash logs
- If building locally: ensure the model exists in `models/` (for embedding at build time)

### Hotkeys don't work

- Grant Accessibility permission
- Grant Input Monitoring permission
- Restart app after granting

### No transcription

- Check `USE_LOCAL_STT=1` in config
- If using local STT: confirm the app is using the embedded engine (default in release builds)

---
*Created by M&K (c)2026 VetCoders*
