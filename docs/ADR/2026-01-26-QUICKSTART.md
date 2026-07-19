# Codescribe Quick Start Guide

Get Codescribe running in 5 minutes.

---

## 1. Install

### Option A: From Source (Developers)

```bash
# Clone and enter directory
git clone https://github.com/vetcoders/codescribe.git
cd Codescribe

# Download required models (~2GB total)
make download-model    # Whisper STT (~888MB)
make download-e5       # E5 embedder (~1.2GB, optional)

# Build and install
make install           # Binary to ~/.cargo/bin/codescribe
make bundle            # Create Codescribe.app
make install-app       # Copy to /Applications
```

### Option B: From Release (Users)

1. Download `Codescribe_x.x.x.dmg` from [Releases](https://github.com/vetcoders/codescribe/releases)
2. Open DMG, drag to Applications
3. Open Codescribe from Applications

---

## 2. Grant Permissions

On first launch, grant these permissions in **System Settings → Privacy & Security**:

| Permission           | Location                   | Why                  |
| -------------------- | -------------------------- | -------------------- |
| **Microphone**       | Privacy → Microphone       | Record speech        |
| **Accessibility**    | Privacy → Accessibility    | Global hotkeys       |
| **Input Monitoring** | Privacy → Input Monitoring | Detect modifier keys |

> **Tip**: Restart Codescribe after granting permissions.

---

## 3. Configure

Edit `~/.codescribe/.env` or use the menu:

```bash
# Open config in editor
codescribe --config
```

### Essential Settings

```env
# Language (REQUIRED - no auto-detect!)
WHISPER_LANGUAGE=pl                    # pl | en | de | fr | es

# Hotkeys
HOLD_MODS=ctrl                         # ctrl | ctrl_alt | ctrl_shift
TOGGLE_TRIGGER=double_option           # double_option | none

# AI Formatting (optional but recommended)
AI_FORMATTING_ENABLED=1
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
LLM_API_KEY=sk-your-key-here
```

---

## 4. Use

### Recording Modes

| Mode          | Hotkey              | What It Does                          |
| ------------- | ------------------- | ------------------------------------- |
| **Raw**       | Hold `Ctrl`         | Fastest, no AI, raw Whisper output    |
| **Assistive** | Hold `Ctrl+Shift`   | AI-enhanced, expands/improves text    |
| **Toggle**    | Double-tap `Option` | Hands-free, ends utterance on silence |

### Visual Feedback

Look at the menu bar icon:

| Icon State | Meaning      |
| ---------- | ------------ |
| Green dot  | Ready (idle) |
| Red dot    | Recording    |
| Orange dot | Processing   |
| Red X      | Error        |

---

## 5. Verify

```bash
# Check version
codescribe --version

# Check status
make status

# View logs
make logs
```

---

## Troubleshooting

### Hotkeys don't work

1. Check all three permissions are granted
2. Restart Codescribe
3. Try `codescribe -v` for verbose logging

### No transcription

1. Check microphone permission
2. Verify `WHISPER_LANGUAGE` is set (not empty!)
3. Check logs: `make logs`

### Error icon appears

```bash
# Check logs for details
tail -50 /tmp/codescribe.log
```

---

## Next Steps

- [Modes Guide](modes.md) - Deep dive into recording modes
- [Settings Reference](settings.md) - All configuration options
- [Troubleshooting](troubleshooting.md) - Common issues

---

_Copyright © 2024–2026 Vetcoders_
