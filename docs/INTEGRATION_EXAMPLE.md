# Config Integration Example

## Cross-Language Configuration Sharing

This example demonstrates how the Rust frontend and Python backend share the same configuration file.

## File Location

Both Rust and Python read/write:
```
$HOME/.CodeScribe/settings.json
```

## Scenario 1: Python Creates Config, Rust Reads It

### Python Side

```python
# Python backend creates initial config
from codescribe.settings_store import update_settings

update_settings({
    "language": "pl",
    "ai_formatting_enabled": True,
    "ai_provider": "harmony",
    "ai_max_tokens": 1024
})
```

This creates `~/.CodeScribe/settings.json`:
```json
{
  "language": "pl",
  "ai_formatting_enabled": true,
  "ai_provider": "harmony",
  "ai_max_tokens": 1024,
  "ai_assistive_max_tokens": 2048
}
```

### Rust Side

```rust
// Rust frontend reads the same config
use crate::config::Config;

let config = Config::load();
assert_eq!(config.language, "pl");
assert_eq!(config.ai_formatting_enabled, true);
assert_eq!(config.ai_max_tokens, 1024);

// Rust-specific fields get default values
assert_eq!(config.hold_mods, "ctrl");
assert_eq!(config.backend_ports, vec![8237, 7237, 6237, 5237]);
```

## Scenario 2: Rust Modifies Config, Python Reads It

### Rust Side

```rust
// Rust frontend updates config
use crate::config::Config;

let mut config = Config::load();
config.silence_db = -50.0;
config.beep_on_start = false;
config.language = "en".to_string();
config.save()?;
```

This updates `~/.CodeScribe/settings.json`:
```json
{
  "language": "en",
  "ai_formatting_enabled": true,
  "ai_provider": "harmony",
  "ai_max_tokens": 1024,
  "ai_assistive_max_tokens": 2048,
  "hold_mods": "ctrl",
  "hold_exclusive": false,
  "hold_start_delay_ms": 800,
  "double_option_interval_ms": 450,
  "silence_db": -50.0,
  "silence_hang_sec": 0.8,
  "beep_on_start": false,
  "backend_ports": [8237, 7237, 6237, 5237]
}
```

### Python Side

```python
# Python backend reads updated config
from codescribe.settings_store import get_settings

settings = get_settings(force_reload=True)
assert settings.language == "en"
assert settings.ai_formatting_enabled == True

# Python ignores Rust-specific fields
# (silence_db, beep_on_start, backend_ports, etc.)
```

## Scenario 3: Shared Settings UI

### Example: Language Toggle

User changes language in tray menu (Rust):

```rust
// User clicks "Language: Polish" in tray menu
use crate::config::Config;

fn handle_language_change(lang: &str) -> anyhow::Result<()> {
    let mut config = Config::load();
    config.update(|c| {
        c.language = lang.to_string();
    })?;

    println!("Language changed to: {}", lang);
    Ok(())
}

handle_language_change("pl")?;
```

Python backend automatically picks it up:

```python
# Next transcription request
from codescribe.settings_store import get_settings

settings = get_settings(force_reload=True)
# Uses Polish language model because language="pl"
```

## Scenario 4: Environment Override

### Development Setup

```bash
# Developer wants custom config location
export CODESCRIBE_DATA_DIR="$HOME/dev/codescribe-test"
```

### Rust Side

```rust
use crate::config::Config;

let dir = Config::config_dir();
// Returns: /Users/dev/codescribe-test

let config = Config::load();
// Reads from: /Users/dev/codescribe-test/settings.json
```

### Python Side

```python
import os
os.environ['CODESCRIBE_DATA_DIR'] = os.path.expanduser('~/dev/codescribe-test')

from codescribe.settings_store import get_settings

settings = get_settings()
# Reads from: ~/dev/codescribe-test/settings.json
```

## Complete Integration Flow

```
┌─────────────────┐
│   User Action   │
│ (Tray Menu)     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Rust Frontend  │
│  Config::load() │
│  config.update()│
│  config.save()  │
└────────┬────────┘
         │
         ▼
┌──────────────────────────────┐
│ ~/.CodeScribe/settings.json  │
│  {                           │
│    "language": "pl",         │
│    "ai_formatting": true,    │
│    "silence_db": -50.0,      │
│    ...                       │
│  }                           │
└────────┬─────────────────────┘
         │
         ▼
┌─────────────────┐
│ Python Backend  │
│ get_settings()  │
│ (auto-reload)   │
└─────────────────┘
         │
         ▼
┌─────────────────┐
│ STT/AI Services │
│ (uses config)   │
└─────────────────┘
```

## Field Compatibility Matrix

| Field | Python Reads | Python Writes | Rust Reads | Rust Writes |
|-------|--------------|---------------|------------|-------------|
| language | ✅ | ✅ | ✅ | ✅ |
| ai_formatting_enabled | ✅ | ✅ | ✅ | ✅ |
| ai_provider | ✅ | ✅ | ✅ | ✅ |
| ai_max_tokens | ✅ | ✅ | ✅ | ✅ |
| ai_assistive_max_tokens | ✅ | ✅ | ✅ | ✅ |
| hold_mods | ❌ | ❌ | ✅ | ✅ |
| hold_exclusive | ❌ | ❌ | ✅ | ✅ |
| hold_start_delay_ms | ❌ | ❌ | ✅ | ✅ |
| double_option_interval_ms | ❌ | ❌ | ✅ | ✅ |
| silence_db | ❌ | ❌ | ✅ | ✅ |
| silence_hang_sec | ❌ | ❌ | ✅ | ✅ |
| beep_on_start | ❌ | ❌ | ✅ | ✅ |
| backend_ports | ❌ | ❌ | ✅ | ✅ |

**Legend:**
- ✅ = Field is used/modified
- ❌ = Field is ignored (no errors, just skipped)

## Best Practices

1. **Always reload config when needed**
   ```rust
   // DON'T cache config globally
   static CONFIG: Config = Config::load(); // ❌

   // DO load fresh when needed
   fn handle_action() {
       let config = Config::load(); // ✅
   }
   ```

2. **Python must force reload after Rust writes**
   ```python
   # After Rust tray changes config
   settings = get_settings(force_reload=True)
   ```

3. **Use update helper for atomic changes**
   ```rust
   config.update(|c| {
       c.language = "pl".to_string();
       c.beep_on_start = false;
   })?; // Saves automatically
   ```

4. **Handle missing config gracefully**
   ```rust
   // This never panics - returns defaults if file missing
   let config = Config::load();
   ```

## Testing Integration

### Test 1: Round-trip compatibility

```bash
# Python writes config
python -c "from codescribe.settings_store import update_settings; \
           update_settings({'language': 'pl', 'ai_formatting_enabled': True})"

# Rust reads and modifies
cargo run --example config_usage

# Python reads back
python -c "from codescribe.settings_store import get_settings; \
           s = get_settings(force_reload=True); \
           print(f'Language: {s.language}, AI: {s.ai_formatting_enabled}')"
```

### Test 2: Verify JSON structure

```bash
cat ~/.CodeScribe/settings.json | jq '.'
# Should show valid JSON with all fields
```

### Test 3: Environment override

```bash
export CODESCRIBE_DATA_DIR="/tmp/test-config"
cargo run --example config_usage
ls -la /tmp/test-config/settings.json
```

## Troubleshooting

### Issue: Python doesn't see Rust changes

**Solution**: Force reload
```python
settings = get_settings(force_reload=True)
```

### Issue: Rust-specific fields disappear

**Cause**: Python rewrites the entire file, removing unknown fields

**Solution**: This is expected behavior. Rust will restore defaults on next load.

### Issue: Invalid JSON after manual edit

**Solution**: Both Rust and Python handle this gracefully by returning defaults

```rust
// Returns default config if JSON is malformed
let config = Config::load();
```

---

Created by M&K (c)2025 The LibraxisAI Team
Co-Authored-By: void@div0.space & the1st@whoai.am
