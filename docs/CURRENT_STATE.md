# CodeScribe - Current State (2026-01-12)

## Architecture: Pure Rust + Tauri 2 + Leptos 0.8

```
                    +------------------+
                    |   Leptos 0.8     |
                    |  (WASM Frontend) |
                    +--------+---------+
                             |
                    Tauri IPC (invoke)
                             |
                    +--------+---------+
                    |  Tauri Backend   |
                    |   (Native Rust)  |
                    +--------+---------+
                             |
              +--------------+--------------+
              |              |              |
        +-----+-----+  +-----+-----+  +-----+-----+
        | Recording |  |   STT     |  |  Config   |
        |   (cpal)  |  | (candle)  |  |  (.env)   |
        +-----------+  +-----------+  +-----------+
```

## What Works

### CLI (`codescribe transcribe`)
- Local STT via candle + Metal acceleration
- Model: `whisper-large-v3-turbo-mlx-q8` (874MB)
- E2E tests: 20/20 passed
- AI formatting via Ollama (optional)
- Language: auto-detect, Polish, English

### Tauri App
- Window renders with Leptos UI
- Tray icon with menu (Show/Settings/Quit)
- Hotkey listener (Ctrl to record)
- `withGlobalTauri: true` - IPC bridge works

## What Doesn't Work Yet

### Frontend-Backend Communication
1. **Serialization mismatch** - Fixed: added `#[serde(rename_all = "camelCase")]`
2. **Audio device enumeration** - Crashes on macOS 26 (CoreAudio SIGSEGV)
3. **Recording flow** - Not tested due to audio crash

### Build Process Issues
- `cargo tauri dev` runs `beforeDevCommand` from wrong directory
- Workaround: build manually with `trunk build` + `cargo tauri build`

## File Structure

```
CodeScribe/
├── src/                    # Core library
│   ├── lib.rs             # Public API
│   ├── main.rs            # CLI binary
│   ├── local_stt.rs       # LocalWhisperEngine
│   ├── models.rs          # ModelManager
│   ├── config/            # Config + .env loading
│   └── audio/             # Recording (cpal)
├── tauri-app/             # Tauri application
│   ├── src/
│   │   ├── lib.rs         # Backend entry + commands registration
│   │   ├── main.rs        # Native entry point
│   │   ├── commands/      # IPC handlers (stt, config, audio, recording)
│   │   ├── state.rs       # AppState (config, models, stt)
│   │   └── ui/            # Leptos frontend (WASM)
│   │       ├── app.rs     # Root component
│   │       ├── tauri.rs   # invoke() wrapper
│   │       ├── lab/       # Voice Lab tab
│   │       └── settings/  # Settings tab
│   ├── dist/              # Trunk output (WASM + JS)
│   ├── Trunk.toml         # WASM build config
│   └── tauri.conf.json    # Tauri config
└── models/                # Whisper model (bundled)
```

## Configuration

Location: `~/.codescribe/.env`

Key settings:
```
USE_LOCAL_STT=true
LOCAL_MODEL=whisper-large-v3-turbo-mlx-q8
WHISPER_LANGUAGE=auto
LLM_HOST=http://localhost:11434
```

## Known Issues to Fix

| Issue | Location | Status |
|-------|----------|--------|
| CoreAudio crash on device enum | `commands/audio.rs` | Needs fix |
| Close button kills app | Window config | Needs `on_close_requested` |
| Leptos reactive warning | `lab/mod.rs:424` | Minor |
| `cargo tauri dev` wrong cwd | CLI behavior | Workaround exists |

## Next Steps

1. Fix CoreAudio enumeration (try-catch or defer loading)
2. Test full recording → transcription flow
3. Add window hide behavior (macOS standard for tray apps)
4. Clean up Leptos reactive warnings

---
*Created by M&K (c)2026 VetCoders*
