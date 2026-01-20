# 10 - History and Files

CodeScribe stores all data in a single directory: `~/.codescribe/`. This guide explains the directory structure, file naming conventions, and how to manage your transcription history.

## Directory Structure

```
~/.codescribe/
├── .env                     # Configuration file
├── prompts/                 # Custom AI prompts
│   ├── formatting.txt       # Formatting prompt
│   ├── formatting_tuning.txt # Optional tuning additions
│   ├── assistive.txt        # Assistive mode prompt
│   └── assistive_tuning.txt # Optional tuning additions
└── transcriptions/          # All transcripts and audio
    ├── 2026-01-15/          # Date-based folders
    │   ├── 143052_hello-world_raw.txt
    │   ├── 143052_hello-world_raw.wav
    │   ├── 143055_hello-world_ai.txt
    │   └── ...
    ├── 2026-01-16/
    └── 2026-01-17/
```

## Transcription Files

Transcripts are saved to `~/.codescribe/transcriptions/YYYY-MM-DD/` with date-based subfolders.

### File Naming Convention

Each file follows the pattern: `HHMMSS_slug_kind.ext`

| Component | Description | Example |
|-----------|-------------|---------|
| HHMMSS | Time of recording (24h format) | `143052` |
| slug | First 3 words, ASCII-safe | `hello-world` |
| kind | Transcript type suffix | `raw`, `ai`, `ai-failed`, `failed` |
| ext | File extension | `.txt` or `.wav` |

### Transcript Kinds

- **`_raw`** - Direct Whisper output, unformatted
- **`_ai`** - Successfully formatted by AI
- **`_ai-failed`** - AI formatting failed, raw text saved
- **`_failed`** - Transcription itself failed

### Paired Files

When "Keep Audio" is enabled, audio files are saved alongside transcripts with matching names:

```
143052_hello-world_raw.txt   # Transcript
143052_hello-world_raw.wav   # Matching audio
```

This pairing makes it easy to review what was said versus what was transcribed.

## Audio Files

Audio recordings are 16-bit WAV files at 16kHz (Whisper's native sample rate). They are only saved when you enable **Keep Audio** in the tray menu.

To enable: **Tray Menu > Settings > Keep Audio**

Audio files can grow large over time. A typical 30-second recording is approximately 1 MB.

## Configuration File

The `.env` file stores all CodeScribe settings:

```
~/.codescribe/.env
```

This file is created automatically on first launch. You can edit it directly or use the tray menu to change settings. Settings include:

- Hotkey configuration (HOLD_MODS, TOGGLE_TRIGGER)
- AI formatting options (AI_FORMATTING_ENABLED, AI_PROVIDER)
- Backend URLs (STT_ENDPOINT, LLM_HOST)
- UI preferences (BEEP_ON_START, HOLD_INDICATOR)

## Custom Prompts

AI formatting uses prompts stored in `~/.codescribe/prompts/`:

| File | Purpose |
|------|---------|
| `formatting.txt` | Main formatting instructions |
| `formatting_tuning.txt` | Additional tuning (appended) |
| `assistive.txt` | Assistive mode instructions |
| `assistive_tuning.txt` | Additional tuning (appended) |

Default prompts are created automatically if missing. Edit them to customize how AI formats your transcriptions.

Access via: **Tray Menu > Prompts > Edit Formatting Prompt**

## Menu Shortcuts

Quick access to history and files:

| Menu Item | Action |
|-----------|--------|
| History > Copy Latest | Copy most recent transcript to clipboard |
| History > Open Folder | Open transcriptions folder in Finder |
| History > Save to History | Toggle automatic saving (on/off) |

## Backup and Restore

### Backup

Copy the entire `~/.codescribe/` directory:

```bash
cp -r ~/.codescribe ~/Desktop/codescribe-backup
```

### Restore

Replace the directory with your backup:

```bash
rm -rf ~/.codescribe
cp -r ~/Desktop/codescribe-backup ~/.codescribe
```

### Selective Backup

To backup only configuration (no transcripts):

```bash
mkdir ~/Desktop/codescribe-config
cp ~/.codescribe/.env ~/Desktop/codescribe-config/
cp -r ~/.codescribe/prompts ~/Desktop/codescribe-config/
```

## Cleaning Up Old Files

### Manual Cleanup

Delete old date folders directly:

```bash
rm -rf ~/.codescribe/transcriptions/2026-01-*
```

### Keep Last N Days

Remove transcriptions older than 30 days:

```bash
find ~/.codescribe/transcriptions -type d -mtime +30 -exec rm -rf {} +
```

### Audio-Only Cleanup

Remove audio files but keep transcripts:

```bash
find ~/.codescribe/transcriptions -name "*.wav" -delete
```

## Environment Variable Overrides

Advanced users can override the data directory:

| Variable | Purpose |
|----------|---------|
| `CODESCRIBE_DATA_DIR` | Override base directory |
| `CODESCRIBE_APP_DIR` | Alternative override |
| `CODESCRIBE_ENV_PATH` | Override .env file location |

Example:

```bash
export CODESCRIBE_DATA_DIR=~/Documents/CodeScribe
```

## Troubleshooting

**No transcriptions saved?**
- Check if "Save to History" is enabled in tray menu
- Verify `~/.codescribe/transcriptions/` exists and is writable

**Audio files missing?**
- Enable "Keep Audio" in Settings
- Check available disk space

**Prompts not loading?**
- Ensure `~/.codescribe/prompts/` exists
- Check file permissions

---

*Created by M&K (c)2026 VetCoders*
