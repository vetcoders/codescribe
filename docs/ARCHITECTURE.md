# CodeScribe Architecture
> Created by M&K (c)2026 VetCoders

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                    CodeScribe Tauri App                         │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                 Leptos WASM Frontend                     │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐               │   │
│  │  │ Voice Lab│  │ Teacher  │  │ Settings │               │   │
│  │  └────┬─────┘  └────┬─────┘  └────┬─────┘               │   │
│  │       │              │              │                    │   │
│  │       └──────────────┴──────────────┘                    │   │
│  │                      │                                   │   │
│  │              invoke("command", args)                     │   │
│  └──────────────────────┬──────────────────────────────────┘   │
│                         │ Tauri IPC                             │
│  ┌──────────────────────┴──────────────────────────────────┐   │
│  │              Tauri Rust Backend (Native)                 │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────┐  │   │
│  │  │ stt.rs   │  │config.rs │  │audio.rs  │  │lexicon  │  │   │
│  │  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬────┘  │   │
│  │       │              │              │              │     │   │
│  └───────┴──────────────┴──────────────┴──────────────┴────┘   │
│                         │                                       │
│                 codescribe crate (lib)                          │
│  ┌──────────────────────┴──────────────────────────────────┐   │
│  │  ┌──────────────┐  ┌─────────┐  ┌────────┐  ┌────────┐  │   │
│  │  │ local_stt.rs │  │config/  │  │audio.rs│  │hotkeys │  │   │
│  │  │ (Whisper)    │  │         │  │(cpal)  │  │        │  │   │
│  │  └──────────────┘  └─────────┘  └────────┘  └────────┘  │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                    ┌─────────┴─────────┐
                    │ Whisper Model     │
                    │ large-v3-turbo    │
                    │ mlx-q8 (~800MB)   │
                    └───────────────────┘
```

## IPC Commands Reference

### commands/stt.rs

| Command | Parameters | Returns | Backend | Status |
|---------|------------|---------|---------|--------|
| `transcribe_audio` | `audio_path: String` | `Result<String, String>` | `LocalWhisperEngine::transcribe_file_with_language()` | ✅ IMPLEMENTED |
| `get_available_models` | none | `Vec<String>` | `ModelManager::list_models()` | ✅ IMPLEMENTED |
| `get_current_model` | none | `String` | `config.local_model` | ✅ IMPLEMENTED |

### commands/config.rs

| Command | Parameters | Returns | Backend | Status |
|---------|------------|---------|---------|--------|
| `get_config` | none | `serde_json::Value` | `Config` serialized | ✅ IMPLEMENTED |
| `save_config` | `config: serde_json::Value` | `Result<(), String>` | `Config::save_to_env()` | ✅ IMPLEMENTED |
| `get_env_var` | `key: String` | `Option<String>` | `std::env::var()` | ✅ IMPLEMENTED |

### commands/audio.rs

| Command | Parameters | Returns | Backend | Status |
|---------|------------|---------|---------|--------|
| `list_audio_devices` | none | `Vec<String>` | `cpal::default_host().input_devices()` | ✅ IMPLEMENTED |
| `get_current_audio_device` | none | `Option<String>` | `cpal::default_host().default_input_device()` | ✅ IMPLEMENTED |

### commands/lexicon.rs

| Command | Parameters | Returns | Backend | Status |
|---------|------------|---------|---------|--------|
| `get_lexicon_entries` | `topic: Option<String>` | `Vec<LexiconEntry>` | File-based storage | ✅ IMPLEMENTED |
| `list_lexicon_topics` | none | `Vec<String>` | Directory scan | ✅ IMPLEMENTED |
| `save_lexicon_entry` | `entry: LexiconEntry` | `Result<(), String>` | File write | ✅ IMPLEMENTED |

### commands/recording.rs

| Command | Parameters | Returns | Backend | Status |
|---------|------------|---------|---------|--------|
| `start_recording` | none | `Result<(), String>` | `codescribe::audio::Recorder::start()` | ✅ IMPLEMENTED |
| `stop_recording` | none | `Result<Option<String>, String>` | `Recorder::stop()` → returns WAV path | ✅ IMPLEMENTED |
| `is_recording` | none | `Result<bool, String>` | State check | ✅ IMPLEMENTED |

## UI → IPC Mapping

### Voice Lab (lab/mod.rs)

| UI Element | Action | IPC Call | Status |
|------------|--------|----------|--------|
| "Start streaming" button | Starts audio capture | `start_recording` | ✅ Connected |
| "Stop" button | Stops audio capture + auto-transcribe | `stop_recording` → `transcribe_audio` | ✅ Connected |
| "Upload → STT" button | Transcribe file | `transcribe_audio` | ✅ Connected |
| "Copy transcript" button | Copy to clipboard | **NONE** - log only | ❌ TODO |
| "Load config" button | Fetch config | `get_config` | ✅ Connected |
| "List models" button | Fetch models | `get_available_models` | ✅ Connected |
| "List devices" button | Fetch devices | `list_audio_devices` | ✅ Connected |

### Settings (settings/mod.rs)

| UI Element | Action | IPC Call | Status |
|------------|--------|----------|--------|
| Config form | Load settings | `get_config` | ✅ Connected |
| Save button | Persist settings | `save_config` | ✅ Connected |

### Chat Panel

| UI Element | Action | IPC Call | Status |
|------------|--------|----------|--------|
| Send message | Call LLM | **NONE** - placeholder | ❌ TODO |

## Hotkey Integration

### Current Flow (Standalone Tray App)

```
┌─────────────┐    ┌────────────┐    ┌───────────┐    ┌──────────┐
│ CGEventTap  │───►│ hotkeys.rs │───►│controller │───►│local_stt │
│ (macOS API) │    │ HotkeyEvent│    │   .rs     │    │   .rs    │
└─────────────┘    └────────────┘    └───────────┘    └──────────┘
       │                                    │
       │                                    ▼
       │                            ┌──────────────┐
       │                            │ Paste to     │
       │                            │ Active App   │
       │                            └──────────────┘
       │
  Hold Ctrl → Start recording
  Release Ctrl → Stop + Transcribe + Paste
  Double Option → Toggle recording
```

### Tauri Integration ✅ IMPLEMENTED

**Implementation**: Spawn hotkey thread in Tauri setup (Option A)
```rust
// In tauri-app/src/lib.rs setup()
// Clone state for hotkey listener (shares internal Arcs)
let state_for_hotkeys = Arc::new(state.clone());

// In setup closure:
hotkey_integration::start_hotkey_listener(
    app.handle().clone(),
    Arc::clone(&state_for_hotkeys),
)?;
```

The `hotkey_integration.rs` module:
- Creates `HotkeyManager` which spawns CGEventTap in background thread
- Receives `HotkeyEvent` via crossbeam channel
- Routes Hold Down → `handle_start_recording()`
- Routes Hold Up / Toggle → `handle_stop_recording()` → transcribe → paste

### Model Location

**Production**: Bundled in `Resources/models/whisper-large-v3-turbo-mlx-q8/`
**Development**: `~/.codescribe/models/whisper-large-v3-turbo-mlx-q8/`

Model files required:
- `config.json`
- `model.safetensors` (~800MB)
- `tokenizer.json`
- `mel_filters.npz`

## File Structure

```
CodeScribe/
├── src/                      # codescribe crate (backend library)
│   ├── local_stt.rs          # Whisper transcription engine
│   ├── hotkeys.rs            # CGEventTap hotkey handler
│   ├── controller.rs         # Recording/transcription orchestration
│   ├── config/               # Configuration management
│   └── ...
├── tauri-app/                # Tauri application
│   ├── src/
│   │   ├── lib.rs            # Tauri setup + tray + hotkey init
│   │   ├── hotkey_integration.rs  # CGEventTap → recording → paste
│   │   ├── state.rs          # AppState (config, stt engine, recording)
│   │   ├── commands/         # IPC command handlers
│   │   │   ├── stt.rs
│   │   │   ├── config.rs
│   │   │   ├── audio.rs
│   │   │   ├── recording.rs  # start/stop/is_recording
│   │   │   └── lexicon.rs
│   │   ├── ui/               # Leptos components
│   │   │   ├── app.rs
│   │   │   ├── lab/mod.rs
│   │   │   ├── settings/mod.rs
│   │   │   └── teacher/mod.rs
│   │   └── state.rs          # AppState (config, stt engine)
│   ├── Trunk.toml            # WASM build config
│   └── tauri.conf.json       # Tauri config
└── docs/
    ├── ARCHITECTURE.md       # This file
    └── ROADMAP-TAURI-INTEGRATION.md
```

## Next Steps

### Phase 4: Wire Streaming (Priority)
1. Add `start_recording` IPC command
2. Add `stop_recording` IPC command
3. Connect "Start streaming" button → `start_recording`
4. Implement audio capture in Tauri backend (cpal)
5. On stop → auto-transcribe → return result

### Phase 5: Hotkey Integration
1. Add hotkey listener thread to Tauri setup
2. Emit Tauri events on hotkey press/release
3. Frontend can listen and show recording state
4. Or: backend handles full flow (paste to active app)

### Phase 6: Model Bundling
1. Add model to `tauri.conf.json` resources
2. Modify `local_stt.rs` to check bundle path first
3. Test cold start (~5-10s for model load)
