# VistaScribe

[![Tests](https://github.com/LibraxisAI/VistaScribe-dev/actions/workflows/tests.yml/badge.svg?branch=main)](https://github.com/LibraxisAI/VistaScribe-dev/actions/workflows/tests.yml)
[![Lint](https://github.com/LibraxisAI/VistaScribe-dev/actions/workflows/lint.yml/badge.svg?branch=main)](https://github.com/LibraxisAI/VistaScribe-dev/actions/workflows/lint.yml)

VistaScribe is a macOS menu-bar companion that records audio through global hotkeys, runs it
through local MLX Whisper models, and pastes the transcript directly into the currently focused
application. Optional Harmony- or Ollama-based formatting can polish the output, but the Light+
formatter keeps everything private and deterministic by default.

## Feature Highlights
- **Zero-cloud capture** – MLX Whisper (Medium, Large V3, Large V3 Turbo, PL fine-tunes) loads from
  `./models` without API keys.
- **Tray-first UX** – hold Control (press-or-hold) or double-tap Option to start/stop recording,
  with animated tray glyphs and optional chimes.
- **Deterministic Light+ formatting** – always-on cleanup that removes fillers, fixes casing, and
  stabilizes punctuation before anything touches an LLM.
- **Optional AI formatting** – Harmony-compatible providers (Libraxis/OpenAI `/v1/responses`) or
  local Ollama models for “Assistive” mode; toggled live from the Formatting submenu.
- **Shared backend** – `vistascribe_server` exposes REST + NDJSON + WebSocket streaming endpoints so
  the React/Tauri Vista client can reuse the same transcription core.
- **Developer utilities** – launch scripts, tester UI (`/tester`) with live spectrogram, and a
  persistent JSON settings store that is shared between the tray, backend, and packaged apps.

## Quick Start
1. **Install uv** (if missing): `curl -LsSf https://astral.sh/uv/install.sh | sh && exec -l $SHELL`.
2. **Install dependencies**: `uv sync` (creates/updates `.venv`).
3. **Copy env template**: `cp .env.example .env` and tweak anything you need (e.g.
   `WHISPER_VARIANT`, Harmony/Ollama endpoints). All durable settings end up in
   `$HOME/.VistaScribe/settings.json`, which the tray, backend, and packaged app share.
4. **Download models**: `uv run python scripts/get_models.py --whisper large-v3-turbo` (repeat for
   `medium` or PL fine-tunes as needed).
5. **Run the tray**: `uv run python -m vistascribe.main` (foreground) or use the launcher:
   ```bash
   ./VistaScribe start            # tray + backend as daemons (logs written to ./logs)
   ./VistaScribe tester           # ensures backend is up and opens the spectrogram UI
   ./VistaScribe stop             # stop everything started by the launcher
   ```
   For interactive dev runs (foreground, verbose logging), use `./VistaScribe-dev start` or
   `./VistaScribe-dev tester`.

## Repository Layout
```
scripts/               # helper scripts (model downloaders, quickstart, diagnostics)
src/vistascribe/       # tray + backend source (controllers, STT, LLM, assets, settings)
tests/                 # pytest suites (unit + backend streaming/tests/manual)
docs/                  # documentation (team setup, proposals, tree snapshots under docs/reports)
packaging/             # py2app + DMG build scripts and wrappers
tools/                 # auxiliary tooling (bandit config, expose_ollama helper, etc.)
logs/                  # runtime logs, generated trees, PID files (gitignored)
```
The tracked snapshot of the tree lives in `docs/reports/project-tree.txt`. Regenerate it any time
with `tree -a -L 2 > logs/project-tree.txt` and copy the portion you care about into docs.

## Configuration & Settings
- The tray/backend read lightweight config values from `.env`, but persistent toggles (formatting
  provider, preferred Whisper variant, menu toggles) live in `$HOME/.VistaScribe/` and follow you
  between the CLI, LaunchAgent, and bundled app.
- Use `.env.example` in the repo root as the single source of truth. Copy it to `.env`, keep the
  handful of variables you care about, and let the JSON settings store handle everything else.
- When automation scripts need deterministic overrides (e.g. CI, the test-instance launcher) they
  now export the relevant variables inline—no extra template files are required.
- `HARMONY_BASE_URL` no longer has a baked-in default; set it explicitly (for example
  `https://api.libraxis.cloud/llm/v1`) whenever the Harmony provider is enabled. The CLI will raise
  a helpful error if it is missing.
- `VISTASCRIBE_HOST` lets you point the tray utilities (tester, client, chat demo) at a different
  backend host. It defaults to `127.0.0.1`, but setting it to another hostname/IP saves you from
  editing random scripts.

### First-run wizard
If VistaScribe can’t find `.env` or `~/.VistaScribe/settings.json`, it launches a guided wizard that
walks through model selection, permissions, and (optionally) AI formatting providers. The dialogs
force themselves to the front of the desktop stack, so you always see the prompts immediately. To
re-run the wizard later, remove `~/.VistaScribe/settings.json` (or run `./VistaScribe fresh --yes`)
and restart the tray.

## Scripts & Automation
- `VistaScribe` / `VistaScribe-dev`: thin wrappers around `scripts/quickstart_mac.sh`. They ensure
  permissions, detach processes, handle logs, and expose helpers like `status`, `logs`, `fresh`, and
  `tester`.
- `scripts/start_test_instance.sh`: spins up a dedicated backend/tray pair on port 7237 with the
  baked-in “test” overrides (it no longer depends on a separate `.env.test`).
- `scripts/test_hotkeys.sh`: guided smoke test for the double-Option vs. Hold workflows.
- `tools/expose_ollama_tailscale.sh`: networking helper for piping Ollama through Tailscale.

## Background Server Usage
`VistaScribeServer` is a standalone FastAPI process that powers all transcription and formatting
requests. You can leave it running even after quitting the tray icon, which is handy when Vista (the
Tauri frontend) or other tooling needs a local transcription API.

- **Starting the server only** – `./VistaScribe start backend` (or `scripts/quickstart_mac.sh
  --mode backend`) launches the server as a daemon and records its port in
  `logs/vistascribe-server.port`.
- **Checking status** – `./VistaScribe status` reports whether the tray and backend processes are
  alive along with their PIDs.
- **Quitting from the tray** – the menu item is now labeled **Quit...**. When you click it you get
  three choices:
  1. **Quit App & Server** – stops the tray, sends SIGTERM to `VistaScribeServer`, and removes the
     tray icon.
  2. **Keep Background Server** – exits the tray but keeps the FastAPI server alive for Vista or any
     other client.
  3. **Cancel** – leaves everything running.

If you prefer the CLI, `./VistaScribe stop` or `python -m vistascribe.vistascribe_server stop`
performs the same cleanup.

## Quality Gates
All the usual checks can be run locally before pushing:

```bash
uvx ruff format --check .
uvx ruff check .
uvx python -m bandit -c tools/bandit.yaml -r src/vistascribe
uv run mypy                 # uses the repo’s [tool.mypy] settings
uv run pytest -q            # full suite
```
`./VistaScribe-dev lint` wraps the Ruff commands, and `.pre-commit-config.yaml` wires the same stack
into Git hooks (Ruff, pyupgrade, codespell, Bandit via `tools/bandit.yaml`, Semgrep, mypy).

## Contributing
We accept pull requests against the `develop` branch only (see [CONTRIBUTING.md](CONTRIBUTING.md)
for the full workflow, coding style, and PR checklist). In short: keep UI copy in English, run Ruff
and pytest locally, attach screenshots/logs for tray changes, and delete dead code instead of hiding
it behind flags. The GitHub Actions badges above mirror the required checks.

## Changelog
The recent stabilization work is summarized in [`CHANGELOG.md`](CHANGELOG.md), broken into two
phases: `develop` vs `main` (baseline modernization) and `develop` vs `functional` (current modular
refactor).

## Troubleshooting
- **Scripts won’t run** – ensure `scripts/quickstart_mac.sh` is executable and remove quarantine:
  ```bash
  chmod +x scripts/quickstart_mac.sh VistaScribe VistaScribe-dev
  xattr -dr com.apple.quarantine "$(pwd)"
  ```
- **CRLF headaches** – if you cloned on Windows or the files came via email:
  `brew install dos2unix && dos2unix VistaScribe VistaScribe-dev scripts/quickstart_mac.sh`.
- **No audio / permissions dialog** – macOS Privacy & Security → Microphone, Accessibility, Input
  Monitoring must allow “Python” (or the terminal app you’re using).
- **Backend port missing** – the backend rotates through 8237 → 7237 → 6237 → 5237. Check
  `logs/vistascribe-server.port` or `./VistaScribe status`, and ensure nothing else (e.g.
  `mlx_audio.server`) is bound to 8237.
- **Multiple instances** – delete `.vista_scribe.lock` if you hard-killed a tray instance, then
  relaunch. The launcher already removes stale locks when you call `./VistaScribe stop`.

## License & Contact
VistaScribe is distributed under the **BSD 4-Clause License** (Original BSD).

- Copyright (c) 2025 LibraxisAI — [contact@libraxis.ai](mailto:contact@libraxis.ai)
- All redistributions—source or binary—must retain the copyright and license text **and** include
  the acknowledgement: “This product includes software developed by LibraxisAI.”
- Review [`LICENSE`](LICENSE) for the full wording, including the advertising clause and guidance on
  how to refer to VistaScribe in marketing copy.

### Attribution requirement
Any marketing, website copy, blog post, release notes, or in-product splash screen that mentions
VistaScribe’s capabilities must contain the exact sentence above. The DMG README, macOS app
“Credits” dialog, and external release notes have been updated to reference it—mirroring that text in
your own derivatives keeps you compliant with the BSD 4-Clause license.
