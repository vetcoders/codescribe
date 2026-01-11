# CodeScribe Tauri Integration Roadmap
> Created by M&K (c)2026 VetCoders

## Current State (Session 2026-01-11)

### ✅ Completed
- **Phase 1**: Minimal tray menu (legacy codescribe binary)
- **Phase 2**: Tauri app with Leptos GUI + tray icon
  - Voice Lab | Teacher | Settings tabs
  - Tray: Show Window / Settings / Quit
  - IPC commands defined (stt, config, audio, lexicon)
  - WASM frontend builds with Trunk

### 🔄 In Progress
- Agent audits running:
  - IPC commands implementation status
  - UI buttons → backend mapping
  - Hotkeys integration analysis

## Architecture Decision: Single App

**Choice**: One unified Tauri app replaces both:
- Old tray-only app (`codescribe` binary)
- Separate GUI attempts

**Model**: `whisper-large-v3-turbo-mlx-q8` bundled in app
- Location: `~/.codescribe/models/` or embedded in Resources
- No model selection UI needed - single model simplifies UX

## Next Steps by Phase

### Phase 3: Documentation & Audit (Current Session)
- [ ] Create `docs/ARCHITECTURE.md` - full system diagram
- [ ] Document IPC command → backend function mapping
- [ ] Document hotkey → controller → STT flow
- [ ] Identify gaps between UI buttons and backend calls

### Phase 4: Wire Core Flow (Next Session ~2-3h)
1. **Start Streaming button** → `transcribe_audio` IPC
   - Connect to `codescribe::local_stt::transcribe()`
   - Stream audio from browser MediaRecorder → backend

2. **Hotkeys integration**
   - Option A: Tauri app spawns hotkey listener thread
   - Option B: Separate hotkey daemon communicates via IPC
   - Recommendation: Option A (single process)

3. **Settings persistence**
   - `save_config` IPC → `codescribe::config::Config::save_to_env()`
   - Load on startup

### Phase 5: Model Bundling (Next Session ~1-2h)
1. Download `whisper-large-v3-turbo-mlx-q8` (~800MB)
2. Add to `tauri.conf.json` resources
3. Modify `local_stt.rs` to load from bundle path
4. Test cold start performance

### Phase 6: Polish & Testing (~2-3h)
1. Activity glyph for tray (recording/processing states)
2. Error handling UI (toast notifications)
3. E2E tests for critical flows
4. DMG with proper signing

## Session Estimate

| Phase | Estimated Time | Sessions |
|-------|---------------|----------|
| Phase 3 (Docs) | 1-2h | Current |
| Phase 4 (Wiring) | 2-3h | 1 |
| Phase 5 (Bundle) | 1-2h | 1 |
| Phase 6 (Polish) | 2-3h | 1 |
| **Total** | **6-10h** | **3-4 sessions** |

## Critical Path

```
[Hotkeys] ─┬─→ [Controller] ─→ [Audio Capture] ─→ [Local STT] ─→ [Paste]
           │                                          ↑
[UI Button]┴─→ [Tauri IPC] ─→ [commands/stt.rs] ─────┘
```

Both entry points (hotkeys and UI) must reach the same STT backend.

## Files to Modify

### Backend (codescribe crate)
- `src/local_stt.rs` - expose as library function
- `src/controller.rs` - extract recording logic for reuse
- `src/lib.rs` - public API for Tauri

### Tauri App
- `src/commands/stt.rs` - implement `transcribe_audio`
- `src/commands/audio.rs` - real device listing
- `src/lib.rs` - hotkey thread integration
- `src/ui/lab/mod.rs` - wire buttons to invokes

### Config
- `tauri.conf.json` - add model to resources
- `Cargo.toml` - workspace dependencies

## Open Questions
1. Should hotkeys work when GUI window is hidden?
2. Paste target: active window or specific app?
3. History: show in GUI or just save to disk?
