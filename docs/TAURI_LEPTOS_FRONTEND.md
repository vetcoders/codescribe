# Tauri 2.9 + Leptos 0.8 Frontend for CodeScribe

> **Assignment for: Implementation Agent**
> **Created by: Klaudiusz (Claude Opus 4.5)**
> **Date: 2026-01-10**
> **Project: CodeScribe - VetCoders**
> **Depends on: Pure Rust STT (completed by Junie)**

---

## Context & Background

### What is CodeScribe?

CodeScribe is a **speech-to-text tray application for macOS** built in Rust. It currently has:
- Rust backend with tray icon, hotkeys, audio capture
- React (htm) Lab UI served via custom HTTP server
- **NEW: Local Whisper STT** (just implemented by Junie)

### Goal of This Task

Create a **native desktop UI** using Tauri + Leptos to replace the React Lab UI and add a proper Settings window.

**Repository:** `/Users/maciejgad/hosted/VetCoders/CodeScribe`
**Branch:** `feat/pure-rust-flow`

---

## Current State (What Junie Built)

Junie implemented Pure Rust local STT. Key files:

| File | Purpose | Lines |
|------|---------|-------|
| `src/local_stt.rs` | LocalWhisperEngine with Q8 dequantization | 346 |
| `src/whisper_model.rs` | Full Whisper model (encoder/decoder/attention) | 409 |
| `src/audio_loader.rs` | Symphonia-based audio decoder | 141 |
| `src/models.rs` | Model manager (scaffold) | 53 |

**The STT backend compiles and is ready.** Your task is the frontend.

---

## Technology Stack

### Verified Versions (as of 2026-01-10)

```toml
tauri = "2.9"           # NOT 2.0 as in old scaffold
leptos = "0.8"          # NOT 0.7 as in old scaffold, requires rust 1.88+
leptos_router = "0.8"   # For tab navigation
```

### Key Leptos 0.8 Changes from 0.6/0.7

- `create_signal` → `signal()` or `RwSignal::new()`
- `create_effect` → `Effect::new()`
- `spawn_local` → `spawn::spawn_local()`
- View macro: `view! { }` unchanged but internals differ
- CSR mode: `features = ["csr"]`

---

## Architecture

```
CodeScribe/
├── Cargo.toml              # Workspace root (modify)
├── src/                    # Existing Rust core
│   ├── local_stt.rs        # Junie's STT engine
│   ├── audio.rs            # Audio capture
│   ├── config/             # Config management
│   └── ...
├── tauri-app/              # NEW: Tauri + Leptos frontend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── src/
│   │   ├── main.rs         # Tauri entry point
│   │   ├── lib.rs          # Tauri commands
│   │   ├── commands/       # IPC bridge to core
│   │   │   ├── mod.rs
│   │   │   ├── stt.rs      # Transcribe commands
│   │   │   ├── config.rs   # Config read/write
│   │   │   └── audio.rs    # Audio device list
│   │   └── ui/             # Leptos components
│   │       ├── mod.rs
│   │       ├── app.rs      # Main app with tabs
│   │       ├── lab/        # Voice Lab tab
│   │       ├── teacher/    # Teacher tab
│   │       └── settings/   # Settings tab
│   └── index.html          # Leptos mount point
└── assets/
    └── lab/                # Existing React UI (keep for reference)
```

---

## Tauri Commands (IPC Bridge)

### Required Commands

```rust
// tauri-app/src/commands/stt.rs

#[tauri::command]
async fn transcribe_audio(audio_path: String) -> Result<String, String> {
    // Call into codescribe::local_stt::LocalWhisperEngine
}

#[tauri::command]
fn get_available_models() -> Vec<String> {
    vec!["tiny", "base", "small", "medium", "large-v3"]
}

#[tauri::command]
fn get_current_model() -> String {
    // Read from config
}
```

```rust
// tauri-app/src/commands/config.rs

#[tauri::command]
fn get_config() -> Result<serde_json::Value, String> {
    // Read ~/.codescribe/.env and config.json
}

#[tauri::command]
fn save_config(config: serde_json::Value) -> Result<(), String> {
    // Write to ~/.codescribe/.env
}

#[tauri::command]
fn get_env_var(key: String) -> Option<String> {
    std::env::var(&key).ok()
}
```

```rust
// tauri-app/src/commands/audio.rs

#[tauri::command]
fn list_audio_devices() -> Vec<String> {
    // Use cpal to list input devices
}

#[tauri::command]
fn get_current_audio_device() -> Option<String> {
    // From config
}
```

---

## Leptos UI Components

### Main App Structure

```rust
// tauri-app/src/ui/app.rs

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Lab,
    Teacher,
    Settings,
}

#[component]
pub fn App() -> impl IntoView {
    let (active_tab, set_active_tab) = signal(Tab::Lab);

    view! {
        <div class="app-container">
            <nav class="tab-strip">
                <TabButton
                    label="Voice Lab"
                    active=move || active_tab.get() == Tab::Lab
                    on_click=move |_| set_active_tab.set(Tab::Lab)
                />
                <TabButton
                    label="Teacher"
                    active=move || active_tab.get() == Tab::Teacher
                    on_click=move |_| set_active_tab.set(Tab::Teacher)
                />
                <TabButton
                    label="Settings"
                    active=move || active_tab.get() == Tab::Settings
                    on_click=move |_| set_active_tab.set(Tab::Settings)
                />
            </nav>
            <main class="content">
                <Show when=move || active_tab.get() == Tab::Lab>
                    <LabView />
                </Show>
                <Show when=move || active_tab.get() == Tab::Teacher>
                    <TeacherView />
                </Show>
                <Show when=move || active_tab.get() == Tab::Settings>
                    <SettingsView />
                </Show>
            </main>
        </div>
    }
}
```

### Settings View

```rust
// tauri-app/src/ui/settings/mod.rs

#[component]
pub fn SettingsView() -> impl IntoView {
    view! {
        <div class="settings-view">
            <h1>"CodeScribe Settings"</h1>

            <Section title="STT Backend">
                <ModelSelector />
                <LocalSttToggle />
            </Section>

            <Section title="Endpoints">
                <EnvEditor />
            </Section>

            <Section title="Hotkeys">
                <HotkeyConfig />
            </Section>

            <Section title="Audio">
                <AudioDeviceSelector />
            </Section>
        </div>
    }
}
```

### Lab View (Port from React)

Reference the existing React components in `assets/lab/components/`:
- `SpectrogramPanel.js` → `lab/spectrogram.rs`
- `TranscriptPanel.js` → `lab/transcript.rs`
- `EndpointPanel.js` → `lab/endpoint.rs`
- `ChatPanel.js` → `lab/chat.rs`

---

## Styling

Use the existing Vista design system colors from `assets/lab/styles.css`:

```css
:root {
    --vista-bg-primary: #03131a;
    --vista-bg-secondary: #0a1f2c;
    --vista-text-primary: #e6f0f5;
    --vista-text-muted: #7a9aad;
    --vista-accent-mint: #64ffda;
    --vista-accent-blue: #7ca8ff;
    --vista-border-default: rgba(124, 168, 255, 0.2);
}
```

---

## Cargo Configuration

### Root Cargo.toml (modify to workspace)

```toml
[workspace]
resolver = "2"
members = [".", "tauri-app"]

[workspace.package]
version = "0.6.0"
edition = "2024"
authors = ["VetCoders <hello@vetcoders.io>"]

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1", features = ["full"] }
anyhow = "1.0"
tracing = "0.1"
```

### tauri-app/Cargo.toml

```toml
[package]
name = "codescribe-app"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
# Core crate
codescribe = { path = ".." }

# Tauri 2.9
tauri = { version = "2.9", features = ["tray-icon"] }
tauri-plugin-shell = "2"
tauri-plugin-fs = "2"

# Leptos 0.8
leptos = { version = "0.8", features = ["csr"] }

# Shared
serde.workspace = true
serde_json.workspace = true

[target.'cfg(target_os = "macos")'.dependencies]
tauri = { version = "2.9", features = ["tray-icon", "macos-private-api"] }
```

---

## Actionable TODO List

### Phase 1: Tauri Setup
- [ ] Modify root `Cargo.toml` to workspace format
- [ ] Create `tauri-app/` directory structure
- [ ] Run `cargo tauri init` in tauri-app/
- [ ] Configure `tauri.conf.json` with proper bundle ID
- [ ] Create `build.rs` for Tauri
- [ ] Verify `cargo tauri dev` starts empty window

### Phase 2: Leptos Integration
- [ ] Add Leptos dependencies to tauri-app/Cargo.toml
- [ ] Create `index.html` with Leptos mount point
- [ ] Create `ui/mod.rs` and `ui/app.rs`
- [ ] Implement basic tab navigation (Lab/Teacher/Settings)
- [ ] Verify CSR mode works in Tauri webview

### Phase 3: Tauri Commands
- [ ] Create `commands/mod.rs`
- [ ] Implement `commands/config.rs` (get/save config)
- [ ] Implement `commands/stt.rs` (transcribe, model selection)
- [ ] Implement `commands/audio.rs` (device listing)
- [ ] Register all commands in `lib.rs`
- [ ] Test IPC from Leptos to Rust

### Phase 4: Settings UI
- [ ] Create `ui/settings/mod.rs`
- [ ] Implement `ModelSelector` component
- [ ] Implement `LocalSttToggle` component
- [ ] Implement `EnvEditor` component (STT_ENDPOINT, LLM_HOST, etc.)
- [ ] Implement `HotkeyConfig` component
- [ ] Implement `AudioDeviceSelector` component
- [ ] Test save/load config roundtrip

### Phase 5: Lab UI (Port from React)
- [ ] Create `ui/lab/mod.rs`
- [ ] Port `SpectrogramPanel` (or placeholder)
- [ ] Port `TranscriptPanel`
- [ ] Port `EndpointPanel`
- [ ] Port `ChatPanel`
- [ ] Connect to local STT via Tauri commands

### Phase 6: Teacher UI
- [ ] Create `ui/teacher/mod.rs`
- [ ] Implement sentence display
- [ ] Implement transcript comparison
- [ ] Implement lexicon preview
- [ ] Connect to lexicon JSONL files

### Phase 7: Styling & Polish
- [ ] Port Vista CSS variables
- [ ] Style all components consistently
- [ ] Add loading states
- [ ] Add error handling UI
- [ ] Test dark mode

### Phase 8: Integration
- [ ] Integrate with existing tray icon
- [ ] Add menu item to open Tauri window
- [ ] Test full flow: record → transcribe → display
- [ ] Update Makefile with `tauri-dev` and `tauri-build` targets

---

## Important Notes

### DO NOT:
- Remove existing React Lab UI yet (keep as reference/fallback)
- Modify Junie's `local_stt.rs` unless necessary
- Change existing config format
- Break tray icon functionality

### DO:
- Use `codescribe` crate as dependency (not copy code)
- Follow Leptos 0.8 patterns (not 0.6/0.7)
- Use Tauri 2.9 APIs (not 1.x)
- Test on macOS Sequoia (the target platform)

### Leptos 0.8 Gotchas:
- `view!` macro requires `leptos::prelude::*`
- Signals: `let (getter, setter) = signal(value)`
- Effects: `Effect::new(move |_| { ... })`
- Async: `spawn::spawn_local(async move { ... })`

---

## Resources

### Documentation
- Tauri 2.0: https://v2.tauri.app/
- Leptos 0.8: https://leptos.dev/ (check 0.8 migration guide)
- Leptos Book: https://book.leptos.dev/

### Existing Code Reference
- React Lab: `/Users/maciejgad/hosted/VetCoders/CodeScribe/assets/lab/`
- Vista styles: `/Users/maciejgad/hosted/VetCoders/CodeScribe/assets/lab/styles.css`
- Config types: `/Users/maciejgad/hosted/VetCoders/CodeScribe/src/config/types.rs`

### Contact
If blocked, ask questions. Better to clarify than to guess wrong.

---

*Created by M&K (c)2026 VetCoders*
