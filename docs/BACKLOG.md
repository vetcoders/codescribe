# CodeScribe Backlog & Roadmap

> Last updated: 2026-02-07

---

## ✅ Completed Features

### Recording Modes

| Feature                        | Status | Files                                  |
| ------------------------------ | ------ | -------------------------------------- |
| Hold Mode (Fn = Raw)           | ✅     | `app/controller/`, `app/os/hotkeys.rs` |
| Assistive Mode (Fn+Shift = AI) | ✅     | `app/controller/`, `app/os/hotkeys.rs` |
| Toggle Mode (Double Option)    | ✅     | `app/controller/`, `app/os/hotkeys.rs` |
| VAD Auto-Stop (5s silence)     | ✅     | `audio/recorder.rs`                    |

### Voice Chat UI (Mission Control)

| Feature                       | Status | Files                           |
| ----------------------------- | ------ | ------------------------------- |
| Split panel layout (60/40)    | ✅     | `app/ui/voice_chat/mod.rs`      |
| Chat bubbles (user/assistant) | ✅     | `app/ui/voice_chat/mod.rs`      |
| Streaming AI responses        | ✅     | `app/ui/voice_chat/api.rs`      |
| Transcriptions tab            | ✅     | `app/ui/voice_chat/handlers.rs` |
| Settings window               | ✅     | `app/ui/settings/`              |
| Attachments in chat           | ✅     | `app/ui/voice_chat/handlers.rs` |
| Auto-send toggle              | ✅     | `app/ui/voice_chat/state.rs`    |
| Collapsible right panel       | ✅     | `app/ui/voice_chat/mod.rs`      |

### Infrastructure

| Feature                                | Status | Files                              |
| -------------------------------------- | ------ | ---------------------------------- |
| Runtime Whisper model lookup           | ✅     | `core/stt/whisper/`, `tests/support/e2e_stt_matrix.rs` |
| Streaming transcription (Whisper Live) | ✅     | `core/audio/streaming_recorder.rs` |
| IPC Server (Unix socket)               | ✅     | `app/ipc/server.rs`                |
| Quality Loop (self-improvement)        | ✅     | `core/quality/quality_loop.rs`     |
| Quality Reports (batch analysis)       | ✅     | `core/quality/quality_report.rs`   |
| CodeScribe Core separation             | ✅     | `core/`                            |
| Tray app with submenus                 | ✅     | `app/ui/tray/`                     |

---

## 📋 Planned Features

### 1. Tauri GUI (Voice Lab)

- **Status**: 📋 Not started
- **Goal**: Standalone GUI app for voice training and settings
- **Architecture**: Tauri + Leptos WASM, imports `codescribe-core`
- **Features**:
  - Voice Lab (record/playback/compare)
  - Teacher mode (side-by-side correction)
  - Visual settings editor
- **Priority**: Low (current overlay covers most needs)

### 2. TTS Integration

- **Status**: 📋 Not started
- **Goal**: Text-to-Speech for assistive mode responses
- **Integration**: Via Libraxis Qube Protocol — `<tts>` tags in SSE stream
- **Dependency**: Requires Libraxis Qube Protocol implementation

### 3. Libraxis Qube Protocol

- **Status**: 📋 Conceptual ([docs/future/ARCHITECTURE_VISION.md](future/ARCHITECTURE_VISION.md))
- **Goal**: WebSocket-based "Single Stream" architecture
- **Key Concepts**:
  - Central orchestrator (localhost or remote Dragon)
  - Tag-based demuxing (`<speak>`, `<artifact>`, `<ui_message>`)
  - Audio streaming over WebSocket
- **Priority**: Low (current REST + SSE sufficient)

## 🔧 Technical Debt

| Item                                   | Priority | Notes                      |
| -------------------------------------- | -------- | -------------------------- |
| ~~Split legacy voice chat monolith~~   | ✅ Done  | `app/ui/voice_chat/*`      |
| ~~Split controller.rs (<1000 LOC)~~    | ✅ Done  | 4 modules created          |
| ~~Decouple Settings from overlay~~     | ✅ Done  | Separate settings window   |
| Update lexicon (Roost→Rust, etc.)      | CRITICAL | `assets/programming.jsonl` |

---

## 📊 Metrics

| Metric                | Value                |
| --------------------- | -------------------- |
| Total Rust LOC        | ~84,500              |
| `core/`               | ~38,000 LOC          |
| `app/`                | ~37,000 LOC          |
| `tests/`              | ~6,800 LOC           |
| Whisper packaging     | Runtime-loaded       |

---

**Related Documentation:**

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — System architecture
- [`WHISPER_LIVE.md`](WHISPER_LIVE.md) — Streaming transcription
- [`guide/README.md`](guide/README.md) — User documentation

---

_Created by M&K (c)2026 VetCoders_
