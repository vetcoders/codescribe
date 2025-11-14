# VistaScribe — Internal Onboarding Cheatsheet

## Option A – DMG Install (fastest)
1. Copy `packaging/dmg/VistaScribe.dmg` to your Mac and open it.
2. Inside the DMG run the helpers in order:
   - `Helpers/Get Models.command` – download Whisper (Large v3 Turbo or Medium).
   - `Helpers/Install App.command` – copy the app into `/Applications` and launch it.
3. At first launch, grant macOS permissions (System Settings → Privacy & Security):
   - Microphone (Terminal/Python)
   - Accessibility (Terminal/Python)
   - Input Monitoring (Terminal/Python)
4. Usage basics:
   - Hold `Ctrl` ≥ 500 ms to record; release to paste the transcript.
   - Double‑tap `Option (⌥)` for toggle mode.

## Option B – Run from the repo (dev workflow)
1. Clone the repo and `cd VistaScribe`.
2. Install dependencies:
   ```bash
   uv sync
   ```
3. Download models (or copy them into `./models`):
   ```bash
   uv run python scripts/get_models.py --whisper large-v3-turbo
   ```
4. Start tray + backend as daemons:
   ```bash
   ./scripts/quickstart_mac.sh --mode both --daemon --log VistaScribe.log
   ```
   - Watch logs with `tail -f VistaScribe.log`.
   - Stop everything via `./scripts/quickstart_mac.sh --stop-all`.

## Shortcuts & Toggles
- **Light Plus** is always enabled; AI formatting (Harmony/Ollama) can be toggled in the tray.
- Tray → **Hotkey Settings**: pick the hold combo (Ctrl / Ctrl+Option / Ctrl+Shift / Ctrl+Command) and exclusive mode. Changes persist to `.env` automatically.
- Tray → **Feedback**: enable/disable the start chime, switch sounds (Tink/Pop), and set the volume—also persisted to `.env`.

## Logs & PID Files
- Tray (launch agent) log: `~/Library/Logs/VistaScribe.app.log`
- Quickstart log: `VistaScribe.log`, backend logs under `logs/backend.*.log`
- PID files for emergency shutdowns: `.pids/tray.pid`, `.pids/backend.pid`

## Formatting / AI Models
- Tray → **Formatting** → uncheck “AI Formatting Enabled” to stay on Light Plus only.
- To force a local LLM, set `LLM_ID=/path/to/qwen-4b` and select the LLM provider in the Formatting submenu.

## Troubleshooting
- Nothing pastes/records: re-check Accessibility, Input Monitoring, and Microphone permissions.
- No sound: tray → Feedback → Enable Start Sound; set `SOUND_VOLUME` to 0.1–0.3.
- Models missing: ensure they live in `./models` or rerun `Get Models.command`.
- Reset: `./scripts/quickstart_mac.sh --stop-all` and then start again.
