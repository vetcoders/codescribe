# 01 - What is CodeScribe?

CodeScribe is a lightweight menu bar application for macOS that turns your voice into text.
Press a hotkey, speak naturally, and your words appear wherever your cursor is — in any app.

No cloud services. No subscriptions. No data leaving your Mac.
Everything happens locally, on your own hardware.

---

## How It Works

The workflow is simple:

1. **Press a hotkey** (hold Ctrl or double-tap Option)
2. **Speak** into your microphone
3. **Release** the hotkey (or tap again to stop)
4. **Text appears** where your cursor was — ready to use

Behind the scenes, CodeScribe uses OpenAI's Whisper speech recognition model,
running entirely on your Mac with Metal GPU acceleration.
Transcription happens in real-time as you speak, so when you stop recording,
the text appears almost instantly.

---

## Key Features

### Local-First Privacy

Your voice never leaves your computer. The Whisper model is embedded directly
in the application (~888MB), requiring no internet connection and no external
file downloads after installation.

### Real-Time Transcription

CodeScribe uses "Whisper Live" streaming technology. As you speak, audio is
transcribed in small chunks. This means there's no waiting after you finish
recording — results appear immediately.

### Optional AI Formatting

Raw speech-to-text can be messy. CodeScribe optionally cleans up your
transcriptions using AI:

- Fixes punctuation and capitalization
- Removes filler words ("um", "uh", "like")
- Structures text into paragraphs
- Preserves your original language

You can configure different AI providers for different modes — a fast,
cheap model for basic cleanup, or a powerful model for assistive tasks.

### Flexible Recording Modes

- **Hold-to-talk**: Hold Ctrl to record, release to transcribe
- **Assistive mode**: Hold Ctrl+Shift for AI-augmented transcription
- **Toggle mode**: Double-tap Option to start/stop recording

### System Tray Integration

CodeScribe lives quietly in your menu bar. A small icon shows recording status,
and a dropdown menu gives you access to history, settings, and modes.

---

## Who Is CodeScribe For?

### Developers and Writers

Type faster by speaking. Draft documentation, write commit messages,
compose emails, or capture ideas without touching the keyboard.

### Privacy-Conscious Users

If you're uncomfortable sending voice recordings to cloud services,
CodeScribe keeps everything on your machine. Your voice data stays private.

### Anyone Who Types a Lot

Whether you're dealing with repetitive strain, prefer dictation,
or just want a faster way to get words on screen — CodeScribe helps.

### People Who Want Simplicity

No accounts. No sign-ups. No monthly fees. Install it, configure your
hotkey, and start speaking.

---

## How CodeScribe Differs from Cloud Services

| Feature | CodeScribe | Cloud Services |
|---------|------------|----------------|
| Privacy | 100% local | Voice sent to servers |
| Internet | Not required | Required |
| Subscription | None | Usually monthly |
| Latency | Near-instant | Network dependent |
| Data ownership | You own it | Provider may store |

Cloud services like Google's voice typing or Apple Dictation send your
audio to remote servers for processing. CodeScribe processes everything
locally using Apple Silicon's Metal GPU, giving you both privacy and speed.

---

## System Requirements

- **macOS 14+** (Sonoma or later)
- **Apple Silicon** (M1, M2, M3, M4, or later)
- **~1GB disk space** for the application
- **Microphone** (built-in or external)

CodeScribe uses Metal GPU acceleration, which is only available on Apple
Silicon Macs. Intel Macs are not supported.

---

## Architecture Overview

CodeScribe consists of two main components:

### CLI Daemon

The core application runs as a background daemon with a system tray icon.
It handles:

- Global hotkey detection
- Audio recording
- Whisper transcription
- AI formatting (optional)
- Clipboard operations

### Optional GUI (Tauri App)

A graphical interface built with Tauri provides:

- Voice Lab for testing and tuning
- Settings management
- History browser

Most users only need the tray app. The GUI is available for those who
prefer visual configuration.

---

## What's Next?

Continue to the next chapter to learn how to install CodeScribe on your Mac.

---

*Created by M&K (c)2026 VetCoders*
