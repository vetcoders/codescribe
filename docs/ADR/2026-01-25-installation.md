# Installation

## System Requirements

| Requirement | Minimum                           | Recommended              |
| ----------- | --------------------------------- | ------------------------ |
| macOS       | 13.0 (Ventura)                    | 14.0+ (Sonoma)           |
| Chip        | Apple Silicon or Intel with Metal | Apple Silicon (M1/M2/M3) |
| RAM         | 8 GB                              | 16 GB                    |
| Disk        | 1 GB                              | 2 GB                     |

---

## Installation Methods

### Option 1: Homebrew (Recommended)

```bash
brew install --cask codescribe
```

### Option 2: Direct Download

1. Go to [Releases](https://github.com/vetcoders/codescribe/releases)
2. Download `Codescribe-x.x.x.dmg`
3. Open DMG, drag Codescribe to Applications
4. Eject DMG

### Option 3: Build from Source

```bash
git clone https://github.com/vetcoders/codescribe.git
cd Codescribe
make download-model   # Download Whisper model (~888MB)
make release          # Build with embedded model
make install          # Install to /usr/local/bin
```

---

## First Launch

1. **Open Codescribe** from Applications or Spotlight
2. **Grant Microphone access** when prompted
3. **Grant Accessibility access** in System Settings → Privacy & Security → Accessibility
4. **Wait for initialization** (first launch may take 5-10 seconds to load Whisper model)

You'll see the Codescribe icon appear in your menu bar. It starts black (idle).

---

## Required Permissions

Codescribe needs these permissions to function:

| Permission           | Why                        | How to Grant                                            |
| -------------------- | -------------------------- | ------------------------------------------------------- |
| **Microphone**       | Record your speech         | System Settings → Privacy & Security → Microphone       |
| **Accessibility**    | Global hotkeys, paste text | System Settings → Privacy & Security → Accessibility    |
| **Input Monitoring** | Detect modifier keys       | System Settings → Privacy & Security → Input Monitoring |

> **Tip**: If hotkeys don't work, check that Codescribe is enabled in all three permission categories.

---

## Verify Installation

Open Terminal and run:

```bash
codescribe --version
```

Expected output:

```
Codescribe 0.7.x
```

Test transcription:

```bash
# Record 5 seconds of audio and transcribe
codescribe transcribe --record 5
```

---

## Configuration Location

Codescribe stores configuration in:

```
~/.codescribe/
├── .env              # Configuration file
├── prompts/
│   ├── formatting.txt   # AI formatting prompt
│   └── assistive.txt    # Assistive mode prompt
├── transcriptions/   # Saved transcripts
└── logs/             # Debug logs
```

Create default config:

```bash
codescribe --config
```

---

## Updating

### Homebrew

```bash
brew upgrade codescribe
```

### Manual

Download new version from Releases and replace the old app.

---

## Uninstalling

### Homebrew

```bash
brew uninstall codescribe
```

### Manual

1. Delete `/Applications/Codescribe.app`
2. Optionally remove config: `rm -rf ~/.codescribe`

---

_Copyright © 2024–2026 Vetcoders_
