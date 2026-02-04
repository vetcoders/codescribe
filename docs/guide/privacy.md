# Privacy & Security

CodeScribe is designed with privacy as a core principle. Your audio is processed locally by default.

---

## Privacy Summary

| Data            | Where It Goes        | Your Control                             |
| --------------- | -------------------- | ---------------------------------------- |
| **Audio**       | Your Mac only        | Never leaves unless you enable cloud STT |
| **Transcripts** | Your Mac only        | Stored in ~/.codescribe/transcriptions/  |
| **AI requests** | Your chosen provider | Only if AI formatting enabled            |
| **Telemetry**   | Nowhere              | No tracking, no analytics, no phone-home |

---

## What Stays Local

### Always Local (Cannot Be Changed)

| Component       | Location                      | Notes                       |
| --------------- | ----------------------------- | --------------------------- |
| Whisper model   | Embedded in binary            | ~888MB, runs on Metal GPU   |
| Audio recording | RAM only                      | Deleted after transcription |
| Transcripts     | ~/.codescribe/transcriptions/ | You control retention       |
| Configuration   | ~/.codescribe/.env            | Plain text, editable        |
| Prompts         | ~/.codescribe/prompts/        | Your custom prompts         |

### No Network Required For

- Recording audio
- Running Whisper transcription
- Pasting text to applications
- Storing transcripts
- All hotkey operations

**CodeScribe works completely offline in Raw mode.**

---

## What Can Leave Your Mac

### Optional: AI Formatting

**When enabled** (`AI_FORMATTING_ENABLED=1`):

- Transcribed text is sent to your configured AI provider
- This is the text, not the audio
- Uses HTTPS encryption

**Data sent to AI**:

```
- Your transcript text
- System prompt (formatting/assistive)
- Language hint
```

**Not sent**:

```
- Audio files
- File paths
- System information
- Other transcripts
- Your API key (sent as auth header only)
```

### Optional: Cloud STT

**When enabled** (for LibraxisAI users):

- Audio may be sent to cloud STT as fallback
- Only if local Whisper fails

**To disable**:

```bash
USE_LOCAL_STT=1
CODESCRIBE_QUALITY_DISABLE_CLOUD=1
```

---

## AI Provider Comparison

| Provider         | Data Retention                   | Privacy        |
| ---------------- | -------------------------------- | -------------- |
| **Local Ollama** | None (your machine)              | ★★★★★ Maximum  |
| **OpenAI**       | 30 days (API), opt-out available | ★★★☆☆ Standard |
| **Anthropic**    | 30 days (API), opt-out available | ★★★☆☆ Standard |
| **LibraxisAI**   | Custom (your instance)           | ★★★★☆ Good     |

### Maximum Privacy Configuration

For zero cloud communication:

```bash
# ~/.codescribe/.env

# Disable AI formatting
AI_FORMATTING_ENABLED=0

# Force local STT
USE_LOCAL_STT=1

# Disable cloud fallback
CODESCRIBE_QUALITY_DISABLE_CLOUD=1

# No LLM configuration needed
# LLM_ENDPOINT=
# LLM_API_KEY=
```

### Using Local Ollama

Run AI completely on your Mac:

```bash
# Install Ollama
brew install ollama

# Start Ollama
ollama serve

# Pull a model
ollama pull llama3.2

# Configure CodeScribe
LLM_ENDPOINT=http://localhost:11434/v1
LLM_API_KEY=ollama
LLM_MODEL=llama3.2
AI_FORMATTING_ENABLED=1
```

Now AI formatting runs 100% locally.

---

## System Permissions

CodeScribe requests these permissions:

| Permission           | Why                        | Risk Level                            |
| -------------------- | -------------------------- | ------------------------------------- |
| **Microphone**       | Record your speech         | Medium - only during recording        |
| **Accessibility**    | Detect hotkeys, paste text | Low - standard automation             |
| **Input Monitoring** | Detect modifier keys       | Low - only key states, not keystrokes |

### What We Don't Access

- ❌ Keylogger capability (we detect Ctrl/Shift states only)
- ❌ Screen recording
- ❌ Camera
- ❌ Location
- ❌ Contacts
- ❌ Files outside ~/.codescribe/

---

## Data Retention

### Transcripts

By default, transcripts are saved to `~/.codescribe/transcriptions/`:

```
~/.codescribe/transcriptions/
├── 2026-01-22/
│   ├── 143052_hello-world_raw.txt
│   ├── 143052_hello-world_ai.txt
│   └── 143200_meeting-notes_raw.txt
```

**To disable history**:

```bash
HISTORY_ENABLED=0
```

**To clear history**:

```bash
rm -rf ~/.codescribe/transcriptions/*
```

### Audio Files

Audio is NOT saved by default. To enable (for debugging):

```bash
DUMP_AUDIO_LOGS=1
```

Audio files go to `~/.codescribe/audio/`.

---

## Network Connections

### CodeScribe Makes No Connections If:

- AI formatting is disabled
- Using only Raw mode (Ctrl hold)
- No LLM_ENDPOINT configured

### CodeScribe Connects To:

| Destination    | When               | Data               |
| -------------- | ------------------ | ------------------ |
| `LLM_ENDPOINT` | AI formatting      | Text transcript    |
| `STT_ENDPOINT` | Cloud STT fallback | Audio (if enabled) |

### Verify Network Activity

```bash
# Monitor connections
sudo lsof -i -P | grep codescribe

# Check what's configured
cat ~/.codescribe/.env | grep -E "(URL|ENDPOINT)"
```

---

## API Key Storage

API keys are stored in `~/.codescribe/.env`:

```bash
# File permissions
ls -la ~/.codescribe/.env
# Should show: -rw------- (600)
```

**Secure your config**:

```bash
chmod 600 ~/.codescribe/.env
```

**Never commit .env to git** - it's in .gitignore by default.

---

## Open Source Transparency

CodeScribe is open source:

- **Repository**: github.com/VetCoders/CodeScribe
- **License**: BSD 4-Clause
- **Audit**: You can inspect all code

No hidden functionality, no obfuscation.

---

## Security Considerations

### Potential Risks

| Risk                         | Mitigation                                   |
| ---------------------------- | -------------------------------------------- |
| Sensitive audio recorded     | Only records when hotkey active              |
| Transcript stored insecurely | Files only readable by you (700 permissions) |
| API key exposure             | .env file with restricted permissions        |
| Man-in-middle on AI requests | HTTPS enforced                               |

### Recommendations

1. **Don't dictate passwords** - obvious but important
2. **Review transcripts** - clear sensitive ones periodically
3. **Use local AI** - Ollama for maximum privacy
4. **Secure .env** - check file permissions
5. **Lock screen** - prevent others from using your hotkeys

---

## GDPR / Data Subject Rights

For EU users:

- **Right to access**: All your data is in ~/.codescribe/
- **Right to erasure**: `rm -rf ~/.codescribe/`
- **Right to portability**: Files are plain text, easily exported
- **No third-party sharing**: Unless you configure AI providers

---

_Created by M&K (c)2026 VetCoders_
