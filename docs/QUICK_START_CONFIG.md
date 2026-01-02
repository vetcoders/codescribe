# CodeScribe Config - Quick Start

## Installation

Already included! Just use it.

```toml
[dependencies]
dotenvy = "0.15"  # Already in Cargo.toml
```

## Basic Usage

### 1. Initialize (once at startup)

```rust
use codescribe::config;

fn main() {
    config::init();
    // ... rest of app
}
```

### 2. Read Configuration

```rust
let cfg = config::get();
println!("Language: {:?}", cfg.whisper_language);
println!("Beep: {}", cfg.beep_on_start);
println!("Volume: {}", cfg.sound_volume);
```

### 3. Update Configuration

```rust
config::update(|c| {
    c.beep_on_start = false;
    c.sound_volume = 0.8;
    c.whisper_language = Language::Polish;
});
```

### 4. Save to .env

```rust
config::save()?;  // Saves to ~/.codescribe/.env
```

## Environment Variables

All settings can be overridden via environment variables:

```bash
export WHISPER_LANGUAGE=pl
export BEEP_ON_START=false
export SOUND_VOLUME=0.7
export AI_FORMATTING_ENABLED=true
```

## Common Settings

### Change Language
```rust
config::update(|c| {
    c.whisper_language = Language::Polish;
});
```

### Disable Beep
```rust
config::update(|c| {
    c.beep_on_start = false;
});
```

### Change AI Provider
```rust
config::update(|c| {
    c.ai_provider = AiProvider::Ollama;
    c.ollama_model = "llama3.2".to_string();
});
```

### Adjust Hotkey
```rust
config::update(|c| {
    c.hold_mods = HoldMods::CtrlAlt;
    c.hold_start_delay_ms = 500;
});
```

## .env File Example

Create `~/.codescribe/.env`:

```bash
# Language
WHISPER_LANGUAGE=pl

# Sound
BEEP_ON_START=true
SOUND_NAME=Pop
SOUND_VOLUME=0.8

# AI
AI_FORMATTING_ENABLED=true
AI_PROVIDER=ollama
OLLAMA_MODEL=llama3.2

# Hotkeys
HOLD_MODS=ctrl_alt
HOLD_START_DELAY_MS=500
```

## Enums Quick Reference

### HoldMods
- `ctrl` - Control key only
- `ctrl_alt` - Control + Alt
- `ctrl_shift` - Control + Shift
- `ctrl_cmd` - Control + Command (macOS)

### Language
- `auto` - Auto-detect
- `pl` - Polish
- `en` - English

### AiProvider
- `harmony` - Harmony API
- `ollama` - Local Ollama

### ToggleTrigger
- `double_option` - Double-tap Option/Alt
- `double_ralt` - Double-tap Right Alt
- `none` - Disabled

## Test It

```bash
cargo run --example config_demo
```

## Full Documentation

See `CONFIG.md` for complete reference.

---

Created by M&K (c)2025 The LibraxisAI Team
