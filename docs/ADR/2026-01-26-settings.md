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
LLM_ENDPOINT=https://api.openai.com/v1
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

CodeScribe uses **Responses API** (`/v1/responses`) with SSE streaming. You can configure separate providers for Formatting vs Assistive modes.

#### Shared Settings (fallback for both modes)

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `LLM_ENDPOINT` | URL | (none) | API endpoint (must be `/v1/responses`) |
| `LLM_API_KEY` | string | (none) | API key for authentication |
| `LLM_MODEL` | string | gpt-4.1-mini | Model to use |
| `AI_FORMATTING_ENABLED` | 0/1 | 0 | Enable AI post-processing |
| `LLM_USE_STREAMING` | 0/1 | 1 | Stream AI responses |

#### Mode-Specific Settings (override shared)

| Variable | Mode | Description |
|----------|------|-------------|
| `LLM_FORMATTING_ENDPOINT` | Formatting | Endpoint for text cleanup (cheap model) |
| `LLM_FORMATTING_MODEL` | Formatting | Model for formatting |
| `LLM_FORMATTING_API_KEY` | Formatting | API key for formatting provider |
| `LLM_ASSISTIVE_ENDPOINT` | Assistive | Endpoint for AI assistant (smart model) |
| `LLM_ASSISTIVE_MODEL` | Assistive | Model for assistive mode |
| `LLM_ASSISTIVE_API_KEY` | Assistive | API key for assistive provider |

> **Important**: Use `/v1/responses` endpoint, NOT `/v1/chat/completions`. CodeScribe uses OpenAI Responses API with `previous_response_id` for conversation chaining.

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

### Voice Activity Detection (VAD)

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `CODESCRIBE_VAD_SILENCE_DB` | -60 to -20 | -45 | Silence threshold in dB |
| `CODESCRIBE_VAD_SILENCE_SEC` | 0.5-5.0 | 1.5 | Silence duration before utterance flush |
| `CODESCRIBE_VAD_PRE_ROLL_MS` | 0-1000 | 300 | Audio to keep before speech detected |

### Streaming Transcription

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `CODESCRIBE_BUFFERED_STREAM` | 0/1 | 0 | Buffer before sending (reduces churn) |
| `CODESCRIBE_STREAM_CHUNK_SEC` | 1-10 | 4 | Chunk size for streaming |
| `CODESCRIBE_STREAM_OVERLAP_RATIO` | 0-0.5 | 0.2 | Overlap between chunks |
| `CODESCRIBE_BUFFER_DELAY_MS` | 0-3000 | 1500 | Delay before sending buffer |
| `CODESCRIBE_TYPING_CPS` | 10-100 | 35 | Typing speed (chars per second) |
| `CODESCRIBE_STREAM_SIMILARITY` | 0.8-1.0 | 0.92 | Embedding similarity threshold |
| `CODESCRIBE_STREAM_NOVELTY` | 0-0.5 | 0.18 | Minimum novelty for update |
| `CODESCRIBE_STREAM_DISABLE_EMBEDDINGS` | 0/1 | 0 | Disable embedding dedup |

### Overlay / UI

| Variable | Values | Default | Description |
|----------|--------|---------|-------------|
| `OVERLAY_POSITION_MODE` | snapped_* | snapped_top_right | Overlay position |
| `SHOW_TRAY_GLYPH` | 0/1 | 1 | Show status dot on tray icon |
| `HOLD_INDICATOR` | 0/1 | 1 | Show hold badge |
| `HOLD_BADGE_SIZE` | 4-16 | 8 | Badge size in pixels |

---

## AI Provider Setup

### OpenAI (Recommended)

```bash
# Single provider for both modes
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_API_KEY=sk-proj-your-key
LLM_MODEL=gpt-4.1-mini
```

### Separate Providers (Cost Optimization)

```bash
# Formatting mode: cheap/fast model
LLM_FORMATTING_ENDPOINT=http://localhost:8088/v1/responses
LLM_FORMATTING_MODEL=llama3.2
LLM_FORMATTING_API_KEY=local

# Assistive mode: smart model
LLM_ASSISTIVE_ENDPOINT=https://api.openai.com/v1/responses
LLM_ASSISTIVE_MODEL=gpt-5.2
LLM_ASSISTIVE_API_KEY=sk-proj-your-key
```

### Local Ollama (via OpenAI-compatible proxy)

```bash
# Requires Responses API proxy (e.g., openai-harmony)
LLM_ENDPOINT=http://localhost:8088/v1/responses
LLM_API_KEY=ollama
LLM_MODEL=llama3.2
```

### LibraxisAI Cloud

```bash
LLM_ENDPOINT=https://api.libraxis.cloud/v1/responses
LLM_API_KEY=your-libraxis-key
LLM_MODEL=qwen3-235b
```

> **Note**: Anthropic's API uses a different format. Use [openai-harmony](https://github.com/VetCoders/openai-harmony) proxy to convert to Responses API format.

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
