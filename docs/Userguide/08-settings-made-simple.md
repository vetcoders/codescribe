# 08 - Settings Made Simple

CodeScribe stores all configuration in a single `.env` file. You can modify settings
through the GUI or by editing the file directly.

## Configuration File Location

Your settings live in:

```
~/.codescribe/.env
```

You can override this location with the `CODESCRIBE_ENV_PATH` environment variable.
The config directory itself can be changed via `CODESCRIBE_DATA_DIR`.

## GUI vs File-Based Configuration

**GUI (recommended for most users):**
Open Settings from the menu bar icon. Changes are saved immediately when you click Save.

**File-based (power users):**
Edit `~/.codescribe/.env` directly. The app reads this file on startup. For changes
to take effect, restart CodeScribe or use the "Reload Config" option in the menu.

## Settings Reference

### Hotkey Settings

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Hold Modifier | `HOLD_MODS` | `ctrl` | Modifier keys for hold-to-talk: `ctrl`, `ctrl_alt`, `ctrl_shift`, `ctrl_cmd` |
| Hold Exclusive | `HOLD_EXCLUSIVE` | `true` | When enabled, Ctrl+K won't trigger recording (ignores extra modifiers) |
| Toggle Trigger | `TOGGLE_TRIGGER` | `double_option` | Toggle mode: `double_option`, `double_ralt`, or `none` |
| Hold Start Delay | `HOLD_START_DELAY_MS` | `800` | Milliseconds before recording starts after holding key |

### Language Settings

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Whisper Language | `WHISPER_LANGUAGE` | `pl` | Transcription language: `pl` (Polish) or `en` (English) |

### AI Formatting

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| AI Formatting | `AI_FORMATTING_ENABLED` | `false` | Enable AI post-processing of transcriptions |
| AI Provider | `AI_PROVIDER` | `harmony` | Provider: `harmony` or `ollama` |
| Max Tokens | `AI_MAX_TOKENS` | `0` | Token limit for AI (0 = no limit) |
| Assistive Max Tokens | `AI_ASSISTIVE_MAX_TOKENS` | `0` | Token limit for assistive mode |

### Audio Input

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Input Device | `AUDIO_INPUT_DEVICE` | (system default) | Preferred microphone name (leave empty for system default) |

### UI Settings

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Show Tray Glyph | `SHOW_TRAY_GLYPH` | `true` | Show icon in menu bar |
| Hold Indicator | `HOLD_INDICATOR` | `true` | Show visual indicator when recording |
| Badge Size | `HOLD_BADGE_SIZE` | `12` | Size of hold indicator in pixels (8-64) |
| Badge Offset X | `HOLD_BADGE_OFFSET_X` | `10` | Horizontal badge position offset |
| Badge Offset Y | `HOLD_BADGE_OFFSET_Y` | `-10` | Vertical badge position offset |

### Sound Settings

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Beep on Start | `BEEP_ON_START` | `true` | Play sound when recording starts |
| Sound Name | `SOUND_NAME` | `Tink` | macOS system sound (e.g., Tink, Pop, Basso) |
| Sound Volume | `SOUND_VOLUME` | `1.0` | Volume level (0.0 to 1.0) |

### Backend Configuration

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Use Local STT | `USE_LOCAL_STT` | `false` | Use local Whisper model instead of cloud |
| Local Model | `LOCAL_MODEL` | `ggml-large-v3-turbo-q5_0` | Local Whisper model name |
| STT Endpoint | `STT_ENDPOINT` | (none) | Custom STT API endpoint URL |
| STT API Key | `STT_API_KEY` | (none) | API key for cloud STT |
| LLM Host | `LLM_HOST` | `http://localhost:11434` | LLM server address |
| LLM Model | `LLM_MODEL` | `llama3.2` | Model name for AI formatting |
| LLM API Key | `LLM_API_KEY` | (none) | API key for cloud LLM |

### Clipboard Settings

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Restore Clipboard | `RESTORE_CLIPBOARD` | `true` | Restore previous clipboard after paste |
| Restore Delay | `RESTORE_CLIPBOARD_DELAY_MS` | `1000` | Delay before restoring clipboard |

### System Settings

| Setting | Env Variable | Default | Description |
|---------|-------------|---------|-------------|
| Start at Login | `START_AT_LOGIN` | `false` | Launch CodeScribe automatically at login |
| History Enabled | `HISTORY_ENABLED` | `true` | Keep transcription history |
| Dump Audio Logs | `DUMP_AUDIO_LOGS` | `false` | Save raw audio to logs/audio (debugging) |

## Example Configuration

Here is a minimal `.env` file with common customizations:

```bash
# CodeScribe Configuration

# Use Ctrl+Shift for AI-assisted dictation
HOLD_MODS=ctrl_shift

# English transcription
WHISPER_LANGUAGE=en

# Enable AI formatting with local Ollama
AI_FORMATTING_ENABLED=true
AI_PROVIDER=ollama
LLM_HOST=http://localhost:11434
LLM_MODEL=llama3.2

# Quieter feedback sound
SOUND_VOLUME=0.5

# Specific microphone
AUDIO_INPUT_DEVICE=MacBook Pro Microphone
```

## Resetting to Defaults

If settings become problematic, quit CodeScribe, delete `~/.codescribe/.env`, and restart.
A fresh configuration file will be created from the template.

## Troubleshooting

**Settings not applying?** Restart the app after manual edits. Check for typos.
Boolean values accept: `true`, `false`, `1`, `0`, `yes`, `no`, `on`, `off`.

**Audio device not found?** Use the exact device name from System Settings > Sound,
or leave `AUDIO_INPUT_DEVICE` empty for system default.

---

*Created by M&K (c)2026 VetCoders*
