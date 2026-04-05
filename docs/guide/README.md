# CodeScribe User Guide

> **Speech-to-text for macOS with local Whisper plus optional cloud transcript paths**

CodeScribe is a native macOS menu-bar application that transcribes your speech locally using Whisper. No internet is required for the local path. Optional cloud STT and AI formatting are available when you want a different final transcript backend.

---

## Quick Start (30 seconds)

1. **Install**: download a DMG from [Releases](https://github.com/VetCoders/CodeScribe/releases), or build from source if no tagged release is published yet
2. **Launch**: Open CodeScribe from Applications
3. **Grant permissions**: Microphone + Accessibility + Input Monitoring (follow prompts)
4. **Transcribe**: Use your **Dictation** hotkey (default: hold `Fn/Globe`), speak, release → text appears at cursor (Creator → Keys)

That's it. You're transcribing.

---

## Table of Contents

| Chapter                               | Description                                            |
| ------------------------------------- | ------------------------------------------------------ |
| [Installation](installation.md)       | System requirements, installation methods, permissions |
| [Recording Modes](modes.md)           | Hold-to-talk, toggle mode, assistive mode              |
| [Chat Overlay](chat-overlay.md)       | Voice Chat UI, drafts, AI responses                    |
| [Overlay UX (POC)](overlay-ux-poc.md) | Agent/Drawer UX notes for this branch                  |
| [Settings](settings.md)               | Configuration options, environment variables           |
| [Troubleshooting](troubleshooting.md) | Common issues and solutions                            |
| [Privacy & Security](privacy.md)      | What stays local, what goes to cloud                   |

---

## How It Works

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Your Voice │ ──► │   Whisper   │ ──► │    Text     │
│             │     │  (on Mac)   │     │  (pasted)   │
└─────────────┘     └─────────────┘     └─────────────┘
                           │
                           ▼ (optional)
                    ┌─────────────┐
                    │  AI Format  │
                    │  (cloud)    │
                    └─────────────┘
```

**Key Features:**

- **Local-first**: Whisper model runs entirely on your Mac (Metal GPU)
- **Embedded-first local path**: current shipped builds center on a local ~888MB Whisper model
- **Live streaming**: See transcription appear as you speak
- **Three modes**: Raw (fast), Formatted (clean), Assistive (AI-powered)
- **Privacy**: Audio never leaves your machine unless you enable cloud AI

---

## Recording Modes at a Glance

| Hotkey                                  | Mode                  | What It Does                                |
| --------------------------------------- | --------------------- | ------------------------------------------- |
| Hold `Fn/Globe` (default; configurable) | **Dictation**         | Fast dictation (AI optional), auto‑paste ON |
| Double‑tap `Left Option`                | **Formatting**        | AI formatting pass, auto‑paste ON           |
| Double‑tap `Right Option`               | **Assistive (Agent)** | Agent chat (uses selection when available)  |

---

## Menu Bar Icon

CodeScribe lives in your menu bar. The tray logo stays neutral and the status glyph indicates runtime state:

| Glyph     | Meaning                      |
| --------- | ---------------------------- |
| 🟢 Green  | Idle, ready                  |
| 🔴 Red    | Recording                    |
| 🟠 Orange | Processing transcription     |
| 🟢 Green  | Success, text pasted         |
| ❌ Red X  | Error or backend unavailable |

---

## System Requirements

- **macOS**: 14.0 (Sonoma) or later
- **Chip**: Apple Silicon (M1/M2/M3/M4/M5)
- **RAM**: 8GB minimum, 16GB recommended
- **Disk**: ~1GB for app with embedded model

---

## Getting Help

- **GitHub Issues**: [github.com/VetCoders/CodeScribe/issues](https://github.com/VetCoders/CodeScribe/issues)
- **Documentation**: This guide + [ARCHITECTURE.md](../ARCHITECTURE.md)
- **Logs**: `~/.codescribe/logs/` or run `codescribe -v` for verbose output

---

_Created by M&K (c)2026 VetCoders_
