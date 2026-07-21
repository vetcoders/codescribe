# Codescribe Architecture

> Created by vetcoders (c)2026
>
> **2026-05-26:** transcription pipeline is now layered. See
> [ADR — Layered Incremental Transcription Pipeline](./ADR/2026-05-26-LAYERED_INCREMENTAL_TRANSCRIPTION.md)
> for the authoritative model. Sections below describe the packaging and module layout that hosts it.

## Layered Incremental Transcription (since 2026-05-26)

Live transcription is no longer a single Whisper stream. Five concurrent layers cooperate, with
Apple Speech as the live primary and Whisper / lexicon / small LLM / Silero paralingual
classifier filling in behind it. The overlay (`app/ui/overlay/`) renders the union of layer
events, never wipes and retypes — _NEVER REWRITE FROM ZERO_ is the operator-mandated invariant.

| Layer           | Engine                                                  | Module                                                       |
| --------------- | ------------------------------------------------------- | ------------------------------------------------------------ |
| 0 — Live        | Apple `SFSpeechRecognizer` (primary) · Whisper fallback | `core/stt/apple_stt/` + `core/stt/whisper/`                  |
| 1 — Tail Patch  | Whisper background diff                                 | `core/stt/tail_patcher/` (new, Phase 1)                      |
| 2 — Polish      | Lexicon + small LLM                                     | `core/lexicon/` + `core/llm/inline_polish.rs` (new, Phase 2) |
| 3 — Paralingual | Silero classifier head                                  | `core/vad/paralingual_classifier.rs` (new, Phase 3)          |
| 4 — Final BAM   | Session-end contextual pass                             | `core/pipeline/final_bam.rs` (new, Phase 4)                  |
| Orchestrator    | —                                                       | `app/controller/layered_orchestrator.rs` (new, Phase 1)      |

Existing files (`core/stt/whisper/`, `core/audio/streaming_recorder.rs`, `core/vad/silero_ort.rs`,
`app/ui/overlay/mod.rs`) keep their public APIs — the layered orchestrator reuses them as Layer 1
and Layer 3 backends. See ADR §"What is shipped today" for the gap analysis.

## System Overview

```mermaid
flowchart TB
    %% High-level packaging / layers

    subgraph APP[app/ (macOS app)]
        direction LR
        HK[os/hotkeys.rs]
        CTRL[controller/]
        IPC_SERVER[ipc/server.rs]
        TRAY[ui/tray/]
        OVERLAY[ui/overlay/ + ui/voice_chat/]
        SETTINGS[ui/settings/]

        subgraph CORE[core/ (portable)]
            direction LR
            WH[stt/whisper/]
            CO[config/]
            AU[audio/]
            IPC_CORE[ipc types]
        end

        APP --> CORE
    end

    WH --> MODEL[Whisper Model\nlarge-v3-turbo\nmlx-q8\nruntime-loaded]

    subgraph TOOLS[Quality & CLI Tools]
        CLI[bin/codescribe_quality]
        LOOP[bin/codescribe_loop]
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
       │                            │ ui/voice_    │      │ ui/overlay/  │
       │                            │ chat/        │      │              │
       │                            └──────────────┘      └──────────────┘
       │
  Fn hold → Raw mode (no AI)
  Fn+Shift hold → Assistive arm (default; Cmd selectable in Settings)
  Double Option → Toggle mode (respects AI setting)
```

### Voice Chat UI (Mission Control)

```
┌─────────────────────────────────────────────────────────────────┐
│ Status Header                                        [Collapse] │
├─────────────────────────────────────┬───────────────────────────┤
│ LEFT PANEL (60%)                    │ RIGHT PANEL (40%)         │
│                                     │                           │
│ Chat bubbles (NSStackView)          │ [Drawer][Transcription]   │
│ ┌─────────────────────────────┐     │                           │
│ │ User message (blue, right)  │     │ Draft files list          │
│ └─────────────────────────────┘     │ [Format] [Copy] [Augment] │
│       ┌─────────────────────────┐   │                           │
│       │ AI response (gray,left) │   │ Agent tab + tools          │
│       └─────────────────────────┘   │ Settings button → window   │
│                                     │                           │
│ [Attach] [Input...] [Send]          │                           │
└─────────────────────────────────────┴───────────────────────────┘
```

## File Structure

```
Codescribe/
├── core/                         # Core library (portable, no macOS deps)
│   ├── stt/whisper/              # Embedded Whisper engine
│   ├── audio/                    # Recorder + StreamingRecorder
│   ├── vad/                      # Silero VAD
│   ├── config/                   # Tiered config + defaults
│   ├── llm/                      # Responses API client
│   ├── pipeline/                 # Streaming + postprocess
│   ├── embedder/                 # MiniLM embedder
│   └── quality/                  # Quality loop + reports
│
├── app/                          # macOS app (AppKit, hotkeys, tray)
│   ├── controller/               # Recording state machine
│   ├── os/                       # Hotkeys, permissions, clipboard
│   └── ui/
│       ├── overlay/              # Dictation overlay window
│       ├── voice_chat/           # Overlay UI
│       ├── settings/             # Persistent settings window
│       ├── onboarding/           # First-run flow
│       ├── tray/                 # Menu bar UI
│       └── shared/               # UI helpers/tokens
│
├── bin/                          # CLI binaries
├── tests/                        # Integration/E2E tests
├── assets/                       # Icons + packaged assets
├── scripts/                      # Release + tooling scripts
│   │   └── types.rs              # MenuIds, TrayMenuEvent
│   │
│   ├── hotkeys.rs                # CGEventTap handler
│   ├── ui.rs                     # Badge, Dock icon
│   ├── ui_helpers.rs             # AppKit utilities
│   ├── clipboard.rs              # Paste to active app
│   ├── permissions.rs            # macOS permission checks
│   └── ipc/                      # IPC server (Unix socket)
│
├── bin/                          # CLI tools
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
│   └── TEAM_SETUP.md             # Developer setup
│
└── tests/                        # Integration tests
```

## Key Components

### Controller State Machine

```rust
// app/controller/types.rs
pub enum State {
    Idle,      // Ready for input
    RecHold,   // Recording (hold mode)
    RecToggle, // Recording (toggle mode)
    Busy,      // Processing transcription
}
```

State transitions:

- `Idle` + Fn down → (800ms delay) → `RecHold`
- `Idle` + Double Option → `RecToggle`
- `RecHold` + Fn up → `Busy` → `Idle`
- `RecToggle` + Double Option → `Busy` → `Idle`
- `RecToggle` + 5s silence (VAD) → auto‑send (stays `RecToggle`)

### Mode Determination

```rust
// app/controller/mod.rs - handle_hotkey_event()
match (hotkey, flags) {
    (Hold, no_arm)    => force_raw = true,   // Fn: always raw
    (Hold, arm_mod)   => assistive = true,   // configured arm (Shift default / Cmd alt)
    // Act-on-selection is a delivery lane when a selection is present (W10-D),
    // not a separate dead Cmd chord.
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
- **Embedded-first**: Builds embed Whisper when the snapshot is present at build time; runtime lookup from `CODESCRIBE_MODEL_PATH`, repo-local models, or HF cache remains the fallback path

## Implementation Status

| Feature                                      | Status |
| -------------------------------------------- | ------ |
| Local Whisper STT (Metal GPU)                | ✅     |
| Runtime Whisper model lookup                 | ✅     |
| Global hotkeys (CGEventTap)                  | ✅     |
| Three recording modes (Raw/Assistive/Toggle) | ✅     |
| Voice Chat UI (split panel)                  | ✅     |
| Chat bubbles (NSStackView)                   | ✅     |
| Drafts panel with tabs                       | ✅     |
| Settings window from tray + overlay          | ✅     |
| AI formatting (Responses API)                | ✅     |
| Streaming AI responses                       | ✅     |
| Attachments in chat                          | ✅     |
| Tray app with submenus                       | ✅     |
| History with slug filenames                  | ✅     |
| IPC server (runtime interface)               | ✅     |
| Stream postprocess (semantic gating)         | ✅     |
| Quality loop + report                        | ✅     |
| Codescribe Core separation                   | ✅     |
| VAD (auto-stop on silence)                   | ✅     |
| Transcription overlay                        | ✅     |
| Tauri GUI (future)                           | 📋     |

## Model Location

**Current runtime truth**: Whisper is embedded by default when the model snapshot is
available at build time. Runtime lookup remains available as a fallback when
embedding is disabled with `CODESCRIBE_NO_EMBED=1` or the build cannot embed the
model:

1. `CODESCRIBE_MODEL_PATH` environment variable
2. `~/.codescribe/models/whisper-large-v3-turbo-mlx-q8/`
3. `./models/whisper-large-v3-turbo-mlx-q8/` in repo
4. Hugging Face cache snapshots for `LibraxisAI/whisper-large-v3-turbo-mlx-q8`

## Related Documentation

- [`guide/README.md`](guide/README.md) — User documentation
- [`WHISPER_LIVE.md`](WHISPER_LIVE.md) — Runtime Whisper + streaming transcription
- [`TEAM_SETUP.md`](TEAM_SETUP.md) — Developer setup guide

---

**Made with ⌜ Codescribe ⌟ by vetcoders (c) 2024-2026**
