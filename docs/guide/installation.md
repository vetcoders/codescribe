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

### Option 1: Direct Download (Preferred for end users once a release is published)

1. Go to [Releases](https://github.com/VetCoders/CodeScribe/releases)
2. Download `CodeScribe-x.x.x.dmg`
3. Open DMG, drag CodeScribe to Applications
4. Eject DMG

> If Releases is empty, use the source path below. The repository now ships a release workflow, but not every branch has a published tag yet.

### Option 2: Build from Source

```bash
git clone https://github.com/VetCoders/CodeScribe.git
cd CodeScribe
make download-model   # Download Whisper model (~888MB)
make release          # Build with embedded model
make install          # Install to /usr/local/bin
```

---

## First Launch

1. **Open CodeScribe** from Applications or Spotlight
2. **Grant Microphone access** when prompted
3. **Grant Accessibility access** in System Settings → Privacy & Security → Accessibility
4. **Wait for initialization** (first launch may take 5-10 seconds to load Whisper model)

You'll see the CodeScribe icon appear in your menu bar. It starts black (idle).

---

## Required Permissions

CodeScribe needs these permissions to function:

| Permission           | Why                        | How to Grant                                            |
| -------------------- | -------------------------- | ------------------------------------------------------- |
| **Microphone**       | Record your speech         | System Settings → Privacy & Security → Microphone       |
| **Accessibility**    | Global hotkeys, paste text | System Settings → Privacy & Security → Accessibility    |
| **Input Monitoring** | Detect modifier keys       | System Settings → Privacy & Security → Input Monitoring |

> **Tip**: If hotkeys don't work, check that CodeScribe is enabled in all three permission categories.

---

## Verify Installation

Open Terminal and run:

```bash
codescribe --version
```

Expected output:

```
CodeScribe 0.7.x
```

Test transcription:

```bash
# Record 5 seconds of audio and transcribe
codescribe transcribe --record 5
```

---

## Configuration Location

CodeScribe stores configuration in:

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

### Manual / Releases

Download new version from Releases and replace the old app.

---

## Uninstalling

### Manual

1. Delete `/Applications/CodeScribe.app`
2. Optionally remove config: `rm -rf ~/.codescribe`

---

_Created by M&K (c)2026 VetCoders_
