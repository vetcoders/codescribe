# Codescribe Architecture

> Created by Vetcoders (c)2026
>
> **2026-08-22:** transcription follows the canonical
> [four-layer engine contract](./THE_ENGINE_CONTRACT.md). The 2026-05-26
> [five-layer ADR](./ADR/2026-05-26-LAYERED_INCREMENTAL_TRANSCRIPTION.md)
> is a superseded historical proposal. Sections below describe the packaging
> and module layout that hosts the current contract.

## Layered Incremental Transcription (since 2026-05-26)

Live transcription is no longer a single Whisper stream. Exactly four machine
layers cooperate: Apple, Whisper, Lexicon + Light+, and the existing Responses
formatter. Their observations enter `AcousticLedger`; corrections, bounded
patches, annotations, and marker placement are resolved by the Rust transcript
reducer in `app/presentation/emitter.rs`. That reducer emits an immutable,
complete rendered projection through the Transcript Bus, together with the
acoustic receipts that justify it.

Swift is a projection consumer, not a second reducer.
`OverlayState.applyTranscriptProjection` is the only admitted Swift
transcript-text input. It displays and delivers the projection's complete
`renderedText` after validating its sequence, reducer revision, and acoustic
receipts. `OverlayState` does not own transcript segments, fold previews/finals,
apply replacement ranges, rebase reducer markers, or reconstruct transcript
highlights. _NEVER REWRITE FROM ZERO_ is enforced upstream by occurrence/span
identity and reducer authority, not by a Swift text-mutation API.

| Layer                        | Engine                                        | Status                                                                          | Where it lives                                                                                                                           |
| ---------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| **L0 — Apple**               | `SFSpeechRecognizer` live observer            | ✅ shipped first paint                                                          | `core/stt/apple_stt/` + Apple progressive session                                                                                        |
| **L1 — Whisper**             | Contextual observation over proven PCM spans  | ✅ exact-span Apple progressive; VAD/Whisper-first remains its own primary lane | `core/stt/tail_patcher/`, `core/stt/tail_provider.rs`, `core/stt/whisper/`                                                               |
| **L2 — Lexicon + Light+**    | Deterministic vocabulary and sentence shaping | ✅ currently wired                                                              | `core/pipeline/stream_postprocess.rs::apply_lexicon`, `core/pipeline/light_plus.rs`, progressive seals and delivery floor                |
| **L3 — Responses formatter** | Existing configured Formatting lane           | ✅ implemented behind `CODESCRIBE_INLINE_FORMAT`                                | `core/llm/inline_format.rs` schedules stable spans through `core/llm/ai_formatting.rs`; inline is scheduling, not a separate small model |

Silero sits beside these layers as VAD and PCM-time evidence. Speech boundaries,
silence duration, pause timing, and pre-roll are its truthful outputs. Named
laughter/noise classes require an optional measured provider; plain Silero does
not claim them. Final BAM is superseded and has no producer, while
`SessionFinalised` closes lifecycle only.

Existing files (`core/stt/whisper/`, `core/audio/streaming_recorder.rs`, `core/vad/silero_ort.rs`)
keep their public APIs — Layer 1 reuses them as its backend.

### Final pass routing & stop-path receipts (since W11, 2026-07-23)

The Apple bridge (`core/stt/apple_stt/codescribe-stt-bridge.swift`) probes
backends per locale: `SpeechTranscriber` only when supported **and installed**,
else `SFSpeechRecognizer` on-device (notably pl-PL — measured 0.24–2.3 s final
pass vs the 20–30 s double-Whisper era). A third lane,
`DictationTranscriber` — the SpeechAnalyzer module behind the SYSTEM dictation
and the only Apple analyzer whose catalog carries pl-PL — sits between them but
stays **off unless `CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1`**; it is a
measurement PoC (W4-A), not a shipped default. Stop-path final-pass routing is owned
by `FINAL_PASS_MODE` (`always|smart|off`, Smart default; Settings → Dictation →
"Final pass"). **Smart only** skips the full stop re-pass on a typed,
adjudicator-backed completeness decision (`StreamingCompleteness`) — never on
punctuation and never rewritten by live engine (Off stays Off; Off never forces
Whisper at stop). Live repair is orthogonal: Local Power + Apple/Auto arms the
exact-span Apple progressive patcher by default. `phase1` remains compatible
explicit arming; explicit off/invalid is degraded. The VAD/scheduler route uses
Whisper directly and refuses a second unbound lane. Dictionary/lexicon always
runs in postprocess.

Two INFO receipts prove the path in `codescribe.log`:

- `stop_path_budget: total=…s phases={rec_stop,final_pass,postproc,format,delivery} remainder=…s`
  — closes when the stop pipeline returns; remainder is explicit, never relabeled.
- `assistive_delivery_budget: total=…s outcome=delivered|no_pending_context|empty_transcript`
  — assistive overlay submission is user-triggered after the stop budget ends,
  so its real agent-runtime send reports its own wall clock.

The Settings "Active STT" row consumes the last serving verdict published by
`app/controller/serving_status.rs` through UniFFI `current_serving_verdict()` —
runtime truth (including Apple→Whisper fallback), never configured preference.

## System Overview

```mermaid
flowchart TB
    %% High-level packaging / layers

    subgraph UI["macos/Codescribe/ (SwiftUI + AppKit)"]
        direction LR
        OVERLAY[Screens/Overlay/OverlayState.swift]
        CHAT[Screens/AgentChat/]
        SETTINGS[Screens/Settings/]
        TRAY[Screens/Tray/]
    end

    subgraph BRIDGE["bridge/ (UniFFI)"]
        FFI[Rust ↔ Swift bindings\nmake app-bindings]
    end

    subgraph APP["app/ (Rust app layer)"]
        direction LR
        HK[os/hotkeys/]
        CTRL[controller/]
        PRES[presentation/emitter.rs\nRust transcript reducer]
        BUS[presentation/transcript_bus.rs\nimmutable projection + acoustic receipts]
        AGENT[agent/]
    end

    subgraph CORE["core/ (portable)"]
        direction LR
        WH[stt/whisper/]
        APPLE[stt/apple_stt/]
        TAIL[stt/tail_patcher/]
        CO[config/]
        AU[audio/]
        IPC_CORE[ipc types]
    end

    CORE -->|observations| LEDGER[AcousticLedger]
    LEDGER --> PRES
    PRES --> BUS
    BUS --> BRIDGE
    BRIDGE -->|CsTranscriptProjectionEvent| UI

    WH --> MODEL[Whisper Model\nlarge-v3-turbo\nfp16 only\nembedded or runtime-loaded]

    subgraph TOOLS[Quality & CLI Tools]
        TEACH[bin/codescribe-teacher]
        QUBE[bin/qube_daemon + qube_report]
    end

    APP -.-> TOOLS
```

## Module Architecture

### Recording Flow

```
┌─────────────┐    ┌────────────┐    ┌───────────────┐    ┌──────────────┐
│ CGEventTap  │───►│ os/hotkeys/│───►│ controller/   │───►│ STT observers│
│ (macOS API) │    │            │    │   mod.rs      │    │ Apple/Whisper│
└─────────────┘    └────────────┘    └───────────────┘    └──────┬───────┘
       │                                                         ▼
       │                                               ┌─────────────────┐
       │                                               │ AcousticLedger  │
       │                                               │ + Rust reducer  │
       │                                               └────────┬────────┘
       │                                                        ▼
       │                                               ┌─────────────────┐
       │                                               │ complete Swift  │
       │                                               │ projection      │
       │                                               └─────────────────┘
       │                                                 (display/delivery)
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
├── app/                          # Rust app layer (state machine, OS integration)
│   ├── controller/               # Recording state machine, stop-path, serving truth
│   ├── os/                       # Hotkeys (CGEventTap), permissions, clipboard,
│   │                             #   selection, hold badge, tray status, thermal
│   ├── presentation/             # Rust transcript reducer + immutable projection bus
│   ├── agent/                    # Agent loop, tools, monitor
│   └── agent_delivery.rs         # Voice → thread delivery gateway
│
├── bridge/                       # UniFFI bridge (Rust ↔ Swift); `make app-bindings`
│
├── macos/Codescribe/             # The macOS app UI — SwiftUI/AppKit
│   ├── App.swift                 # AppDelegate / lifecycle
│   ├── Core/                     # AppModel, ComposerDictation, chat/thread engines
│   ├── Screens/
│   │   ├── Overlay/              # OverlayState.swift — projection display/delivery boundary
│   │   ├── AgentChat/            # Assistive chat surface
│   │   ├── Settings/             # Settings window + SettingsViewModel
│   │   ├── Onboarding/           # First-run flow
│   │   └── Tray/                 # Menu bar UI
│   ├── Services/                 # UpdaterService (Sparkle), platform services
│   └── DesignSystem/             # Tokens, shared components
│
├── bin/                          # CLI binaries
│   ├── codescribe-teacher.rs     # Teacher / correction replay
│   ├── qube_daemon.rs            # Qube donor daemon
│   └── qube_report.rs            # Qube reporting
│
├── tests/                        # Integration/E2E tests
├── assets/                       # Icons + packaged assets
├── scripts/                      # Release + tooling scripts
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

### Agent Chat UI Components (`macos/Codescribe/Screens/AgentChat/`)

The Rust AppKit `ui/voice_chat/` module (`mod.rs` / `api.rs` / `handlers.rs` / `state.rs`,
`VoiceChatOverlayState`) no longer exists — the surface was rewritten in Swift.

| Module                              | LOC  | Purpose                                         |
| ----------------------------------- | ---- | ----------------------------------------------- |
| `AgentChatStore.swift`              | 2464 | Chat/thread state, config + thread change buses |
| `MessageList.swift`                 | 1535 | Message rendering, streaming assistant bubbles  |
| `ChatComponents.swift`              | 1008 | Shared bubble / attachment / tool components    |
| `Composer.swift`                    | 823  | Input composer (dictation, attachments, send)   |
| `ThreadRail.swift`                  | 498  | Thread list rail                                |
| `AgentChatView.swift`               | 473  | Screen composition                              |
| `ComposerTextView.swift`            | 370  | NSTextView bridge for the composer              |
| `AssistivePromptPresentation.swift` | 346  | Assistive-lane prompt presentation              |

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

**Current runtime truth**: daily builds keep Whisper and MiniLM weights out of
Cargo artifacts. MiniLM loads from the signed app resource (or HF cache for CLI
and development); Whisper resolves from the paths below. Explicit fat builds
may still opt into binary embedding:

1. `CODESCRIBE_MODEL_PATH` environment variable
2. `~/.codescribe/models/whisper-large-v3-turbo/` (fp16 default)
3. A complete explicitly configured Hugging Face snapshot

Every candidate must contain config, tokenizer, mel filters and safetensors
weights, and must pass config plus safetensors-header checks proving that it is
not quantized. Q8 has no runtime fallback path.

MiniLM resolution: `CODESCRIBE_EMBEDDER_PATH`, then
`Codescribe.app/Contents/Resources/models/embedder`, then the configured/default
Hugging Face cache snapshot. `CODESCRIBE_EMBED_EMBEDDER=1` is the explicit
binary-embed escape hatch.

## Related Documentation

- [`guide/README.md`](guide/README.md) — User documentation
- [`WHISPER_LIVE.md`](WHISPER_LIVE.md) — Runtime Whisper + streaming transcription
- [`TEAM_SETUP.md`](TEAM_SETUP.md) — Developer setup guide

---

**Made with ⌜ Codescribe ⌟ by Vetcoders (c) 2024-2026**
