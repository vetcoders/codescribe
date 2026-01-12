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

| Key | Action |
|-----|--------|
| Hold **Ctrl** | Start recording |
| Release **Ctrl** | Stop & transcribe |
| **Option** (tap) | Toggle pause |

## Model

Bundled in app: `whisper-large-v3-turbo-mlx-q8` (874MB)

Location (dev): `models/whisper-large-v3-turbo-mlx-q8/`

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
USE_LOCAL_STT=true
LOCAL_MODEL=whisper-large-v3-turbo-mlx-q8
WHISPER_LANGUAGE=auto
LLM_HOST=http://localhost:11434
```

## Troubleshooting

### App doesn't start
- Check Console.app for crash logs
- Ensure model exists in `models/` directory

### Hotkeys don't work
- Grant Accessibility permission
- Grant Input Monitoring permission
- Restart app after granting

### No transcription
- Check `USE_LOCAL_STT=true` in config
- Verify model path exists

---
*Created by M&K (c)2026 VetCoders*
