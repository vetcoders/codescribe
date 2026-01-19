# CodeScribe Backlog & Roadmap

## 1. Core / Backend (CodeScribe Daemon)

### 1.1. Voice Activity Detection (VAD)
- **Status**: Implemented (`vad_triggered` in controller) but not actively used for auto-stop.
- **Goal**: Enable "Hands-off" mode where recording stops automatically after silence.
- **Trigger**: Double-press Option to start -> Listen -> Silence (3-8s) -> Stop & Transcribe.
- **Files**: `src/controller.rs` (needs state machine update).

### 1.2. Overlay Text Preview
- **Status**: Code exists in `src/voice_chat_ui.rs` (macOS Native Overlay).
- **Goal**: "Always-on-top" overlay showing real-time transcription.
- **Requirement**: Intelligent auto-hide (when silence detected or manually).

### 1.3. Hands-off Mode
- **Description**: A new interaction mode combining VAD and Overlay.
- **Flow**:
  1. Trigger (Double Option).
  2. Overlay appears.
  3. Recording starts.
  4. Real-time text streams to overlay.
  5. User stops speaking -> VAD detects silence.
  6. Recording stops -> Final transcript -> Paste/Action.

## 2. Frontend / GUI (Tauri App)

### 2.1. "Copy Transcript" Button
- **Status**: UI button exists in "Voice Lab".
- **Issue**: Currently logs "TODO" instead of copying.
- **Fix**: Implement clipboard copy via browser API (since it's WASM/WebView) or IPC command to Daemon.

### 2.2. Hotkey Configuration
- **Status**: `hotkey_integration.rs` was removed from Tauri App (duplicated/dead code).
- **Goal**: Allow configuring Daemon hotkeys via GUI (IPC `SaveConfig`).

## 3. Architecture

### 3.1. Tesseract Protocol (Future)
- **Goal**: Move to WebSocket-based "Single Stream" architecture.
- **Status**: Conceptual (`docs/ARCHITECTURE_VISION.md`).
- **Next Steps**:
  - Implement WebSocket server in Core/Daemon.
  - Implement Tag Demuxer.
  - Integrate TTS.

### 3.2. CodeScribe Core Separation
- **Status**: ✅ Completed.
- **Details**: `codescribe_core` crate extracted. `codescribe` (Daemon) and `tauri-app` (GUI) both depend on Core.
