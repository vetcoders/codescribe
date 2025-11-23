# Changelog

This changelog summarizes the two recent stabilization phases based on the
branch diffs you requested. Dates follow the Git history recorded in this repo.

## v0.4.3 – 2025-11-21

- TODO: Add release notes.

## v0.4.1 – 2025-11-11

- TODO: Add release notes.

## v0.4.0 – 2025-11-11

- **License clarification** – Switched from MIT to BSD 4-Clause (Original BSD) to
  make attribution to Maciej Gad & LibraxisAI explicit in any advertising or
  bundled distribution. If this acknowledgement requirement becomes a burden for
  downstream adopters we can soften it to BSD 3-Clause, but for now the
  advertising clause captures the desired attribution policy.
- **Configurator hardening** – `hardware_detector.py` now works cross-platform,
  checks for Ollama/Tailscale binaries before probing, scales MAX tokens with
  available RAM, and no longer spews text unless the CLI entry point runs.
- **First-run portability** – onboarding config lives in a platform-aware
  support directory, carries a `config_version`, and logs JSON errors instead of
  silently dropping them. Cancelling the wizard leaves the config untouched so
  the user can retry next launch.
- **Backend & API hardening** – Whisper/format servers no longer configure
  logging at import time, enforce 20 MB upload limits with MIME/extension
  checks, expose SSE heartbeats so proxies stay connected, and run uvicorn via
  the fully qualified `vistascribe.whisper_server:app` target.
- **Tooling & packaging** – PID/port files are written with 0600 perms,
  packaging scripts locate `src/vistascribe/assets/icon.png` automatically,
  manual tests clean up temporary WAVs, launcher scripts pre-create `.pids/`
  and `logs/`, and the DMG Readme now includes the required BSD attribution.
- **CI & types** – Added `src/vistascribe/py.typed`, made Ruff/mypy part of the
  macOS workflow with concurrency guards, and dropped the outdated
  `docs/legacy` bundle ahead of the public release.
- **Menu robustness** – Submenus are now built before attaching to the tray,
  auto-healed if rumps strips them, and the Quit dialog activates the app so
  the alert always appears on top.

## Unreleased

- **Unified user data directory** – settings, transcript history, stats, and onboarding
  configuration now live in `$HOME/.VistaScribe/`, keeping CLI and packaged builds perfectly in
  sync. Scripts (`quickstart_mac.sh`, packaging installers) were updated accordingly and the README
  reflects the new contract.
- **First-run & quit dialogs stay on top** – both the onboarding wizard and the “Quit…” prompt now
  activate the app and float above other windows, avoiding the “app froze” confusion reported
  during testing.
- **Menu hardening** – dead `menu_manager.py` code was removed, the formatting submenu now uses the
  shared `set_submenu()` helper, and the Language menu no longer clears itself before the NSMenu
  exists. Tray submenus render immediately and auto-heal if rumps detaches them.
- **Docs & polish** – README gained CI badges, clarified the `.VistaScribe` storage layout, and
  added a quick contributing blurb. CHANGELOG now tracks the above as part of the pending release.

## Phase I – `develop` vs `main`

**Platform & Backends**
- Introduced `VistaScribeServer` as a single-instance backend runner with lazy
  MLX loading and a documented CLI so the React/Tauri Vista client can share the
  same transcription core.
- Added transcript telemetry hooks plus developer metrics scripts and new
  backend endpoint guards to tighten observability and error handling.
- Patched critical audio leaks, remote binding safeguards, and background launch
  prompts to keep recorder lifecycles predictable on macOS.

**AI & Formatting**
- Landed the Ollama LLM backend, multimodal chat client, and the initial dual
  mode AI formatting pipeline (Light+ by default, Harmony/Ollama assistive mode
  when enabled).
- Added conveniences for Polish Whisper fine-tunes, refined model selection, and
  relaxed overly aggressive formatting to avoid Markdown hallucinations.

**UX & Tooling**
- Rebuilt the tray menus (appearance, permissions, history) and introduced live
  transcription glyph customizations plus extra developer tools.
- Added `.env.example`, run/debug profiles, troubleshooting docs, MLX cheat
  sheets, and improved diagnostics for quickstart scripts.

## Phase II – `develop` vs `functional`

**Runtime Modularization**
- Split the monolithic tray runtime into controllers/mixins (`recording`,
  `history`, `models`, `appearance`, etc.) so hotkeys, menus, and async loops can
  evolve independently.
- Added compatibility shims for legacy imports (`whisper_server`, client config)
  to keep Vista integrations working during the refactor.

**Configuration & Tests**
- Simplified environment management: consolidated env templates, updated
  sitecustomize hooks, and made the settings store the single source of truth for
  AI/provider toggles.
- Refactored manual Ollama tests to share helpers and moved utility specs under
  `tests/manual`, alongside new pytest-based diagnostics.

**Quality of Life**
- Hardened exception handling across the client/backend boundary, added smoke
  tests around the new controllers, and refreshed documentation to mirror the
  current tree/layout.
