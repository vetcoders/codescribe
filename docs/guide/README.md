# CodeScribe User Guide

> **Speech-to-text for macOS with embedded Whisper model**

CodeScribe is a native macOS menu-bar application that transcribes your speech locally using an embedded Whisper model. No internet required for basic transcription. Optional AI formatting available via cloud providers.

---

## Quick Start (30 seconds)

1. **Install**: `brew install --cask codescribe` or download from [Releases](https://github.com/VetCoders/CodeScribe/releases)
2. **Launch**: Open CodeScribe from Applications
3. **Grant permissions**: Microphone + Accessibility (follow prompts)
4. **Transcribe**: Hold `Ctrl`, speak, release → text appears at cursor

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
- **Zero latency**: ~888MB model embedded in binary, no download needed
- **Live streaming**: See transcription appear as you speak
- **Three modes**: Raw (fast), Formatted (clean), Assistive (AI-powered)
- **Privacy**: Audio never leaves your machine unless you enable cloud AI

---

## Recording Modes at a Glance

| Hotkey            | Mode          | What It Does                                |
| ----------------- | ------------- | ------------------------------------------- |
| `Ctrl` hold       | **Raw**       | Fast dictation, no AI, text pasted directly |
| `Ctrl+Shift` hold | **Assistive** | AI expands/enhances your speech             |
| `Double Option`   | **Toggle**    | Hands-free, auto-stops on silence           |

---

## Menu Bar Icon

CodeScribe lives in your menu bar. The icon color indicates status:

| Color     | Meaning                      |
| --------- | ---------------------------- |
| ⚫ Black  | Idle, ready                  |
| 🔴 Red    | Recording (hold mode)        |
| 🟣 Purple | Recording (assistive mode)   |
| 🟠 Orange | Processing transcription     |
| 🟢 Green  | Success, text pasted         |
| ⚪ Gray   | Error or backend unavailable |

---

## System Requirements

- **macOS**: 13.0 (Ventura) or later
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
