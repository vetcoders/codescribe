# CodeScribe Tauri + Leptos Migration Plan

> Status: DRAFT - requires fixes before implementation
> Created: 2026-01-10
> Updated: 2026-01-10 (added Pure Rust analysis)
> Based on: Two scaffolds from Notion/planning documents

## Executive Summary

Migrate CodeScribe frontend from React (htm) + custom HTTP server to Tauri + Leptos native desktop app.

**Current architecture:**
- Rust STT engine with tray icon (tray-icon + muda + tao)
- React 18 Lab UI served via custom HTTP server (port 8237)
- CGEventTap-based global hotkeys
- HTTP/WebSocket client to external STT endpoints (LibraxisAI cloud or local Python)

**Two proposed architectures:**

### Option A: Tauri + Leptos + Python Backend (Scaffold #1)
- Tauri 2.0 native windows
- Leptos CSR for UI
- Keep Python backend for STT/LLM
- Estimated: ~2-3 weeks

### Option B: Pure Rust (Scaffold #2)
- Tauri 2.0 native windows
- Leptos CSR for UI
- `whisper-rs` or `candle-transformers` for local STT
- NO Python dependencies
- Single binary distribution
- Estimated: ~1-2 months

**Recommendation: Start with Option A, migrate to B later**
- Option A is faster to implement
- Option B has unresolved questions (model bundling, LLM without Python)
- Incremental approach reduces risk

---

## Scaffold Comparison

| Aspect | Scaffold #1 (Python) | Scaffold #2 (Pure Rust) |
|--------|---------------------|-------------------------|
| STT | External HTTP/WS | `whisper-rs` local |
| LLM | Python FastAPI | Rust (undefined) |
| Binary size | ~10MB + Python | ~200MB+ (models) |
| User dependencies | Python, uv | None |
| Complexity | Medium | High |
| Model bundling | Not needed | Critical blocker |

---

## Common Errors in Both Scaffolds

Both scaffolds have markdown corruption from copy-paste:
```rust
// WRONG (markdown links in code):
.map_err(|e| [e.to](http://e.to)_string())?;
if let Some(obj) = [config.as](http://config.as)_object() {

// CORRECT:
.map_err(|e| e.to_string())?;
if let Some(obj) = config.as_object() {
```

Version issues:
- Leptos 0.7 doesn't exist → use 0.6.x
- cpal 0.15 is a downgrade → keep 0.16
- whisper-rs: scaffold says 0.13, actual latest is **0.15.1** (with Metal support!)

---

## Phase 1: Foundation (Must Fix First)

### 1.1 Fix Scaffold Errors
- [ ] Replace markdown-corrupted syntax (`[x.as](http://...)` -> `x.as_...`)
- [ ] Update Leptos version to 0.6.x (0.7 doesn't exist yet)
- [ ] Fix cpal version: keep 0.16, not downgrade to 0.15
- [ ] Add proper wasm-bindgen setup for Tauri + Leptos CSR

### 1.2 Create Workspace Structure
```
CodeScribe/
├── Cargo.toml           # Workspace root
├── stt-engine/          # Move current src/ here
└── tauri-app/           # New Tauri frontend
```

### 1.3 Preserve Critical Modules
- [ ] `hotkeys.rs` - CGEventTap must remain in stt-engine
- [ ] `tray/` - evaluate: migrate to Tauri tray or keep parallel?
- [ ] `voice_chat.rs` + `voice_chat_ui.rs` - integrate with Leptos

---

## Phase 2: Tauri Setup

### 2.1 Initialize Tauri
```bash
cd CodeScribe
cargo tauri init --ci
```

### 2.2 Configure tauri.conf.json
- [ ] Set proper bundle identifier
- [ ] Configure tray icon
- [ ] Add entitlements for microphone access
- [ ] Set up proper CSP for Leptos

### 2.3 IPC Commands
Required Tauri commands:
- `get_env_config` - read ~/.codescribe/.env
- `save_env_config` - write ~/.codescribe/.env
- `start_recording` - trigger STT engine
- `stop_recording` - stop STT engine
- `get_transcript` - fetch current transcript
- `send_chat_message` - forward to Python backend

---

## Phase 3: Leptos UI

### 3.1 Core Components
- [ ] `App` - main layout with tab navigation
- [ ] `SettingsView` - env editor, hotkeys, whisper model
- [ ] `LabView` - voice panel, chat panel, spectrogram
- [ ] `TeacherView` - lexicon calibration wizard

### 3.2 Port from React
Map existing React components to Leptos:

| React (assets/lab/) | Leptos (tauri-app/src/) |
|---------------------|-------------------------|
| `LabApp.js` | `app.rs` |
| `SpectrogramPanel.js` | `lab/spectrogram.rs` |
| `TranscriptPanel.js` | `lab/voice_panel.rs` |
| `ChatPanel.js` | `lab/chat_panel.rs` |
| `TeacherPanel.js` | `teacher/mod.rs` |
| `EndpointPanel.js` | `settings/env_editor.rs` |

### 3.3 Shared State
- [ ] Use Leptos signals for local state
- [ ] Use Tauri events for cross-window communication
- [ ] Consider leptos_use for common patterns

---

## Phase 4: Integration

### 4.1 Hotkey Bridge
The CGEventTap hotkey system must communicate with Tauri:
- Option A: Keep separate processes, use IPC
- Option B: Embed hotkey listener in Tauri main process
- Option C: Use Tauri plugin for global shortcuts

**Recommended: Option B** - move CGEventTap code into Tauri main.rs

### 4.2 Audio Pipeline
- [ ] Keep cpal audio capture in Rust
- [ ] Stream audio data to Leptos via Tauri events
- [ ] Render spectrogram in Canvas (web-sys)

### 4.3 Python Backend
- [ ] Use `tauri-plugin-shell` to spawn Python process
- [ ] Communicate via localhost HTTP or Unix socket
- [ ] Consider: migrate Python logic to Rust?

---

## Phase 5: Polish

### 5.1 Styling
- [ ] Port Vista design system (SCSS -> Leptos styles)
- [ ] Ensure dark theme consistency
- [ ] Add system theme detection

### 5.2 Testing
- [ ] Unit tests for Tauri commands
- [ ] Integration tests for UI flows
- [ ] Manual testing on macOS Sequoia

### 5.3 Distribution
- [ ] Configure DMG bundle
- [ ] Sign and notarize
- [ ] Update Makefile

---

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| CGEventTap doesn't work in Tauri | Critical | Test early, have fallback plan |
| Leptos learning curve | Medium | Start with simple components |
| Tauri 2.0 stability | Medium | Pin versions, test frequently |
| Python backend complexity | Low | Keep existing architecture initially |

---

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-01-10 | Scaffold requires major fixes | Markdown corruption, wrong versions |
| - | Phase 1 must complete before coding | Foundation issues block everything |
| - | Keep React Lab as fallback | Don't remove until Tauri UI proven |

---

## Open Questions

1. **Tray architecture**: Keep both tray systems during migration or switch immediately?
2. **Python backend**: Long-term migrate to Rust or keep hybrid?
3. **Hotkey plugin**: Does `tauri-plugin-global-shortcut` support modifier-only triggers?
4. **Audio streaming**: Best approach for real-time spectrogram in Leptos?

---

## Pure Rust Specific Blockers (Option B)

If pursuing Pure Rust architecture later, these must be resolved:

### 1. Model Bundling Strategy
- Whisper models: 39MB (tiny) to 1.5GB (large)
- Options:
  - Bundle tiny/base in app, download larger on demand
  - Use `include_bytes!` for small models
  - Lazy download to `~/.codescribe/models/`

### 2. LLM Without Python
Current `ai_formatting.rs` calls external LLM via HTTP. Pure Rust options:
- `candle-transformers` for local inference (GPU heavy)
- Keep HTTP client to external LLM (Ollama, LibraxisAI)
- Hybrid: local formatting, remote for complex tasks

### 3. STT Backend Options (Updated 2026-01-10)

**Three paths discovered:**

| Option | Crate | Backend | Speed (1min audio) |
|--------|-------|---------|-------------------|
| A | whisper-rs 0.15.1 | Metal GPU (whisper.cpp) | ~5s |
| B | **candle-coreml 0.3.1** | **ANE (Neural Engine)** | **~3s** ✅ |
| C | lbrx-metal + mlx-rs | Custom (your infra!) | TBD |

**Recommendation: Option B (candle-coreml)**
- Uses Apple Neural Engine - faster than GPU for inference
- Existing crate, no C++ dependency hell
- Requires Whisper model converted to .mlmodelc format
- See: https://github.com/wangchou/whisper.coreml for conversion

**Your lbrx infrastructure:**
- `/Users/maciejgad/LIBRAXIS/lbrx/crates/metal/` - Metal bindings ready!
- `mlx-rs = "0.25.1"` - MLX integration
- Can extend for custom STT if needed

### 4. Existing Module Compatibility
These modules use Python-specific patterns:
- `backend.rs` - subprocess management (would be removed)
- `client.rs` - HTTP multipart upload (would become direct call)

---

## Files to Migrate/Remove per Option

### Option A (Keep Python)
| File | Action |
|------|--------|
| `backend.rs` | Keep |
| `client.rs` | Keep, add Tauri IPC |
| `lab_server.rs` | Remove (Tauri serves UI) |
| `assets/lab/` | Port to Leptos |

### Option B (Pure Rust)
| File | Action |
|------|--------|
| `backend.rs` | Remove |
| `client.rs` | Replace with whisper-rs |
| `ai_formatting.rs` | Rewrite for candle or keep HTTP |
| `lab_server.rs` | Remove |
| `assets/lab/` | Port to Leptos |

---

*Created by M&K (c)2026 VetCoders*
