# Contributing to CodeScribe

Thanks for your interest in improving CodeScribe! This document explains how to set up
your environment, the expectations we have for pull requests, and the checks that run in
continuous integration.

## Ground Rules

- **Pull-request only.** Please do not push directly to `main`. Fork or create a topic
  branch, then open a PR into `develop` (maintainers fast-forward `develop` → `main` when
  release-ready).
- **English user-facing text.** UI strings, docs, scripts, and comments must be in English.
- **Security & privacy.** Do not add secrets, personal paths, or machine-specific data to the
  repo. Shared configuration lives in `.env.example` and `settings_store.py`.
- **Stay on macOS.** CodeScribe targets Apple Silicon macOS. The tooling (MLX, pyobjc,
  permissions prompts) assumes that platform.

## Environment Setup

1. Install [uv](https://docs.astral.sh/uv/getting-started/installation/).
2. Clone the repo and run `uv sync` (this creates/updates `.venv`).
3. Download at least one Whisper model for local STT:
   ```bash
   uv run python scripts/get_models.py --whisper medium
   ```
4. (Optional) Launch the tray + backend in dev mode:
   ```bash
   ./scripts/quickstart_mac.sh --mode both --dev --fg
   ```
   The script automatically installs/refreshes `pre-commit` hooks (set
   `SKIP_PRECOMMIT_BOOTSTRAP=1` to opt out).
5. If you need an Ollama-backed formatter, start `ollama serve` in another shell and
   configure the provider via the tray menu.

### Helpful Utilities

- `CodeScribe` – friendly launcher (start/stop/status/logs).
- `CodeScribe-dev` – developer toolbox (fresh cleanup, build DMG, run lint/tests, etc.).
- `.run/*.run.xml` – JetBrains run configs for the common actions above.

## Coding Standards

- Python 3.12
- Ruff line length 100, import ordering via Ruff/isort (already configured).
- `snake_case` for functions/variables, `CamelCase` for classes, `UPPER_SNAKE` for constants.
- Keep UI copy concise; prefer structured logging with module + function context.
- Avoid speculative abstractions—delete dead code instead of hiding it behind flags.

## Required Checks (Local & CI)

Before opening a PR, run the same commands that gate CI:

```bash
uvx ruff format .
uvx ruff check .
uv run pytest -q > logs/pytest.log 2>&1 & disown  # keeps pytest alive while logging
```

For quick iterations you can omit the log redirection, but the backgrounded version is
recommended for longer suites. Add focused tests when touching a subsystem (e.g.,
`scripts/test_hotkeys.sh` for hotkey regressions, `tests/manual/test_ollama_*.py` when
changing formatter plumbing).

Git hooks: `./scripts/quickstart_mac.sh …` installs them automatically unless you
explicitly set `git config core.hooksPath` (custom hook dir). In that case, run
`uvx pre-commit install --install-hooks --overwrite` manually in your environment.

## Continuous Integration

GitHub Actions currently runs:

1. **Lint (Ruff)** – `.github/workflows/lint.yml`
2. **Tests (Pytest)** – `.github/workflows/tests.yml`

Both workflows trigger on pushes to `main`, `develop`, and on every PR. Keep your PRs
green by running the local commands above before pushing.

## Pull Request Checklist

- [ ] All new code includes targeted tests (unit, hook, or manual smoke as appropriate).
- [ ] `uvx ruff format .` and `uvx ruff check .` succeed.
- [ ] `uv run pytest -q` passes (attach `logs/pytest.log` snippets for tricky failures).
- [ ] Screenshots/logs are attached for visible UI changes or tray menu updates.
- [ ] The PR description explains user impact and lists the commands you ran locally.
- [ ] Commits use imperative mood (e.g., "Add websocket stream viewer").

## Getting Help

File an issue or open a draft PR if you need early feedback. Maintainers hang out in the
repository Discussions tab; feel free to start a thread if you have questions about the
architecture or packaging.

Thanks again for helping us keep CodeScribe reliable for the Vista desktop clients!
