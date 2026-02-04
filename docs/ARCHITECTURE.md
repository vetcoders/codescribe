# CodeScribe Architecture

> Created by M&K (c)2026 VetCoders

## System Overview

```mermaid
flowchart TB
    %% High-level packaging / layers

    subgraph APP[codescribe crate - bin/daemon]
        direction LR
        HK[hotkeys.rs]
        CTRL[controller/]
        IPC_SERVER[ipc/server.rs]
        TRAY[tray/]
        OVERLAY[voice_chat_ui/]

        subgraph CORE[codescribe-core crate]
            direction LR
            WH[whisper/]
            CO[config/]
            AU[audio/]
            IPC_CORE[ipc types]
        end

        APP --> CORE
    end

    WH --> MODEL[Whisper Model\nlarge-v3-turbo\nmlx-q8 ~888MB\nembedded in bin]

    subgraph TOOLS[Quality & CLI Tools]
        CLI[codescribe-quality]
        LOOP[codescribe-loop]
    end

    APP -.-> TOOLS
```

## Module Architecture

### Recording Flow

```
┌─────────────┐    ┌────────────┐    ┌───────────────┐    ┌──────────────┐
│ CGEventTap  │───►│ hotkeys.rs │───►│ controller/   │───►│ whisper/     │
│ (macOS API) │    │            │    │   mod.rs      │    │   engine.rs  │
└─────────────┘    └────────────┘    └───────────────┘    └──────────────┘
       │                                    │                     │
       │                                    ▼                     ▼
       │                            ┌──────────────┐      ┌──────────────┐
       │                            │ voice_chat   │      │ transcription│
       │                            │ _ui/         │      │ _overlay.rs  │
       │                            └──────────────┘      └──────────────┘
       │
  Ctrl hold → Raw mode (no AI)
  Ctrl+Shift hold → Assistive mode (AI)
  Double Option → Toggle mode (respects AI setting)
```

### Voice Chat UI (Mission Control)

```
┌─────────────────────────────────────────────────────────────────┐
│ Status Header                                        [Collapse] │
├─────────────────────────────────────┬───────────────────────────┤
│ LEFT PANEL (60%)                    │ RIGHT PANEL (40%)         │
│                                     │                           │
│ Chat bubbles (NSStackView)          │ [Transcriptions][Settings]│
│ ┌─────────────────────────────┐     │                           │
│ │ User message (blue, right)  │     │ Draft files list          │
│ └─────────────────────────────┘     │ [Format] [Copy] [Augment] │
│       ┌─────────────────────────┐   │                           │
│       │ AI response (gray,left) │   │ Settings toggles          │
│       └─────────────────────────┘   │ [Edit Config] [Edit Prompt]│
│                                     │                           │
│ [Auto] [📎] [Input...] [Send]       │                           │
└─────────────────────────────────────┴───────────────────────────┘
```

## File Structure

```
CodeScribe/
├── codescribe-core/              # Core library (portable, no macOS deps)
│   └── src/
│       ├── whisper/              # Embedded + singleton Whisper engine
│       │   ├── engine.rs         # Transcription logic
│       │   ├── singleton.rs      # Global instance (lazy init)
│       │   └── embedded.rs       # Model bytes (include_bytes!)
│       ├── audio/                # Recorder + StreamingRecorder
│       │   ├── recorder.rs       # cpal audio capture
│       │   └── streaming_recorder.rs  # Live transcription
│       ├── config/               # Configuration management
│       ├── stream_postprocess.rs # Semantic gating for live chunks
│       ├── quality_loop.rs       # Self-improvement loop
│       ├── quality_report.rs     # Batch quality reports
│       └── ipc/                  # IPC types
│
├── src/                          # codescribe crate (macOS-specific)
│   ├── main.rs                   # CLI entry (daemon/transcribe)
│   ├── lib.rs                    # Library exports
│   │
│   ├── controller/               # Recording state machine
│   │   ├── mod.rs                # RecordingController impl
│   │   ├── types.rs              # State, HotkeyInput, etc.
│   │   ├── helpers.rs            # Session state, callbacks
│   │   └── tests.rs              # Controller tests
│   │
│   ├── voice_chat_ui/            # Voice Chat Overlay (Mission Control)
│   │   ├── mod.rs                # UI creation (AppKit)
│   │   ├── state.rs              # VoiceChatOverlayState
│   │   ├── handlers.rs           # Objective-C callbacks
│   │   └── api.rs                # Public API functions
│   │
│   ├── tray/                     # System tray menu
│   │   ├── mod.rs                # Tray setup
│   │   ├── menu.rs               # Menu creation
│   │   ├── handlers.rs           # Menu actions
│   │   ├── icons.rs              # Icon generation
│   │   └── types.rs              # MenuIds, TrayMenuEvent
│   │
│   ├── hotkeys.rs                # CGEventTap handler
│   ├── transcription_overlay.rs  # Simple text overlay
│   ├── ui.rs                     # Badge, Dock icon
│   ├── ui_helpers.rs             # AppKit utilities
│   ├── clipboard.rs              # Paste to active app
│   ├── permissions.rs            # macOS permission checks
│   └── ipc/                      # IPC server (Unix socket)
│
├── src/bin/                      # CLI tools
│   ├── codescribe_quality.rs     # Batch quality reports
│   └── codescribe_loop.rs        # Self-improving loop
│
├── docs/
│   ├── guide/                    # User documentation
│   │   ├── README.md             # Quick start
│   │   ├── installation.md
│   │   ├── modes.md
│   │   ├── chat-overlay.md
│   │   ├── settings.md
│   │   ├── troubleshooting.md
│   │   └── privacy.md
│   ├── ARCHITECTURE.md           # This file
│   ├── WHISPER_LIVE.md           # Streaming transcription
│   ├── TEAM_SETUP.md             # Developer setup
│   └── future/                   # Aspirational docs
│       ├── ARCHITECTURE_VISION.md
│       └── FEASIBILITY_ANALYSIS.md
│
└── tests/                        # Integration tests
```

## Key Components

### Controller State Machine

```rust
// src/controller/types.rs
pub enum State {
    Idle,      // Ready for input
    RecHold,   // Recording (hold mode)
    RecToggle, // Recording (toggle mode)
    Busy,      // Processing transcription
}
```

State transitions:

- `Idle` + Ctrl down → (800ms delay) → `RecHold`
- `Idle` + Double Option → `RecToggle`
- `RecHold` + Ctrl up → `Busy` → `Idle`
- `RecToggle` + Double Option → `Busy` → `Idle`
- `RecToggle` + 5s silence (VAD) → `Busy` → `Idle`

### Mode Determination

```rust
// src/controller/mod.rs - handle_hotkey_event()
match (hotkey, flags) {
    (Hold, no_shift)  => force_raw = true,   // Ctrl: always raw
    (Hold, shift)     => assistive = true,   // Ctrl+Shift: always AI
    (Toggle, force_ai)=> force_ai = true,    // Left Option x2: force AI
    (Toggle, _)       => /* respects AI_FORMATTING_ENABLED */
}
```

### Voice Chat UI Components

| Module        | LOC | Purpose                          |
| ------------- | --- | -------------------------------- |
| `mod.rs`      | 632 | UI creation with AppKit          |
| `api.rs`      | 589 | Public API (update_status, etc.) |
| `handlers.rs` | 450 | Objective-C action handlers      |
| `state.rs`    | 148 | VoiceChatOverlayState struct     |

### Whisper Engine

- **Singleton pattern**: One global instance, lazy initialized
- **Metal acceleration**: Uses Apple GPU via candle-core
- **Streaming**: Chunks processed during recording
- **Embedded**: Model bytes in binary (~888MB)

## Implementation Status

| Feature                                      | Status |
| -------------------------------------------- | ------ |
| Local Whisper STT (Metal GPU)                | ✅     |
| Embedded model (~888MB binary)               | ✅     |
| Global hotkeys (CGEventTap)                  | ✅     |
| Three recording modes (Raw/Assistive/Toggle) | ✅     |
| Voice Chat UI (split panel)                  | ✅     |
| Chat bubbles (NSStackView)                   | ✅     |
| Drafts panel with tabs                       | ✅     |
| Settings in overlay                          | ✅     |
| AI formatting (Responses API)                | ✅     |
| Streaming AI responses                       | ✅     |
| Tray app with submenus                       | ✅     |
| History with slug filenames                  | ✅     |
| IPC server (runtime interface)               | ✅     |
| Stream postprocess (semantic gating)         | ✅     |
| Quality loop + report                        | ✅     |
| CodeScribe Core separation                   | ✅     |
| VAD (auto-stop on silence)                   | ✅     |
| Transcription overlay                        | ✅     |
| Tauri GUI (future)                           | 📋     |

## Model Location

**Release Builds**: Model embedded via `include_bytes!` (~888MB total).
Zero disk I/O, model bytes loaded directly into GPU memory.

**Development**: External model from:

1. `CODESCRIBE_MODEL_PATH` environment variable
2. `~/.codescribe/models/whisper-large-v3-turbo-mlx-q8/`
3. `./models/whisper-large-v3-turbo-mlx-q8/` in repo

## Related Documentation

- [`guide/README.md`](guide/README.md) — User documentation
- [`WHISPER_LIVE.md`](WHISPER_LIVE.md) — Embedded + streaming transcription
- [`TEAM_SETUP.md`](TEAM_SETUP.md) — Developer setup guide
- [`BACKLOG.md`](BACKLOG.md) — Feature backlog
- [`future/ARCHITECTURE_VISION.md`](future/ARCHITECTURE_VISION.md) — Libraxis Qube Protocol vision

---

**Made with ⌜ CodeScribe ⌟ by Maciej & Monika + Klaudiusz (AI) (c) 2024-2026**
