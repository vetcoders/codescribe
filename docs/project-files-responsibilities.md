# Project Files – Responsibilities Map

Quick reference for what the main paths do in this repo.

## Top level
- `VistaScribe`, `VistaScribe-dev`: launchers (tray/backend start/stop, logs).
- `backend.py`, `whisper_server.py`: legacy CLI entrypoints for local STT server.
- `scripts/`: helper shell scripts (quickstart_mac, clean, tests, model fetch).
- `packaging/`: packaging/signing/notarization helpers and app wrapper scripts.
- `sitecustomize.py`: site-wide tweaks for Python startup.

## Docs & reports
- `docs/`: misc docs; `menu_tree.*` shows tray menu structure; `proposals/`, `reports/` (includes project-tree.txt capture), `TEAM_SETUP.md`, `MLX_CHEATSHEET.md`.

## Core source (`src/vistascribe`)
- `main.py`: tray entrypoint; loads `.env`, then runtime.
- `app/runtime.py`: rumps app wiring; builds menu, pulls in mixins, hotkeys loop, state.
- `app/recording_controller.py`: hotkey state machine + recorder lifecycle + paste/history glue.
- `app/mixins/*.py`: menu/backends/feedback/appearance/tools/hold orchestration.
- `app/controllers/history.py` & `app/controllers/models.py`: history menu and model picker.
- `app/menu_utils.py`, `app/status.py`: menu/status helpers.
- `audio.py`: microphone recorder (sounddevice), buffering, WAV save, diagnostics.
- `hotkeys.py`: Quartz event tap; hold/toggle detection, queue of events.
- `client.py`: HTTP client to backend (transcribe/format), server discovery/resilience.
- `backend.py`: FastAPI STT/format server (Whisper load, /transcribe, /stream/transcribe, etc.).
- `stt.py`, `llm.py`, `formatting/light_plus.py`: STT and formatting helpers.
- `ui.py`, `menu_formatting.py`, `menu_model.py`, `history.py`: UI glue, formatting/menu models.
- `path_utils.py`: repo/data path resolution; MLX path normalization.
- `settings_store.py`, `config.py`, `model_defaults.py`: config load/merge/defaults.
- `diag.py`, `event_log.py`, `metrics.py`, `stats.py`, `session_memory.py`: telemetry/diag.
- `permission_manager.py`, `login_manager.py`, `first_run.py`, `hardware_detector.py`: env/perm checks.
- `chatclient.py`, `tts.py`, `codescribe_context.py`, `context_folder.py`, `ollama_endpoint_setter.py`, `onboarding/`: optional helpers and integrations.
- `assets/`: icon and voice chat lab HTML.
- `vistascribe_server.py`, `whisper_server.py`: server entrypoints (FastAPI + legacy Whisper server).

## Tests
- `tests/`: unit/integration; `manual/` has manual flow scripts; helpers include fake hotkeys/rumps and Ollama stubs.

## Tools
- `tools/`: lint/hooks configs, misc helper scripts (e.g., expose_ollama_tailscale.sh, manifest.py).
