# CodeScribe Architecture

> Created by M&K (c)2026 VetCoders

## System Overview

```mermaid
flowchart TB
    %% High-level packaging / layers

    subgraph APP[codescribe crate (bin/daemon)]
        direction LR
        HK[hotkeys/\n(macOS CGEventTap)]
        CTRL[controller.rs]
        IPC_SERVER[ipc/server.rs]
        TRAY[tray/]

        subgraph CORE[codescribe-core crate]
            direction LR
            WH[whisper/\n(embedded + singleton)]
            CO[config/]
            AU[audio/\n(cpal + stream)]
            IPC_CORE[ipc types]
        end

        APP --> CORE
    end

    WH --> MODEL[Whisper Model\nlarge-v3-turbo\nmlx-q8 (~888MB)\n(embedded in bin)]

    subgraph TOOLS[Quality & CLI Tools]
        CLI[codescribe-quality]
        LOOP[codescribe-loop]
    end

    APP -.-> TOOLS
```

## Runtime & Quality Tools

- **IPC Server**: Unix socket server (`src/ipc/`) allowing external clients (or CLI tools) to control the
  recording/transcription session and receive real-time events.
- **Quality Loop**: Automated self-tuning system (`codescribe-core/src/quality_loop.rs`) that evaluates transcription accuracy.
- **Quality Report**: Batch quality reports (`codescribe-core/src/quality_report.rs`) for transcription analysis.
- **Stream Postprocess**: Pipeline stage (`codescribe-core/src/stream_postprocess.rs`) that applies semantic gating and cleanup to live
  chunks.


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
├── codescribe-core/          # Core library (Whisper, audio, config, quality)
│   ├── src/
│   │   ├── whisper/           # Embedded + singleton Whisper engine
│   │   ├── audio/             # Recorder + StreamingRecorder
│   │   ├── ipc/               # IPC types
│   │   ├── stream_postprocess.rs # Semantic gating for live chunks
│   │   ├── quality_loop.rs    # Automated quality loop
│   │   ├── quality_report.rs  # Batch quality reports
│   │   ├── config/            # Configuration management
│   │   └── ...
├── src/                      # codescribe crate (daemon/CLI)
│   ├── ipc/                  # IPC server (Unix socket)
│   ├── hotkeys.rs            # CGEventTap hotkey handler
│   ├── tray/                 # Tray app setup + menu
│   ├── controller.rs         # Recording/transcription orchestration
│   └── ...
├── src/bin/                  # CLI tools (codescribe-quality, codescribe-loop)
├── docs/
│   ├── ARCHITECTURE.md       # This file
│   ├── WHISPER_LIVE.md       # Embedded + streaming transcription (DONE)
│   └── TEAM_SETUP.md         # Team setup guide
└── tests/
```

## Implementation Status

### ✅ Completed (current release)

- **Whisper Live (Streaming)** - transcription happens during recording (chunking + overlap + dedup)
- **Hotkeys** - CGEventTap integration, hold Ctrl/Ctrl+Shift modes, double-Option toggle (left/right)
- **Embedded Model** - Model baked into binary via `include_bytes!`, zero disk I/O
- **CodeScribe Core** - Extracted as separate crate (`codescribe-core`)

### 🟡 In Progress (implemented but not fully integrated)

- **VAD (Voice Activity Detection)** - `vad_triggered` flag exists in `controller.rs`, not used for auto-stop yet
- **Overlay Text Preview** - Code exists in `voice_chat_ui.rs`, not fully integrated with recording flow

### Current Capabilities

| Feature                                    | Status |
|--------------------------------------------|--------|
| Local Whisper STT (Metal GPU)              | ✅      |
| Embedded model (~888MB binary)             | ✅      |
| Global hotkeys (CGEventTap)                | ✅      |
| AI formatting (Responses API)              | ✅      |
| Provider separation (formatting/assistive) | ✅      |
| Tray app with submenus                     | ✅      |
| History with slug filenames                | ✅      |
| IPC server (runtime interface)             | ✅      |
| Stream postprocess (semantic gating)       | ✅      |
| Quality loop + report                      | ✅      |
| CodeScribe Core separation                 | ✅      |
| VAD (auto-stop on silence)                 | 🟡      |
| Overlay text preview                       | 🟡      |
| Tauri GUI (Voice Lab, Settings)            | 📋      |

---

**Related Documentation:**
- [`BACKLOG.md`](BACKLOG.md) — Detailed backlog with target implementations
- [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md) — Future Libraxis Qube Protocol architecture
- [`WHISPER_LIVE.md`](WHISPER_LIVE.md) — Embedded + streaming transcription details

---

**Made with (งಠ_ಠ)ง by the ⌜ CodeScribe ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026
Maciej & Monika + Klaudiusz (AI) + Junie (AI)**
