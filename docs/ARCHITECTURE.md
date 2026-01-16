# CodeScribe Architecture

> Created by M&K (c)2026 VetCoders

## System Overview

```mermaid
flowchart TB
    %% High-level packaging / layers

    subgraph TAURI[CodeScribe Tauri App]
        direction TB

        subgraph UI[Leptos WASM Frontend]
            direction LR
            VL[Voice Lab]
            TE[Teacher]
            SE[Settings]
        end

        INV[invoke("command", args)]
        UI --> INV

        subgraph BACKEND[Tauri Rust Backend (Native)]
            direction LR
            BENTRY[Command handlers]
            STT[commands/stt.rs]
            CFG[commands/config.rs]
            AUD[commands/audio.rs]
            LEX[commands/lexicon.rs]

            BENTRY --> STT
            BENTRY --> CFG
            BENTRY --> AUD
            BENTRY --> LEX
        end

        INV -->|Tauri IPC| BENTRY

        subgraph LIB[codescribe crate (lib)]
            direction LR
            LENTRY[Core modules]
            WH[whisper/\n(embedded + singleton)]
            CO[config/]
            AU[audio/\n(cpal + stream)]
            HK[hotkeys/]

            LENTRY --> WH
            LENTRY --> CO
            LENTRY --> AU
            LENTRY --> HK
        end

        BENTRY --> LENTRY
    end

    WH --> MODEL[Whisper Model\nlarge-v3-turbo\nmlx-q8 (~888MB)\n(embedded in bin)]
```

## IPC Commands Reference

### commands/stt.rs

| Command                | Parameters           | Returns                  | Backend                                               | Status        |
|------------------------|----------------------|--------------------------|-------------------------------------------------------|---------------|
| `transcribe_audio`     | `audio_path: String` | `Result<String, String>` | `LocalWhisperEngine::transcribe_file_with_language()` | ✅ IMPLEMENTED |
| `get_available_models` | none                 | `Vec<String>`            | `ModelManager::list_models()`                         | ✅ IMPLEMENTED |
| `get_current_model`    | none                 | `String`                 | `config.local_model`                                  | ✅ IMPLEMENTED |

### commands/config.rs

| Command       | Parameters                  | Returns              | Backend                 | Status        |
|---------------|-----------------------------|----------------------|-------------------------|---------------|
| `get_config`  | none                        | `serde_json::Value`  | `Config` serialized     | ✅ IMPLEMENTED |
| `save_config` | `config: serde_json::Value` | `Result<(), String>` | `Config::save_to_env()` | ✅ IMPLEMENTED |
| `get_env_var` | `key: String`               | `Option<String>`     | `std::env::var()`       | ✅ IMPLEMENTED |

### commands/audio.rs

| Command                    | Parameters | Returns          | Backend                                       | Status        |
|----------------------------|------------|------------------|-----------------------------------------------|---------------|
| `list_audio_devices`       | none       | `Vec<String>`    | `cpal::default_host().input_devices()`        | ✅ IMPLEMENTED |
| `get_current_audio_device` | none       | `Option<String>` | `cpal::default_host().default_input_device()` | ✅ IMPLEMENTED |

### commands/lexicon.rs

| Command               | Parameters              | Returns              | Backend            | Status        |
|-----------------------|-------------------------|----------------------|--------------------|---------------|
| `get_lexicon_entries` | `topic: Option<String>` | `Vec<LexiconEntry>`  | File-based storage | ✅ IMPLEMENTED |
| `list_lexicon_topics` | none                    | `Vec<String>`        | Directory scan     | ✅ IMPLEMENTED |
| `save_lexicon_entry`  | `entry: LexiconEntry`   | `Result<(), String>` | File write         | ✅ IMPLEMENTED |

### commands/recording.rs

| Command           | Parameters | Returns                          | Backend                                | Status        |
|-------------------|------------|----------------------------------|----------------------------------------|---------------|
| `start_recording` | none       | `Result<(), String>`             | `codescribe::audio::Recorder::start()` | ✅ IMPLEMENTED |
| `stop_recording`  | none       | `Result<Option<String>, String>` | `Recorder::stop()` → returns WAV path  | ✅ IMPLEMENTED |
| `is_recording`    | none       | `Result<bool, String>`           | State check                            | ✅ IMPLEMENTED |

## UI → IPC Mapping

### Voice Lab (lab/mod.rs)

| UI Element               | Action                                | IPC Call                              | Status      |
|--------------------------|---------------------------------------|---------------------------------------|-------------|
| "Start streaming" button | Starts audio capture                  | `start_recording`                     | ✅ Connected |
| "Stop" button            | Stops audio capture + auto-transcribe | `stop_recording` → `transcribe_audio` | ✅ Connected |
| "Upload → STT" button    | Transcribe file                       | `transcribe_audio`                    | ✅ Connected |
| "Copy transcript" button | Copy to clipboard                     | **NONE** - log only                   | ❌ TODO      |
| "Load config" button     | Fetch config                          | `get_config`                          | ✅ Connected |
| "List models" button     | Fetch models                          | `get_available_models`                | ✅ Connected |
| "List devices" button    | Fetch devices                         | `list_audio_devices`                  | ✅ Connected |

### Settings (settings/mod.rs)

| UI Element  | Action           | IPC Call      | Status      |
|-------------|------------------|---------------|-------------|
| Config form | Load settings    | `get_config`  | ✅ Connected |
| Save button | Persist settings | `save_config` | ✅ Connected |

### Chat Panel

| UI Element   | Action   | IPC Call               | Status |
|--------------|----------|------------------------|--------|
| Send message | Call LLM | **NONE** - placeholder | ❌ TODO |

## Hotkey Integration

### Current Flow (Standalone Tray App)

```
┌─────────────┐    ┌────────────┐    ┌───────────┐    ┌──────────┐
│ CGEventTap  │───►│ hotkeys.rs │───►│controller │───►│whisper   │
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

```text
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

**Release Builds**: Model is embedded directly in the binary via `include_bytes!` (~888MB total).
Zero disk I/O, zero file paths, model bytes loaded directly into GPU memory.

**Development**: External model from:

1. `CODESCRIBE_MODEL_PATH` environment variable
2. `~/.codescribe/models/whisper-large-v3-turbo-mlx-q8/`
3. `./models/whisper-large-v3-turbo-mlx-q8/` in repo

**Build Options**:

- `cargo build --release` → embedded model (default)
- `CODESCRIBE_NO_EMBED=1 cargo build --release` → dev-only (not supported for distribution)

Model files required:

- `config.json`
- `weights.safetensors` (~894MB)
- `tokenizer.json`
- `mel_filters.npz`

## File Structure

```
CodeScribe/
├── src/                      # codescribe crate (backend library)
│   ├── whisper/              # Embedded + singleton Whisper engine
│   ├── audio/                # Recorder + StreamingRecorder
│   ├── hotkeys/              # CGEventTap hotkey handler
│   ├── controller.rs         # Recording/transcription orchestration (uses StreamingRecorder)
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
├── docs/
│   ├── ARCHITECTURE.md       # This file
│   ├── WHISPER_LIVE.md        # Embedded + streaming transcription (DONE)
│   └── TEAM_SETUP.md         # Team setup guide
└── .ai-agents/               # Planning/internal docs
```

## Implementation Status

### ✅ Completed (v0.6.2)

- **Whisper Live (Streaming)** - transcription happens during recording (chunking + overlap + dedup)
- **Hotkeys** - CGEventTap integration, hold Ctrl/Ctrl+Shift modes, double-Option toggle
- **Embedded Model** - Model baked into binary via `include_bytes!`, zero disk I/O

### Current Capabilities

| Feature                                    | Status |
|--------------------------------------------|--------|
| Local Whisper STT (Metal GPU)              | ✅      |
| Embedded model (~888MB binary)             | ✅      |
| Global hotkeys (CGEventTap)                | ✅      |
| AI formatting (Responses API)              | ✅      |
| Provider separation (formatting/assistive) | ✅      |
| Tray app with submenus                     | ✅      |
| Tauri GUI (Voice Lab, Settings)            | ✅      |
| History with slug filenames                | ✅      |

---

**Made with (งಠ_ಠ)ง by the ⌜ CodeScribe ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026
Maciej & Monika + Klaudiusz (AI) + Junie (AI)**
