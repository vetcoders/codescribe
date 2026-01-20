# 12 - Privacy and Safety

CodeScribe is designed with privacy as a core principle. This chapter explains exactly what data stays on your Mac, what (if anything) leaves it, and how to configure the app for maximum privacy.

## Local-First Architecture

By default, CodeScribe processes your voice entirely on your Mac:

- **Speech-to-text runs locally** using the embedded Whisper model
- **No audio is sent to the cloud** unless you explicitly configure cloud STT
- **No account required** to use core transcription features
- **No telemetry or analytics** - we do not collect usage data

Your voice recordings are processed in memory and never leave your machine unless you enable optional cloud features.

## What Data Stays on Your Mac

All of the following remain strictly local:

| Data Type | Location | Purpose |
|-----------|----------|---------|
| Configuration | `~/.codescribe/.env` | Your settings and preferences |
| Transcription history | `~/.codescribe/transcriptions/` | Saved transcripts (if history enabled) |
| Audio recordings | `~/.codescribe/transcriptions/` | Paired WAV files (if history enabled) |
| Custom prompts | `~/.codescribe/prompts/` | Your AI formatting instructions |
| Whisper model | App bundle or `~/.codescribe/models/` | Local speech recognition |
| IPC socket | `~/.codescribe/codescribe.sock` | Internal app communication |

The IPC socket uses Unix domain sockets with strict permissions (mode 0600) - only your user account can connect. The app also verifies the connecting process belongs to you via peer UID checks.

## When Data May Leave Your Mac

Data is only transmitted externally if you explicitly enable these optional features:

### AI Formatting (Optional)

When AI formatting is enabled, your **transcribed text** (not audio) may be sent to:

- **Ollama (local)**: Runs entirely on your Mac. No network transmission.
- **Harmony/Libraxis Cloud**: Text sent to `api.libraxis.cloud` over HTTPS.
- **Custom LLM endpoint**: Text sent to your configured `LLM_ENDPOINT`.

To enable: Settings > AI Formatting > Enable

### Cloud STT (Optional)

If you disable local Whisper and configure a cloud STT endpoint:

- Audio is sent to your configured `STT_ENDPOINT` over HTTPS
- This is disabled by default

To enable: Set `USE_LOCAL_STT=0` and configure `STT_ENDPOINT` in settings.

## AI Provider Privacy Comparison

| Provider | Data Location | Privacy Level | Notes |
|----------|---------------|---------------|-------|
| **None** (AI off) | Local only | Maximum | Raw transcription only |
| **Ollama** | Local only | Maximum | Runs on your Mac |
| **Harmony** | Cloud (Libraxis) | High | European servers, no data retention |
| **Custom LLM** | Depends on endpoint | Varies | Check your provider's policy |

For maximum privacy, use either no AI formatting or local Ollama.

## System Permissions

CodeScribe requests two macOS permissions:

### Microphone Access

- **Purpose**: Record your voice for transcription
- **Scope**: Only active during recording (hold key or toggle mode)
- **Stored**: Temporary buffer in memory; optionally saved to history folder
- **Grant in**: System Settings > Privacy & Security > Microphone

### Accessibility Access

- **Purpose**: Detect global hotkeys (Ctrl hold, Option double-tap)
- **Scope**: Key event monitoring only; no keylogging or screen capture
- **Stored**: Nothing - events processed in real-time
- **Grant in**: System Settings > Privacy & Security > Accessibility

CodeScribe does not request screen recording, contacts, calendar, or any other permissions.

## API Keys and Credentials

If you use cloud AI features:

- API keys are stored locally in `~/.codescribe/.env`
- Keys are never logged or transmitted except to authenticate with your provider
- Keys are redacted when displayed in the settings UI
- The `.env` file has restricted permissions (user-only read/write)

## Data Retention

CodeScribe stores data only if you enable history:

- **History disabled**: Transcripts exist only in clipboard, never written to disk
- **History enabled**: Transcripts saved to `~/.codescribe/transcriptions/YYYY-MM-DD/`
- **Audio pairing**: If enabled, WAV files saved alongside transcripts

To clear history: Menu > Open History Folder > delete files manually, or use the Clear History option.

## How to Ensure Maximum Privacy

For the most private configuration:

1. **Keep AI Formatting disabled** (default)
2. **Use local Whisper** (default) - never enable cloud STT
3. **Disable history** if you do not need transcript archives
4. **Do not configure** `LLM_ENDPOINT`, `STT_ENDPOINT`, or API keys

With these settings, CodeScribe operates entirely offline. Your voice goes in, text comes out to your clipboard, and nothing is stored or transmitted.

## Network Connections Summary

| Feature | Network Required | Default State |
|---------|------------------|---------------|
| Local transcription | No | Enabled |
| AI Formatting (Ollama) | No | Disabled |
| AI Formatting (Cloud) | Yes (HTTPS) | Disabled |
| Cloud STT | Yes (HTTPS) | Disabled |
| App updates | No | Manual only |

## Security Considerations

- The app is code-signed and notarized by Apple
- IPC uses Unix sockets with peer authentication
- All cloud communication uses TLS/HTTPS
- No remote code execution or plugin system
- Open source: audit the code yourself if desired

## Questions?

If you have privacy concerns not addressed here, please open an issue on GitHub or contact us directly. We are committed to transparency about how CodeScribe handles your data.

---

*Created by M&K (c)2026 VetCoders*
