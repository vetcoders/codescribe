# 02 - Open the Box

This guide walks you through installing CodeScribe on your Mac and getting it ready
for first use.

## System Requirements

- **macOS 14 (Sonoma)** or later
- **Apple Silicon** (M1, M2, M3, M4) — Intel Macs are not supported
- **2 GB free disk space** for the binary with embedded model (~888 MB)
- **8 GB RAM minimum** (16 GB recommended)

## Quick Start (Pre-built Binary)

If CodeScribe is already installed:
- Open it from Applications, or
- Run `codescribe` in Terminal

When CodeScribe starts, you should see a small icon in the menu bar.

## Installing from Source

### Step 1: Install Prerequisites

```bash
# Install Xcode Command Line Tools
xcode-select --install

# Install Rust via rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Verify Rust version (requires 1.85+ for edition 2024)
rustc --version
```

### Step 2: Clone and Build

```bash
git clone https://github.com/VetCoders/CodeScribe.git
cd CodeScribe

# Download Whisper model (~894 MB)
make download-model

# Build and install (~888 MB binary with embedded model)
make install

# Verify installation
codescribe --version
```

The binary is installed to `~/.cargo/bin/codescribe`.

### Alternative Build Options

```bash
cargo build                   # Debug build (external model, faster compile)
cargo build --release         # Release build without installing
make install-no-embed         # Dev-only: install without embedding model
```

For builds without embedded model, set the path at runtime:
```bash
export CODESCRIBE_MODEL_PATH=./models/whisper-large-v3-turbo-mlx-q8
```

## Granting Permissions

CodeScribe requires three macOS permissions:

| Permission | Purpose | Location |
|------------|---------|----------|
| Microphone | Audio recording | System Settings > Privacy & Security > Microphone |
| Accessibility | Global hotkeys | System Settings > Privacy & Security > Accessibility |
| Input Monitoring | Keyboard events | System Settings > Privacy & Security > Input Monitoring |

macOS prompts automatically on first use. Add Terminal or CodeScribe.app if needed.

## First-Time Setup

### Create Configuration

```bash
make config
```

This creates `~/.codescribe/.env`. Minimum configuration:

```env
WHISPER_LANGUAGE=en           # pl, en, de, fr (no auto-detect)
AI_FORMATTING_ENABLED=0       # Set to 1 to enable LLM formatting
```

### Test Recording

1. Run `codescribe`
2. Hold **Ctrl** for 800ms to start recording
3. Speak into your microphone
4. Release **Ctrl** to transcribe and paste

## Verifying Installation

```bash
codescribe --version                    # Check version
codescribe transcribe test.wav          # Test CLI transcription
```

## Troubleshooting

**"Model not found" Error**
```bash
make download-model && make install
```

**Build fails with "edition 2024" error**
```bash
rustup update stable    # Requires Rust 1.85+
```

**Hotkeys not working**
Add your terminal to System Settings > Privacy & Security > Accessibility.

**No audio recording**
Enable microphone access in System Settings > Privacy & Security > Microphone.

**Binary too large (~888 MB)**
This is expected. The Whisper model is embedded for zero-dependency deployment.

## File Locations

| Path | Purpose |
|------|---------|
| `~/.cargo/bin/codescribe` | Installed binary |
| `~/.codescribe/.env` | Configuration file |
| `~/.codescribe/history/` | Saved transcripts |
| `~/.codescribe/audio/` | Audio logs (if enabled) |
| `/tmp/codescribe.log` | Runtime logs |

## Next Steps

- Configure AI formatting: see [03 - Configuration](03-configuration.md)
- Learn hotkey modes: see [04 - Recording Modes](04-recording-modes.md)
- Explore CLI commands: see [05 - CLI Reference](05-cli-reference.md)

---

*Created by M&K (c)2026 VetCoders*
