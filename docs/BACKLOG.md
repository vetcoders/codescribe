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
- **Status**: 🟡 Implemented but not actively used for auto-stop
- **Implementation**: `vad_triggered` flag in `src/controller.rs` (lines 185, 289-295, 551-557)
- **Goal**: Enable "Hands-off" mode where recording stops automatically after silence
- **Trigger**: Double-press Option to start → Listen → Silence (3-8s threshold from envvars) → Stop & Transcribe
- **Files**: `src/controller.rs` (needs state machine update to use VAD flag for auto-stop)

### 1.2. Overlay Text Preview
- **Status**: 🟡 Code exists but not fully integrated
- **Implementation**: `src/voice_chat_ui.rs` (400 lines) — macOS Native Overlay with:
  - `show_voice_chat_overlay()` / `hide_voice_chat_overlay()`
  - `append_voice_chat_delta()` — streaming text support
  - `get_accumulated_text()` — retrieve full text
  - `is_voice_chat_overlay_visible()` — visibility check
- **Current Goal**: Always-on-top overlay showing real-time transcription chunks
- **Target Goal**:
  - User sees live Whisper chunks on overlay during speech
  - After AI response: growing text block with copy-to-clipboard option
  - Intelligent auto-hide (on silence or manual dismiss)

### 1.3. Hands-off Mode (Target Implementation)
- **Status**: 🔴 Not yet implemented (current version is basic toggle)
- **Description**: Enhanced interaction mode combining VAD + Overlay + streaming preview
- **Target Flow**:
  1. **Trigger**: Double-press `Option` key → starts **listening** (not immediate transcription)
  2. **Overlay appears**: Always-on-top, shows real-time Whisper chunks
  3. **Recording active**: User speaks, sees live transcription on overlay
  4. **VAD detects silence**: Threshold from envvars (e.g., 3s or 8s)
  5. **Transcription/Response**: Depending on mode:
     - `formatting only` (left_alt) → AI-formatted version of user speech, growing text block, copy option
     - `assistive` (right_alt) → augmented AI response + (future) TTS via SSE stream tags
  6. **Cleanup**: Overlay hides or shows final result with copy action

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
