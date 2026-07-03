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

1. Go to [Releases](https://github.com/vetcoders/codescribe/releases)
2. Download `Codescribe-x.x.x.dmg`
3. Open DMG, drag Codescribe to Applications
4. Eject DMG

> If Releases is empty, use the source path below. The repository now ships a release workflow, but not every branch has a published tag yet.

### Option 2: Build from Source

```bash
git clone https://github.com/vetcoders/codescribe.git
cd codescribe
make app PROFILE=release   # Build Codescribe.app
make install-app      # Build + copy app to /Applications
```

---

## First Launch

1. **Open Codescribe** from Applications or Spotlight
2. **Grant Microphone access** when prompted
3. **Grant Accessibility access** in System Settings → Privacy & Security → Accessibility
4. **Wait for initialization** (first launch may take a few seconds to resolve and load the local Whisper model)

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
make version
```

Expected output:

```
v0.12.x
```

Test transcription:

1. Launch Codescribe.
2. Start a short Dictation capture with your configured shortcut.
3. Check `make logs` if the menu-bar state reports an error.

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
make config
```

---

## Updating

### Manual / Releases

Download new version from Releases and replace the old app.

---

## Uninstalling

### Manual

1. Delete `/Applications/Codescribe.app`
2. Optionally remove config: `rm -rf ~/.codescribe`

---

_Created by vetcoders (c)2026_
