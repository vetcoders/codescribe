# Settings & Configuration

CodeScribe is configured through environment variables stored in `~/.codescribe/.env`.

---

## Quick Access

### From Chat Overlay
1. Open Chat Overlay (menu bar → Show Chat Overlay)
2. Click **Settings** tab in right panel
3. Use quick toggles or **Edit Config** button

### From Terminal
```bash
codescribe --config   # Opens config file in default editor
```

### Direct Edit
```bash
nano ~/.codescribe/.env
```

---

## Configuration File

Default location: `~/.codescribe/.env`

Example configuration:

```bash
# ═══════════════════════════════════════════════════
# AI Provider Configuration
# ═══════════════════════════════════════════════════

# API endpoint for AI formatting
LLM_BASE_URL=https://api.openai.com/v1
LLM_API_KEY=sk-your-key-here
LLM_MODEL=gpt-4o-mini

# Enable AI formatting (1=on, 0=off)
AI_FORMATTING_ENABLED=1

# Use streaming responses (1=streaming, 0=batch)
LLM_USE_STREAMING=1

# ═══════════════════════════════════════════════════
# Transcription Settings
# ═══════════════════════════════════════════════════

# Language for Whisper (en, pl, de, etc.)
WHISPER_LANGUAGE=en

# Use local Whisper (1) or cloud STT (0)
USE_LOCAL_STT=1

# ═══════════════════════════════════════════════════
# Hotkey Configuration
# ═══════════════════════════════════════════════════

# Hold key modifiers: ctrl, ctrl_alt, ctrl_shift, ctrl_cmd
HOLD_MODS=ctrl

# Hold delay before recording starts (ms)
HOLD_START_DELAY_MS=800

# Toggle trigger: double_option, double_right_option, none
TOGGLE_TRIGGER=double_option

# Hold exclusive mode (1=block other Ctrl uses while held)
HOLD_EXCLUSIVE=0

# ═══════════════════════════════════════════════════
# Audio & Recording
# ═══════════════════════════════════════════════════

# Play beep when recording starts
BEEP_ON_START=1

# Save audio files alongside transcripts
DUMP_AUDIO_LOGS=0

# ═══════════════════════════════════════════════════
# History & Files
# ═══════════════════════════════════════════════════

# Enable transcript history
HISTORY_ENABLED=1

# Keep raw transcripts (for quality reports)
CODESCRIBE_QUALITY_DISABLE_RAW_SAVE=0
```

---

## Settings Reference

### AI Provider

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `LLM_BASE_URL` | URL | (none) | AI API endpoint |
| `LLM_API_KEY` | string | (none) | API key for authentication |
| `LLM_MODEL` | string | gpt-4o-mini | Model to use |
| `AI_FORMATTING_ENABLED` | 0/1 | 0 | Enable AI post-processing |
| `LLM_USE_STREAMING` | 0/1 | 1 | Stream AI responses |

### Transcription

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `WHISPER_LANGUAGE` | ISO code | en | Transcription language |
| `USE_LOCAL_STT` | 0/1 | 1 | Use embedded Whisper |

### Hotkeys

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `HOLD_MODS` | ctrl, ctrl_alt, ctrl_shift, ctrl_cmd | ctrl | Hold key combination |
| `HOLD_START_DELAY_MS` | 0-2000 | 800 | Delay before recording |
| `TOGGLE_TRIGGER` | double_option, double_right_option, none | double_option | Toggle hotkey |
| `HOLD_EXCLUSIVE` | 0/1 | 0 | Block other Ctrl uses |

### Audio

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `BEEP_ON_START` | 0/1 | 1 | Audio feedback on record |
| `DUMP_AUDIO_LOGS` | 0/1 | 0 | Save audio files |

### History

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `HISTORY_ENABLED` | 0/1 | 1 | Save transcripts |

---

## AI Provider Setup

### OpenAI

```bash
LLM_BASE_URL=https://api.openai.com/v1
LLM_API_KEY=sk-your-openai-key
LLM_MODEL=gpt-4o-mini
```

### Anthropic (Claude)

```bash
LLM_BASE_URL=https://api.anthropic.com/v1
LLM_API_KEY=sk-ant-your-anthropic-key
LLM_MODEL=claude-3-5-sonnet-20241022
```

### Local Ollama

```bash
LLM_BASE_URL=http://localhost:11434/v1
LLM_API_KEY=ollama
LLM_MODEL=llama3.2
```

### LibraxisAI (Custom)

```bash
LLM_BASE_URL=https://your-instance.libraxis.ai/v1
LLM_API_KEY=your-key
LLM_MODEL=qwen3-235b-a22b
```

---

## Custom Prompts

AI behavior is controlled by prompt files:

| File | Purpose | Used In |
|------|---------|---------|
| `~/.codescribe/prompts/formatting.txt` | Text cleanup/formatting | Toggle mode with AI |
| `~/.codescribe/prompts/assistive.txt` | Content expansion/help | Assistive mode (Ctrl+Shift) |

### Editing Prompts

```bash
# Edit formatting prompt
nano ~/.codescribe/prompts/formatting.txt

# Edit assistive prompt
nano ~/.codescribe/prompts/assistive.txt
```

Or use Chat Overlay → Settings → **Edit Prompt**.

### Default Prompts

**Formatting prompt** (cleanup only):
```
You are a transcription formatter. Clean up the text:
- Fix punctuation and capitalization
- Remove filler words (um, uh, like)
- Keep the original meaning intact
- Do not add or expand content
```

**Assistive prompt** (AI helper):
```
You are a helpful assistant. The user is speaking to you.
Respond helpfully and concisely.
You may expand, structure, or improve their request.
```

---

## Resetting Configuration

### Reset to Defaults

```bash
codescribe reset-defaults
```

Or delete and recreate:

```bash
rm ~/.codescribe/.env
codescribe --config
```

### Reset AI Context

From Chat Overlay → Settings → **Reset Context**

This clears the conversation history for the AI session.

---

## Environment Variable Loading

Variables are loaded from multiple sources (in order):

1. System environment variables
2. `~/.codescribe/.env` file
3. Command-line flags (override all)

```bash
# Override for single run
AI_FORMATTING_ENABLED=0 codescribe
```

---

## Language Codes

Common values for `WHISPER_LANGUAGE`:

| Code | Language |
|------|----------|
| en | English |
| pl | Polish |
| de | German |
| fr | French |
| es | Spanish |
| it | Italian |
| pt | Portuguese |
| nl | Dutch |
| ru | Russian |
| zh | Chinese |
| ja | Japanese |
| ko | Korean |

Full list: [Whisper supported languages](https://github.com/openai/whisper#available-models-and-languages)

---

*Created by M&K (c)2026 VetCoders*
