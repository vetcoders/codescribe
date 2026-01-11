# CodeScribe

**Local Speech-to-Text for macOS (Pure Rust)**

CodeScribe is a native macOS menu-bar application that captures audio through global hotkeys, transcribes it locally using Whisper with Metal GPU acceleration, and pastes the transcript directly into the focused application. Optional AI formatting via Ollama polishes the output while keeping everything private and local.

## Features

- **Pure Rust Implementation** - Native macOS app built entirely in Rust with candle-core + Metal GPU
- **Local Whisper STT** - whisper-large-v3-turbo-mlx-q8 model (4-layer turbo, 10x faster than full)
- **Metal GPU Acceleration** - Hardware-accelerated inference on Apple Silicon
- **System Tray App** - Minimal menu-bar presence with animated status glyphs
- **Global Hotkeys** - Hold Ctrl or double-tap Option to record
- **CLI Transcribe Command** - `codescribe transcribe` for batch audio processing
- **AI Formatting** - Optional post-processing via Ollama for text cleanup
- **Zero Cloud Dependency** - All processing happens locally, no API keys required

## Requirements

- **macOS 14+** (Sonoma or later)
- **Apple Silicon** (M1, M2, M3, or later)
- **Rust Toolchain** (1.85+ with edition 2024 support)

## Installation

### From Source (Recommended)

```bash
# Clone the repository
git clone https://github.com/VetCoders/CodeScribe.git
cd CodeScribe

# Install CLI to ~/.cargo/bin
make install

# Or build only
make build      # Debug build
make release    # Release build with optimizations
```

### Install as macOS App Bundle

```bash
# Create CodeScribe.app bundle
make bundle

# Install to /Applications
make install-app
```

## Usage

### System Tray Mode

Run `codescribe` to start the menu-bar app:

```bash
codescribe          # Start tray app (foreground)
codescribe -v       # Verbose logging mode
codescribe --config # Create/edit config file
```

Or use Make targets:

```bash
make start     # Start as background daemon
make stop      # Stop running instance
make restart   # Restart
make status    # Show process status
make logs      # View recent logs
```

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| **Hold Ctrl** | Hold-to-talk recording (800ms delay to prevent accidental triggers) |
| **Ctrl+Shift** (hold) | Assistive mode with AI formatting |
| **Double-tap Option** | Toggle recording on/off |

### CLI Transcription

Transcribe audio files directly without the tray app:

```bash
# Basic transcription
codescribe transcribe audio.wav

# With language hint
codescribe transcribe -l pl audio.m4a

# With AI formatting via Ollama
codescribe transcribe -f audio.mp3

# Specify model and LLM
codescribe transcribe -m whisper-large-v3-turbo-mlx-q8 --llm qwen3-coder:480b-cloud audio.wav
```

**Supported formats:** WAV, MP3, M4A

## Configuration

Configuration is stored in `~/.codescribe/`:

```
~/.codescribe/
  .env          # Primary configuration file
  models/       # Whisper model files
```

### Environment Variables

Create `~/.codescribe/.env` with:

```bash
# STT (Speech-to-Text)
WHISPER_LANGUAGE=auto          # Language: auto, en, pl, de, fr, etc.
LOCAL_MODEL=whisper-large-v3-turbo-mlx-q8

# Hotkeys
HOLD_MODS=ctrl                 # Hold key: ctrl, ctrl_alt, ctrl_shift
TOGGLE_TRIGGER=double_option   # Toggle: double_option, double_fn

# Audio
BEEP_ON_START=1                # Play sound when recording starts
SOUND_VOLUME=0.25              # 0.0 to 1.0

# AI Formatting (optional - requires Ollama)
AI_FORMATTING_ENABLED=0        # Enable AI post-processing
LLM_HOST=http://127.0.0.1:11434

# Logging
LOG_LEVEL=INFO                 # DEBUG, INFO, WARN, ERROR
```

Run `codescribe --config` to create a default config and open it in your editor.

## Model

CodeScribe uses **whisper-large-v3-turbo-mlx-q8** by default:

- 4-layer turbo architecture (vs 32 layers in full model)
- Q8 quantization for reduced memory footprint
- ~10x faster than whisper-large-v3
- Metal GPU acceleration via candle-core

Models are stored in `~/.CodeScribe/models/` or `./models/` in the repo.

### Model Files Required

```
whisper-large-v3-turbo-mlx-q8/
  config.json
  weights.safetensors
  tokenizer.json
  mel_filters.npz
```

## Building

```bash
# Development
make build          # Debug build
make lint           # Clippy + format check
make test           # Run tests
make check          # Full quality gate (lint + test)

# Production
make release        # Optimized release build
make bundle         # Create CodeScribe.app
make install-app    # Install to /Applications

# Tauri Frontend (optional)
make tauri-dev      # Start Tauri dev server
make tauri-build    # Build Tauri release
```

### Release Profile

The release build uses aggressive optimizations:

```toml
[profile.release]
opt-level = "z"     # Size optimization
lto = true          # Link-time optimization
codegen-units = 1   # Single codegen unit
panic = "abort"     # Minimal panic handling
strip = true        # Strip symbols
```

## Permissions

CodeScribe requires macOS permissions for:

- **Microphone** - Audio recording
- **Accessibility** - Global hotkey detection
- **Input Monitoring** - Keyboard event capture

Grant permissions in System Settings > Privacy & Security when prompted.

## Troubleshooting

**No audio / permissions dialog:**
Grant permissions in System Settings > Privacy & Security > Microphone and Accessibility.

**Multiple instances:**
```bash
make stop
rm ~/.codescribe/codescribe.pid
```

**Model not found:**
Ensure model files exist in `~/.CodeScribe/models/whisper-large-v3-turbo-mlx-q8/`.

**Quarantine issues (downloaded app):**
```bash
xattr -dr com.apple.quarantine /Applications/CodeScribe.app
```

## License

CodeScribe is distributed under the **Apache License 2.0**.

Copyright 2024-2026 Maciej Gad & VetCoders

See [LICENSE](LICENSE) for the full license text.

---

Created by M&K (c)2026 VetCoders
