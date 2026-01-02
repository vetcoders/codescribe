# Config Module Documentation

## Overview

The `config.rs` module provides configuration management for CodeScribe Rust frontend. It maintains compatibility with the Python backend by sharing the same `~/.CodeScribe/settings.json` file.

## Features

- **Cross-platform config paths** using the `directories` crate
- **JSON serialization** with serde for Python interoperability
- **Automatic sanitization** of invalid values
- **Environment variable overrides** for custom config locations
- **Default values** for all settings when config file doesn't exist

## Config Structure

### Shared Settings (Python + Rust)

These settings are shared with the Python backend:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `language` | `String` | `"auto"` | Language preference: "auto", "pl", or "en" |
| `ai_formatting_enabled` | `bool` | `false` | Whether AI formatting is enabled |
| `ai_provider` | `String` | `"harmony"` | AI provider: "harmony" or "ollama" |
| `ai_max_tokens` | `i32` | `512` | Maximum tokens for regular AI completions |
| `ai_assistive_max_tokens` | `i32` | `2048` | Maximum tokens for assistive completions |

### Rust-Specific Settings

These settings are used only by the Rust frontend:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hold_mods` | `String` | `"ctrl"` | Modifier keys for hold-to-talk (e.g., "ctrl", "ctrl+alt") |
| `hold_exclusive` | `bool` | `false` | Whether hold key is exclusive |
| `hold_start_delay_ms` | `u64` | `800` | Delay before recording starts (ms) |
| `double_option_interval_ms` | `u64` | `450` | Double key press detection interval (ms) |
| `silence_db` | `f32` | `-45.0` | Silence threshold in decibels (-60 to 0) |
| `silence_hang_sec` | `f32` | `0.8` | Silence hang time before stopping (seconds) |
| `beep_on_start` | `bool` | `true` | Play beep when recording starts |
| `backend_ports` | `Vec<u16>` | `[8237, 7237, 6237, 5237]` | Backend ports to try (in priority order) |

## Usage Examples

### Basic Usage

```rust
use codescribe::config::Config;

// Load configuration (or get defaults if file doesn't exist)
let config = Config::load();

println!("Language: {}", config.language);
println!("Backend ports: {:?}", config.backend_ports);
```

### Modifying and Saving

```rust
use codescribe::config::Config;

let mut config = Config::load();
config.ai_formatting_enabled = true;
config.language = "pl".to_string();
config.save()?;
```

### Using the Update Helper

```rust
use codescribe::config::Config;

let mut config = Config::load();
config.update(|c| {
    c.silence_db = -50.0;
    c.beep_on_start = false;
})?;
```

### Getting Config Directory

```rust
use codescribe::config::Config;

let config_dir = Config::config_dir();
println!("Config directory: {:?}", config_dir);
```

## File Locations

### Default Location

```
$HOME/.CodeScribe/settings.json
```

### Environment Variable Overrides

1. **`CODESCRIBE_SETTINGS_PATH`** - Direct path to settings.json
   ```bash
   export CODESCRIBE_SETTINGS_PATH="$HOME/custom/path/settings.json"
   ```

2. **`CODESCRIBE_DATA_DIR`** - Custom data directory (preferred)
   ```bash
   export CODESCRIBE_DATA_DIR="$HOME/custom/codescribe"
   # Settings will be at: $HOME/custom/codescribe/settings.json
   ```

3. **`CODESCRIBE_APP_DIR`** - Legacy data directory override
   ```bash
   export CODESCRIBE_APP_DIR="$HOME/legacy/path"
   ```

## Sanitization

The config module automatically sanitizes invalid values:

- **Language**: Normalized to lowercase, defaults to "auto" if empty
- **AI Provider**: Must be "harmony" or "ollama", defaults to "harmony"
- **Token Limits**: Must be positive, defaults to 512/2048
- **Silence DB**: Must be between -100 and 0, defaults to -45
- **Silence Hang**: Must be between 0 and 10 seconds, defaults to 0.8
- **Backend Ports**: Must have at least one port, defaults to `[8237, 7237, 6237, 5237]`

## Python Compatibility

The Rust config module maintains full compatibility with Python's `settings_store.py`:

### Python Side (Reading)
```python
from codescribe.settings_store import get_settings

settings = get_settings()
print(f"Language: {settings.language}")
```

### Rust Side (Reading)
```rust
let config = Config::load();
println!("Language: {}", config.language);
```

### Shared JSON Format

Both Python and Rust read/write the same JSON file:

```json
{
  "language": "auto",
  "ai_formatting_enabled": false,
  "ai_provider": "harmony",
  "ai_max_tokens": 512,
  "ai_assistive_max_tokens": 2048,
  "hold_mods": "ctrl",
  "hold_exclusive": false,
  "hold_start_delay_ms": 800,
  "double_option_interval_ms": 450,
  "silence_db": -45.0,
  "silence_hang_sec": 0.8,
  "beep_on_start": true,
  "backend_ports": [8237, 7237, 6237, 5237]
}
```

**Note**: Python only uses the first 5 fields. The Rust-specific fields are ignored by Python.

## Error Handling

The config module uses graceful error handling:

- **Missing file**: Returns default configuration
- **Malformed JSON**: Returns default configuration
- **Invalid values**: Sanitized to valid defaults

Only `save()` and `update()` return `Result<()>` errors:

```rust
use anyhow::Result;

fn example() -> Result<()> {
    let mut config = Config::load(); // Never fails
    config.language = "pl".to_string();
    config.save()?; // Can fail if directory can't be created
    Ok(())
}
```

## Testing

Run the config module tests:

```bash
cd codescribe-rs
cargo test config
```

Tests cover:
- Default configuration values
- Language sanitization
- AI provider validation
- Token limit validation
- Config directory resolution

## Migration Notes

### From Python-Only to Rust+Python

If you're migrating from Python-only setup:

1. Existing `~/.CodeScribe/settings.json` will be automatically read by Rust
2. Rust-specific fields will be added with defaults when Rust saves config
3. Python will ignore Rust-specific fields
4. Both can safely read/write the same file

### Adding New Settings

To add a new setting:

1. Add field to `Config` struct with `#[serde(default = "default_fn")]`
2. Create default function if not using `Default` trait
3. Add sanitization logic in `sanitize()` method if needed
4. Update this documentation
5. Consider Python compatibility if field should be shared

## See Also

- Python settings module: `src/codescribe/settings_store.py`
- Python path utilities: `src/codescribe/path_utils.py`
- Cargo dependencies: `Cargo.toml`
