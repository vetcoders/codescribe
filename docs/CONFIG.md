# CodeScribe Configuration System

Complete configuration management with .env file persistence and thread-safe global access.

## Overview

The config system provides:
- **Dual-layer storage**: `.env` (primary) + `settings.json` (legacy compatibility)
- **Environment variable support**: All settings can be overridden via env vars
- **Thread-safe global state**: Safe concurrent access via `OnceLock<RwLock<Config>>`
- **Type-safe enums**: Strongly-typed configuration values
- **Automatic validation**: Sanitization and bounds checking

## Storage Locations

- **Config directory**: `$HOME/.codescribe/`
- **.env file**: `$HOME/.codescribe/.env`
- **settings.json** (legacy): `$HOME/.codescribe/settings.json`

### Override paths:
```bash
export CODESCRIBE_DATA_DIR="~/custom/path"
export CODESCRIBE_ENV_PATH="~/custom/.env"
export CODESCRIBE_SETTINGS_PATH="~/custom/settings.json"
```

## Configuration Options

### Hotkeys

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `HOLD_MODS` | `HoldMods` | `ctrl` | Modifier key combo: `ctrl`, `ctrl_alt`, `ctrl_shift`, `ctrl_cmd` |
| `HOLD_EXCLUSIVE` | `bool` | `false` | Ignore extra modifiers when hold key is pressed |
| `TOGGLE_TRIGGER` | `ToggleTrigger` | `double_option` | Toggle method: `double_option`, `double_ralt`, `none` |
| `HOLD_START_DELAY_MS` | `u64` | `800` | Delay before recording starts (milliseconds) |

### Language

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `WHISPER_LANGUAGE` | `Language` | `auto` | Transcription language: `auto`, `pl`, `en` |

### AI Formatting

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `AI_FORMATTING_ENABLED` | `bool` | `false` | Enable AI-powered formatting |
| `AI_PROVIDER` | `AiProvider` | `harmony` | AI provider: `harmony`, `ollama` |
| `AI_MAX_TOKENS` | `i32` | `512` | Max tokens for regular completions |
| `AI_ASSISTIVE_MAX_TOKENS` | `i32` | `2048` | Max tokens for assistive completions |

### UI

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `SHOW_TRAY_GLYPH` | `bool` | `true` | Show icon in system tray |
| `HOLD_INDICATOR` | `bool` | `true` | Show recording indicator badge |
| `HOLD_BADGE_SIZE` | `u32` | `12` | Badge size in pixels (8-64) |
| `HOLD_BADGE_OFFSET_X` | `i32` | `10` | Badge X offset |
| `HOLD_BADGE_OFFSET_Y` | `i32` | `-10` | Badge Y offset |

### Sound

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `BEEP_ON_START` | `bool` | `true` | Play sound when recording starts |
| `SOUND_NAME` | `String` | `"Tink"` | System sound name (e.g., "Tink", "Pop") |
| `SOUND_VOLUME` | `f32` | `1.0` | Sound volume (0.0-1.0) |

### History

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `HISTORY_ENABLED` | `bool` | `true` | Keep transcription history |

### Backends

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `WHISPER_SERVER_URL` | `String` | `http://localhost:8237` | Whisper STT server URL |
| `LLM_SERVER_URL` | `String` | `http://localhost:8237` | LLM server URL |
| `OLLAMA_HOST` | `String` | `http://localhost:11434` | Ollama server URL |
| `OLLAMA_MODEL` | `String` | `llama3.2` | Ollama model name |

### Clipboard

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `RESTORE_CLIPBOARD` | `bool` | `true` | Restore previous clipboard after paste |
| `RESTORE_CLIPBOARD_DELAY_MS` | `u64` | `1000` | Delay before restoring (milliseconds) |

### System

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `START_AT_LOGIN` | `bool` | `false` | Start app at system login |

## Usage Examples

### Basic Loading

```rust
use codescribe::config::Config;

// Load from .env or defaults
let config = Config::load();
println!("Language: {:?}", config.whisper_language);
println!("Beep enabled: {}", config.beep_on_start);
```

### Global Configuration

```rust
use codescribe::config;

// Initialize once at app startup
config::init();

// Read access (can have multiple readers)
let cfg = config::get();
println!("Hold mods: {:?}", cfg.hold_mods);
drop(cfg); // Release read lock

// Update configuration
config::update(|c| {
    c.beep_on_start = false;
    c.sound_volume = 0.8;
    c.ai_formatting_enabled = true;
});

// Save to .env file
config::save()?;
```

### Enum Parsing

```rust
use codescribe::config::{HoldMods, Language, AiProvider};

// Parse from strings
let mods = HoldMods::from_str("ctrl_alt").unwrap();
let lang = Language::from_str("pl").unwrap();
let provider = AiProvider::from_str("ollama").unwrap();

// Convert to strings
assert_eq!(mods.as_str(), "ctrl_alt");
assert_eq!(lang.as_str(), "pl");
assert_eq!(provider.as_str(), "ollama");
```

### Saving Configuration

```rust
let config = Config::load();

// Save entire config to .env
config.save_all_to_env()?;

// Save single value
config.save_to_env("BEEP_ON_START", "false")?;
config.save_to_env("SOUND_VOLUME", "0.5")?;
```

### Environment Variable Override

```bash
# Override via environment
export WHISPER_LANGUAGE=pl
export AI_FORMATTING_ENABLED=true
export SOUND_VOLUME=0.7

# Or use .env file
echo "WHISPER_LANGUAGE=pl" >> ~/.codescribe/.env
echo "AI_FORMATTING_ENABLED=true" >> ~/.codescribe/.env
```

```rust
// Environment variables take precedence
let config = Config::load();
// Will use values from env vars or .env
```

## Priority Order

Configuration is loaded in this priority (highest to lowest):

1. **Environment variables** - Runtime overrides
2. **.env file** - User configuration
3. **settings.json** - Legacy compatibility
4. **Default values** - Hardcoded fallbacks

## Type Safety

All enum types have safe parsing with `from_str()`:

```rust
// Returns None for invalid values
assert_eq!(HoldMods::from_str("invalid"), None);
assert_eq!(Language::from_str("xyz"), None);

// Case-insensitive
assert_eq!(Language::from_str("PL"), Some(Language::Polish));
assert_eq!(Language::from_str("pl"), Some(Language::Polish));

// Multiple formats supported
assert_eq!(HoldMods::from_str("ctrl_alt"), Some(HoldMods::CtrlAlt));
assert_eq!(HoldMods::from_str("ctrl+alt"), Some(HoldMods::CtrlAlt));
```

## Validation

All configuration values are automatically validated:

- **Token limits**: Must be > 0, defaults to 512/2048
- **Sound volume**: Clamped to 0.0-1.0
- **Badge size**: Clamped to 8-64 pixels
- **Silence thresholds**: Validated on load (legacy)

```rust
let mut config = Config::default();
config.sound_volume = 1.5; // Out of range
config.sanitize();
assert_eq!(config.sound_volume, 1.0); // Clamped
```

## Thread Safety

Global config uses `OnceLock<RwLock<Config>>`:

```rust
// Multiple readers (no blocking)
let reader1 = config::get();
let reader2 = config::get();
println!("{}", reader1.beep_on_start);
println!("{}", reader2.sound_volume);

// Single writer (blocks readers)
config::update(|c| {
    c.beep_on_start = false;
});
```

## Migration from Legacy

Old `settings.json` is automatically migrated:

```rust
// If both exist:
// 1. Load settings.json
// 2. Override with .env values
// 3. Override with env vars
let config = Config::load();

// Migrate to .env permanently
config.save_all_to_env()?;
```

## Example .env File

```bash
# CodeScribe Configuration
# Generated automatically - edit with care

# Hotkeys
HOLD_MODS=ctrl
HOLD_EXCLUSIVE=false
TOGGLE_TRIGGER=double_option
HOLD_START_DELAY_MS=800

# Language
WHISPER_LANGUAGE=pl

# AI Formatting
AI_FORMATTING_ENABLED=true
AI_PROVIDER=ollama
AI_MAX_TOKENS=1024
AI_ASSISTIVE_MAX_TOKENS=4096

# UI
SHOW_TRAY_GLYPH=true
HOLD_INDICATOR=true
HOLD_BADGE_SIZE=14
HOLD_BADGE_OFFSET_X=12
HOLD_BADGE_OFFSET_Y=-12

# Sound
BEEP_ON_START=true
SOUND_NAME=Pop
SOUND_VOLUME=0.8

# History
HISTORY_ENABLED=true

# Backends
WHISPER_SERVER_URL=http://localhost:8237
LLM_SERVER_URL=http://localhost:8237
OLLAMA_HOST=http://localhost:11434
OLLAMA_MODEL=llama3.2

# Clipboard
RESTORE_CLIPBOARD=true
RESTORE_CLIPBOARD_DELAY_MS=1000

# System
START_AT_LOGIN=true
```

## Testing

Run the demo to verify configuration:

```bash
cargo run --example config_demo
```

Run tests:

```bash
cargo test --lib config
```

## See Also

- `examples/config_demo.rs` - Full working example
- `src/config.rs` - Implementation
- `Cargo.toml` - Dependencies (dotenvy, directories, etc.)

---

Created by M&K (c)2025 The LibraxisAI Team
Co-Authored-By: void@div0.space & the1st@whoai.am
