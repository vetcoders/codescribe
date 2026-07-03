# Codescribe Quick Start Guide

Get codescribe running in 5 minutes.

---

## 1. Install

### Option A: From Source (Developers)

```bash
# Clone and enter directory
git clone https://github.com/vetcoders/codescribe.git
cd codescribe

# Build and install the SwiftUI app
make app PROFILE=release
make install-app       # Build + copy Codescribe.app to /Applications
```

### Option B: From Release (Users)

1. Download `Codescribe_x.x.x.dmg` from [Releases](https://github.com/vetcoders/codescribe/releases)
2. Open DMG, drag to Applications
3. Open Codescribe from Applications

> If Releases is empty for the branch you are on, fall back to Option A and build locally.

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

Recommended: configure Codescribe in the **Settings** window.

```bash
# Menu bar icon → Settings
# or: edit ~/.codescribe/.env (power-user overrides)
```

### Essential Settings

- **Audio & Input**
  - Set `Whisper language` (`Auto-detect / multilingual` is the default; pick Polish or English only to force a language)
  - Toggle **AI Formatting** for Dictation (optional)
- **Modes & Shortcuts**
  - Dictation: hold a modifier (default: `Fn/Globe`)
  - Formatting: double‑tap `Left Option`
  - Assistive (Agent): double‑tap `Right Option`
- **AI & Prompts**
  - Configure providers for **Formatting** and **Assistive**
  - API keys are stored in macOS Keychain

Power-user overrides still exist via `~/.codescribe/.env`, but you should not need it to get started.

---

## 4. Use

### Recording Modes

| Mode                  | Shortcut                    | What It Does                                 |
| --------------------- | --------------------------- | -------------------------------------------- |
| **Dictation**         | Hold your Dictation binding | Fast dictation, auto‑paste (AI optional)     |
| **Formatting**        | Double‑tap `Left Option`    | Hands‑free + AI formatting pass (auto‑paste) |
| **Assistive (Agent)** | Double‑tap `Right Option`   | Agent chat overlay (auto‑paste OFF)          |

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
make version

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
3. Check `make logs` for recent app output

### No transcription

1. Check microphone permission
2. Leave `WHISPER_LANGUAGE` unset or set it to `auto` unless you need to force `pl` or `en`
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
