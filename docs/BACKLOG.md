# CodeScribe Backlog & Roadmap

> Last updated: 2026-01-19

---

## 0. Currently Working (End-to-End)

### 0.1. Hold Mode (Raw Transcript)
- **Status**: ✅ Working
- **Trigger**: Hold `Ctrl` / `Ctrl+Opt` / `Ctrl+Shift`
- **Behavior**: Press and hold → live transcription → release → paste to active app
- **Files**: `src/hotkeys.rs` (CGEventTap), `src/controller.rs`

### 0.2. Hands-off Mode (Current Implementation)
- **Status**: ✅ Working (basic)
- **Trigger**: Double-press `Option` key
- **Modes**:
  - **Double Left Option** → `ToggleNormal` (formatting only) — same as user said, but AI-formatted
  - **Double Right Option** → `ToggleAssistive` — augmented response depending on prompt
- **Current Behavior**:
  - Toggle starts recording
  - Accumulates Whisper transcription tokens
  - Returns full transcript instantly (no intermediate preview)
- **Files**: `src/hotkeys.rs` (`ToggleNormal`, `ToggleAssistive`), `src/controller.rs`

---

## 1. Core / Backend (CodeScribe Daemon)

### 1.1. Voice Activity Detection (VAD)
- **Status**: ✅ Implemented & Active
- **Implementation**: `codescribe-core/src/audio/recorder.rs` (RMS/silence logic) + `src/main.rs` (Watchdog task)
- **Goal**: Enable "Hands-off" mode where recording stops automatically after silence
- **Trigger**: Double-press Option to start → Listen → Silence (3-8s threshold) → Stop & Transcribe
- **Behavior**: Auto-stop triggers `finish_recording()` via VAD watchdog

### 1.2. Overlay Text Preview
- **Status**: ✅ Integrated
- **Implementation**: `src/voice_chat_ui.rs` + callbacks in `src/controller.rs`
- **Current Goal**: Always-on-top overlay showing real-time transcription chunks
- **Behavior**:
  - Live Whisper chunks appear during recording (via `StreamingRecorder` delta callback)
  - Live AI response chunks appear during formatting/assistive generation (via `ai_formatting` SSE callback)
  - Auto-hides after interaction

### 1.3. Hands-off Mode (Target Implementation)
- **Status**: ✅ Implemented (Ready for testing)
- **Description**: Enhanced interaction mode combining VAD + Overlay + streaming preview
- **Flow**:
  1. **Trigger**: Double-press `Option` key → starts listening
  2. **Overlay appears**: Shows "Listening..." and then live Whisper chunks
  3. **VAD detects silence**: Stops recording automatically
  4. **Transcription/Response**: Streamed to overlay (AI formatted or Assistive response)
  5. **Result**: Pasted to active app (and visible on overlay)

### 1.4. TTS Integration (Future)
- **Status**: 🔴 Not started
- **Goal**: Text-to-Speech for assistive mode responses
- **Integration**: Via Libraxis Qube Protocol — `<tts>` tags in SSE stream routed to audio output
- **Dependency**: Requires Libraxis Qube Protocol implementation (see 2.1)

---

## 2. Architecture

### 2.1. Libraxis Qube Protocol (Future)
- **Status**: 📋 Conceptual (`docs/ARCHITECTURE_VISION.md`)
- **Goal**: WebSocket-based "Single Stream" architecture with deployment neutrality
- **Key Concepts**:
  - Central orchestrator (runs on localhost or remote Dragon)
  - Tag-based demuxing (`<speak>`, `<artifact>`, `<ui_message>`)
  - Audio streaming over WebSocket
- **Next Steps**:
  1. Implement WebSocket server skeleton in Core/Daemon
  2. Implement Tag Demuxer for response stream
  3. Integrate TTS module
  4. Unify local/remote paths (always use WS, even on localhost)

### 2.2. CodeScribe Core Separation
- **Status**: ✅ Completed
- **Details**:
  - `codescribe-core` crate extracted (12,332 LOC)
  - Contains: Whisper engine, audio, config, quality_loop, quality_report, streaming, IPC types
  - `codescribe` (Daemon) depends on Core
  - CLI tools: `codescribe-quality`, `codescribe-loop`

### 2.3. Tauri App (Future)
- **Status**: 📋 Planned
- **Goal**: Tauri-based GUI app as separate product
- **Architecture**: Imports only `codescribe-core` (Whisper inference + orchestration)
- **Frontend**: Leptos WASM (Voice Lab, Teacher, Settings)
- **Note**: Diagram in README shows planned structure

---

## 3. Quality & Self-Improvement

### 3.1. CodeScribe Quality
- **Status**: ✅ Implemented
- **Location**: `codescribe-core/src/quality_report.rs` (1,520 LOC)
- **CLI**: `codescribe-quality` (`src/bin/codescribe_quality.rs`)
- **Purpose**: Batch quality reports for transcription accuracy

### 3.2. CodeScribe Loop (Self-Improvement)
- **Status**: ✅ Implemented
- **Location**: `codescribe-core/src/quality_loop.rs` (1,154 LOC)
- **CLI**: `codescribe-loop` (`src/bin/codescribe_loop.rs`)
- **Purpose**: Automated self-tuning system for increasing transcription precision
