# Repository Guidelines

## Project Structure

- `src/` — main Rust crate (`codescribe`) and binaries (`src/bin/`), including tray app, audio pipeline, Whisper engine, IPC, and AI formatting.
- `tauri-app/` — Tauri GUI workspace member (Rust backend + `Trunk.toml` / web assets).
- `tests/` — Rust integration/E2E tests (`e2e_*.rs`).
- `assets/` — icons and packaged assets (e.g., `assets/AppIcon.icns`).
- `scripts/` — release + tooling scripts (DMG build, notarization, model download).
- `docs/` and `examples/` — design notes and runnable examples.
- `clients/node/` — small Node client utilities (e.g., streaming client).

## Build, Test, and Development Commands

- `make build` — debug build.
- `make release` — release build.
- `make install` — installs CLI with embedded model (very large binary).
- `make install-no-embed` — dev install without embedding; requires `CODESCRIBE_MODEL_PATH` at runtime.
- `make tauri-dev` / `make tauri-build` — run/build the Tauri GUI.
- `make lint` — `cargo fmt --check` + `cargo clippy -- -D warnings`.
- `make check` — formats Rust, runs Prettier for non-Rust files, then Clippy + Semgrep.

## Coding Style & Naming

- Rust 2024 edition. Format with `cargo fmt`; treat Clippy warnings as errors (`-D warnings`).
- Keep modules cohesive; prefer small, explicit types over “magic” globals.
- Non-Rust files (Markdown/JSON/YAML/TS/JS) are formatted via Prettier (see `make check`).

## Testing Guidelines

- Run quick local suite: `make test-quick` (workspace tests, no ignored “real API” cases).
- Full suite: `make test` (includes `--ignored`; may require API keys).
- Real-API tests are typically `#[ignore]` and expect env like `LLM_API_KEY` / `LLM_ASSISTIVE_API_KEY`.
- Add focused tests near the behavior you change (usually `tests/e2e_*.rs`).

## Commits & Pull Requests

- Prefer Conventional Commits: `feat:`, `fix:`, `refactor:`, `chore:`, `style:` (optional scope, e.g., `feat(tray): ...`).
- Keep PRs small, with a clear description, test command(s) run, and screenshots/logs for UI or tray changes.
- Install hooks locally: `make hooks` (pre-commit fmt/check + pre-push clippy + semgrep).

## Security & Configuration

- Never commit secrets. Use `.env.example` and local config at `~/.codescribe/.env` (`make config`).
- Assume macOS + Apple Silicon; some features require platform-specific APIs (hotkeys, Metal/ML).
