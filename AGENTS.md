# Repository Guidelines

## Project Structure & Module Organization
- `main.py` (tray app), `ui.py` (menu/status), `hotkeys.py` (global shortcuts), `audio.py` + `stt.py` (recording/MLX‑Whisper), `llm.py` (light formatter), `backend.py`/`whisper_server.py` (optional HTTP server).
- `scripts/` — dev helpers (`quickstart_mac.sh`, `get_models.py`, `clean.sh`).
- `tests/` — pytest suite; assets in `assets/`; distributables in `packaging/`; local models in `models/` (git‑ignored).

## Build, Test, and Development Commands
- Setup: `uv sync` — install deps; `./scripts/get_models.py --whisper large-v3-turbo` — download models.
- Run (tray): `./scripts/quickstart_mac.sh --mode tray`.
- Run (tray+backend, bg): `./scripts/quickstart_mac.sh --mode both --daemon --log VistaScribe.log`.
- Tests: `uv run pytest -q`.
- Lint/format: `uvx ruff format . && uvx ruff check .`.

## Coding Style & Naming Conventions
- Python 3.12; Ruff line length 100; import order via isort (Ruff). Prefer `snake_case` for functions/vars, `CamelCase` for classes, and module‑level constants in `UPPER_SNAKE`.
- Keep UI text short; log with level/context (module:function).

## Testing Guidelines
- Framework: pytest. Place tests in `tests/` with `test_*.py` naming; one behavior per test. Use small, deterministic audio snippets.
- Required before PR: `uvx ruff format` + `uvx ruff check` + `uv run pytest -q` on your machine.

## Commit & Pull Request Guidelines
- Commits: imperative mood (e.g., "Fix tray menu update"); group related changes; reference issue IDs in body.
- PRs: include purpose, user impact, screenshots (tray states), and a short test plan. Avoid bundling refactors with feature work.

## Git Hooks (Ruff)
- Using pre-commit: `uvx pre-commit install --install-hooks` (config in `.pre-commit-config.yaml`).
- Or plain Git hooks: `git config core.hooksPath .githooks` (runs Ruff on commit).

## Security & Configuration Tips
- Do not commit `.env`, models, or logs. Whisper path: set `WHISPER_DIR` or `WHISPER_VARIANT` in `.env`.
- macOS permissions: Microphone, Accessibility, and Input Monitoring must be granted for Terminal/Python.

