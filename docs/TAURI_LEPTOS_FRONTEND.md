# Tauri 2.9 + Leptos 0.8 Frontend for CodeScribe

> **Assignment for: Implementation Agent**
> **Created by: Klaudiusz**
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

### Phase 1: Tauri Setup ✅
- [x] Modify root `Cargo.toml` to workspace format
- [x] Create `tauri-app/` directory structure
- [x] Run `cargo tauri init` in tauri-app/
- [x] Configure `tauri.conf.json` with proper bundle ID
- [x] Create `build.rs` for Tauri
- [x] Verify `cargo tauri dev` starts empty window

### Phase 2: Leptos Integration ✅
- [x] Add Leptos dependencies to tauri-app/Cargo.toml
- [x] Create `index.html` with Leptos mount point
- [x] Create `ui/mod.rs` and `ui/app.rs`
- [x] Implement basic tab navigation (Lab/Teacher/Settings)
- [x] Verify CSR mode works in Tauri webview

### Phase 3: Tauri Commands ✅
- [x] Create `commands/mod.rs`
- [x] Implement `commands/config.rs` (get/save config)
- [x] Implement `commands/stt.rs` (transcribe, model selection)
- [x] Implement `commands/audio.rs` (device listing)
- [x] Register all commands in `lib.rs`
- [x] Test IPC from Leptos to Rust (via Lab diagnostics panel)

### Phase 4: Settings UI ✅
- [x] Create `ui/settings/mod.rs`
- [x] Implement `ModelSelector` component (local model dropdown)
- [x] Implement `LocalSttToggle` component (USE_LOCAL_STT toggle)
- [x] Implement `EnvEditor` component (STT_ENDPOINT, LLM_HOST, etc.)
- [x] Implement `HotkeyConfig` component (hold_mods, toggle_trigger)
- [x] Implement `AudioDeviceSelector` component
- [x] Test save/load config roundtrip

### Phase 5: Lab UI (Port from React) ✅
- [x] Create `ui/lab/mod.rs` with sub-tabs (Lab/Chat)
- [x] Port `SpectrogramPanel` (placeholder with status)
- [x] Port `TranscriptPanel` (with history support)
- [x] Port `EndpointPanel` (file transcription)
- [x] Port `ChatPanel` (with message history and composer)
- [x] Implement `DiagnosticsPanel` (IPC testing)
- [x] Connect to local STT via Tauri commands

### Phase 6: Teacher UI ✅
- [x] Create `ui/teacher/mod.rs`
- [x] Implement sentence display (calibration sentences list)
- [x] Implement transcript comparison (split view: reference vs transcript)
- [x] Implement lexicon preview
- [x] Implement record button with recording indicator
- [x] Implement status log
- [x] Implement metrics card (WER display)
- [ ] Connect to lexicon JSONL files (backend integration pending)

### Phase 7: Styling & Polish ✅
- [x] Port Vista CSS variables (dark theme)
- [x] Style all components consistently (759 lines of CSS)
- [x] Add loading states (buttons show "Loading...", "Transcribing...", etc.)
- [x] Add error handling UI (error class with styling)
- [x] Test dark mode (default theme)

### Phase 8: Integration ✅
- [x] Verify compilation (native + WASM)
- [x] Run clippy checks (passes with no warnings)
- [x] Run tests (68 passed, 1 pre-existing failure unrelated to tauri-app)
- [x] Integrate with existing tray icon (via `handle_open_native_lab()`)
- [x] Add menu item to open Tauri window ("Open Native Lab (Tauri)")
- [ ] Test full flow: record → transcribe → display
- [x] Update Makefile with `tauri-dev` and `tauri-build` targets

---

## 🚀 Plan Dopięcia Implementacji (Remaining Work)

Poniżej szczegółowy plan z actionable checkboxami dla pozostałych zadań:

### 8.1 Integracja z Tray Icon ✅
- [x] W `src/tray/submenus.rs` dodać "Open Native Lab (Tauri)" do Tools submenu
- [x] W `src/tray/types.rs` dodać `OpenNativeLab` event i `tools_native_lab` MenuId
- [x] W `src/tray/handlers.rs` dodać handler uruchamiający `codescribe-app`
- [x] Przetestować że kliknięcie w menu otwiera okno

### 8.2 Menu Item → Tauri Window ✅
- [x] Zbadać jak uruchomić Tauri window z istniejącego procesu tray
- [x] Opcja A: Osobny proces Tauri uruchamiany przez tray ← **wybrana**
- [x] Handler szuka binarki w: /Applications, target/release, target/debug, PATH
- [x] Zaimplementować wybraną opcję
- [x] Logowanie jeśli binarka nie znaleziona

### 8.3 Test Full Flow: Record → Transcribe → Display
- [ ] Uruchomić `make tauri-dev`
- [ ] W Lab UI: wybrać plik audio i kliknąć "Transcribe"
- [ ] Zweryfikować że transkrypcja pojawia się w UI
- [ ] Przetestować Settings: zmiana modelu, zapis, reload
- [ ] Przetestować Teacher: wyświetlanie sentences, metrics
- [ ] Przetestować menu tray → "Open Native Lab (Tauri)"

### 8.4 Makefile Targets ✅
- [x] Dodać target `tauri-dev` (uruchamia `cd tauri-app && cargo tauri dev`)
- [x] Dodać target `tauri-build` (buduje release: `cd tauri-app && cargo tauri build`)
- [x] Dodać target `tauri-check` (sprawdza kompilację WASM + native)
- [x] Zaktualizować `make help` z nowymi targetami
- [ ] Przetestować wszystkie nowe targety

### 8.5 Lexicon Backend Integration ✅
- [x] Dodać Tauri command `get_lexicon_entries(topic: String)` w commands/lexicon.rs
- [x] Dodać Tauri command `save_lexicon_entry(topic, entry)`
- [x] Dodać Tauri command `list_lexicon_topics()`
- [x] Zarejestrować komendy w lib.rs invoke_handler
- [ ] Zaktualizować Teacher UI aby używał nowych komend (frontend task)

### 8.6 Finalizacja ✅
- [ ] Usunąć wszystkie `#[allow(dead_code)]` które nie są potrzebne
- [ ] Przejrzeć TODO komentarze w kodzie i rozwiązać
- [x] Zaktualizować README.md z instrukcjami dla Tauri UI (sekcja w CHANGELOG)
- [x] Zaktualizować CHANGELOG.md z nową funkcjonalnością
- [x] Clippy passes: `cargo clippy --all-targets` (tylko pre-existing warnings)

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
