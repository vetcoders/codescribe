# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v0.7.2-dev] – 2026-01-20

### Added
- **Hands-off Chat Overlay** – Full chat interface in overlay with history, user/assistant roles, and input field.
- **Persistence** – Chat history is preserved between sessions; messages do not disappear on close.
- **Auto-send Toggle** – UI checkbox to control automatic sending vs. draft mode.
- **Improved VAD** – 5s timeout for hands-off mode to allow for pauses; short silences (1-2s) are ignored.
- **Tray Actions** – Added "Show Chat Overlay" and "Copy Last to Clipboard" to tray menu.
- **UI Improvements** – Input field at top, reversed message flow (newest first), selectable text for copying.

### Fixed
- **Quality Gates** – Resolved `cargo check` and `make check` warnings; improved code quality.
- **Reliability** – Fixed issue where overlay would reset state unexpectedly.

## [v0.7.0] – 2026-01-17

### Added
- **Strict Embedded Policy** – Whisper model is always embedded into release binary. Zero external model files, zero exceptions.
- **IPC server** – New IPC server and message types for stable runtime integration.
- **Quality loop** – Automated transcription quality assessment loop.
- **Quality report** – Batch quality report generator with WER/CER metrics.
- **Stream postprocess** – Semantic gating and stream cleanup in live pipeline.
- **New CLI tools** – `codescribe-quality`, `codescribe-loop` for quality management.
- **serial_test** – E2E test serialization to reduce race conditions.

### Changed
- **Version unification** – Consistent versioning across the project.
- **Security hardening** – `cap-std` and file operation restrictions to allowed paths only.

### Fixed
- SSE formatting and final text collection fixes.

## [v0.6.3] – 2026-01-16

### Added
- **New hotkey architecture** – Each hotkey now determines the processing mode:
  - **Ctrl Hold** = ALWAYS RAW (fast dictation, no AI processing, ignores AI toggle)
  - **Double Option** = respects AI_FORMATTING_ENABLED toggle setting
  - **Ctrl+Shift Hold** = ALWAYS Assistive (AI assistant mode)
- **Triple-tap Option** – Quick toggle for AI Formatting (shows toast notification)
- **Shift upgrade mid-hold** – Adding Shift during Ctrl hold upgrades to Assistive mode
- **KURIER/ASYSTENT prompt system** – Adaptive system prompts that detect user intent:
  - KURIER: Pass-through mode for dictation (zero commentary)
  - KURIER+REDAGUJ: Dictation with light editing on explicit request
  - ASYSTENT: Full AI assistant mode for questions/help
- **SSE streaming by default** – OpenAI/Libraxis endpoints now use SSE streaming for
  immediate handshake and no timeout issues

### Changed
- **Timeout increased to 90s** – GPT-5.x with longer inputs needs more time
- **Token limits removed** – All token limits set to 0 (API decides). Tokens are cheap,
  lost notes are not.
- **force_raw_mode flag** – New controller state flag for explicit RAW mode override

### Fixed
- **Timeout issues with GPT-5.2** – Streaming mode eliminates 30s timeout failures

## [v0.6.2] – 2026-01-16

### Added
- **Whisper Live (streaming transcription)** – Local transcription now happens *during recording*.
  Audio from the CPAL callback is chunked and processed in the background, so on `stop()` we only
  finalize the last chunk for near-instant time-to-paste.
- **StreamingRecorder** – New streaming capture/transcription pipeline built around a non-blocking
  channel from the audio callback, plus overlap + deduplication between chunks.
- **DMG packaging improvements (embedded-only)** – Release packaging is now aligned with the
  embedded-model strategy (no bundling `Resources/models/*` that would duplicate ~900MB).

### Changed
- **Docs & pitch** – Documentation and README now highlight the core differentiator: embedded Whisper
  + live streaming transcription.

## [v0.6.1] – 2026-01-14

### Added
- **Model embedded in binary** – Release builds now include the Whisper model directly via
  `include_bytes!`, eliminating runtime model loading and disk I/O. Binary size ~888MB with
  model welded in. Debug builds still use external model path.
- **Provider separation** – New `LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}` convention
  allows different LLM providers for formatting (Ctrl hold) vs assistive mode (Ctrl+Shift hold).
- **Keep Audio toggle** – Added "Keep Audio" option to History submenu for enabling/disabling
  paired `.wav` + `.txt` storage on the fly.
- **Slug in filenames** – Transcription and audio files now include first 3 words as slug for
  easier identification: `2026-01-14_12-30-00_hello-world-test.txt`.
- **Whisper singleton API** – `whisper::singleton::init()` and `transcribe()` for shared model
  instance with automatic embedded vs external path resolution.

### Changed
- **Responses API optimization** – Instructions are now sent only on first request; subsequent
  requests rely on `previous_response_id` to preserve context, reducing payload size.
- **Build safety** – Release builds now hard-fail when model is missing. Dev-only: set
  `CODESCRIBE_NO_EMBED=1` to build without embedding (binary will require `CODESCRIBE_MODEL_PATH`
  at runtime).
- **Language enum** – Removed `Auto` variant from `Language` enum; use explicit language codes.
- **Tray menu restructure** – Reorganized submenus for History, Modes, and Settings.
- **Environment schema** – Updated `.env.example` with complete configuration reference including
  provider separation, audio settings, and debug options.

### Fixed
- **Clippy warnings** – Resolved unused imports, dead code, and type complexity warnings.
- **E2E tests** – Fixed `LLM_HOST` → `LLM_ENDPOINT` migration in all test files.
- **Borrow checker** – Fixed move-after-borrow in AI formatting trace logging.

## [v0.6.0] – 2026-01-13

### Added
- **Native desktop UI (Tauri + Leptos)** – Introduced the (now legacy) Tauri frontend with a
  three-tab interface (Voice Lab, Teacher, Settings). ([a275ae8](https://github.com/VetCoders/CodeScribe/commit/a275ae8),
  [7aa0754](https://github.com/VetCoders/CodeScribe/commit/7aa0754))
- **Pure Rust local Whisper STT (Metal GPU)** – Added local Whisper inference via
  `candle-transformers` (Metal acceleration), with long-audio chunking + language detection.
  ([268f5d0](https://github.com/VetCoders/CodeScribe/commit/268f5d0),
  [69ed294](https://github.com/VetCoders/CodeScribe/commit/69ed294))
- **Whisper decoding controls** – Added `DecodingParams` (mlx_whisper-compatible) including
  n-gram blocking and streaming callback support. ([69574fb](https://github.com/VetCoders/CodeScribe/commit/69574fb),
  [cc0d8aa](https://github.com/VetCoders/CodeScribe/commit/cc0d8aa))
- **CLI transcription + E2E pipeline tests** – Added file transcription flows and a comprehensive
  end-to-end pipeline test suite. ([d7bdb4b](https://github.com/VetCoders/CodeScribe/commit/d7bdb4b),
  [d46c62c](https://github.com/VetCoders/CodeScribe/commit/d46c62c))
- **Config convenience** – Added `--config` flag to open/create the config file. ([535270c](https://github.com/VetCoders/CodeScribe/commit/535270c))
- **UX updates** – Added badge modes + Dock icon behavior and tightened environment/API key
  requirements. ([7946c17](https://github.com/VetCoders/CodeScribe/commit/7946c17))

### Changed
- **License** – Switched the project license to Apache 2.0 and added release scripts/docs.
  ([e0e7ec1](https://github.com/VetCoders/CodeScribe/commit/e0e7ec1))
- **Backend architecture** – Removed the Python backend and updated the Rust CI pipeline to match.
  ([5c65481](https://github.com/VetCoders/CodeScribe/commit/5c65481))
- **AI formatting pipeline** – Improved configuration, workflows, and Harmony support; refined
  formatting behavior and defaults. ([e11400c](https://github.com/VetCoders/CodeScribe/commit/e11400c),
  [8a3157f](https://github.com/VetCoders/CodeScribe/commit/8a3157f),
  [d46c62c](https://github.com/VetCoders/CodeScribe/commit/d46c62c))
- **Tray menu + local STT integration** – Refactored tray menu plumbing while integrating the local
  Whisper engine and improving related behavior. ([16021b1](https://github.com/VetCoders/CodeScribe/commit/16021b1))
- **Local model packaging/loading** – Bundled a default model and updated model loading logic.
  ([13378fe](https://github.com/VetCoders/CodeScribe/commit/13378fe))
- **Cloud/STT provider work** – Refactored lab assets and migrated cloud provider integration.
  ([8392cb9](https://github.com/VetCoders/CodeScribe/commit/8392cb9))
- **Configuration consolidation** – Deduplicated configuration to a single source of truth.
  ([217a336](https://github.com/VetCoders/CodeScribe/commit/217a336))
- **Error handling/refactors** – Refactored Whisper engine imports and adopted `anyhow`.
  ([b9ac5d9](https://github.com/VetCoders/CodeScribe/commit/b9ac5d9))
- **Repository maintenance** – Restructured the repo and added conversation session tracking.
  ([07fe69f](https://github.com/VetCoders/CodeScribe/commit/07fe69f))
- **Developer ergonomics** – Applied `cargo fmt`-driven formatting fixes.
  ([f8e04ef](https://github.com/VetCoders/CodeScribe/commit/f8e04ef))

### Fixed
- **Stability** – Handled poisoned mutexes via `into_inner()` fallback to avoid cascading failures
  after panics. ([b7591ab](https://github.com/VetCoders/CodeScribe/commit/b7591ab))
- **Backend cleanup** – Ensured backend processes are killed on all known ports.
  ([417b002](https://github.com/VetCoders/CodeScribe/commit/417b002))

### Removed
- **Cleanup** – Removed unused and deprecated code to keep the build clean.
  ([68469dc](https://github.com/VetCoders/CodeScribe/commit/68469dc))

### Changed (Internal)
- **Foundations** – Landed the initial Rust-based architecture groundwork.
  ([5a17c3a](https://github.com/VetCoders/CodeScribe/commit/5a17c3a))

## v0.4.3 – 2025-11-21

- TODO: Add release notes.

## v0.4.1 – 2025-11-11

- TODO: Add release notes.

## v0.4.0 – 2025-11-11

- **License clarification** – Switched from MIT to BSD 4-Clause (Original BSD) to
  make attribution to Maciej Gad & Loctree explicit in any advertising or
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
  the fully qualified `codescribe.whisper_server:app` target.
- **Tooling & packaging** – PID/port files are written with 0600 perms,
  packaging scripts locate `src/codescribe/assets/icon.png` automatically,
  manual tests clean up temporary WAVs, launcher scripts pre-create `.pids/`
  and `logs/`, and the DMG Readme now includes the required BSD attribution.
- **CI & types** – Added `src/codescribe/py.typed`, made Ruff/mypy part of the
  macOS workflow with concurrency guards, and dropped the outdated
  `docs/legacy` bundle ahead of the public release.
- **Menu robustness** – Submenus are now built before attaching to the tray,
  auto-healed if rumps strips them, and the Quit dialog activates the app so
  the alert always appears on top.

<!--
Historical notes below predate the Keep a Changelog-style format used above.
-->

## Phase I – `develop` vs `main`

**Platform & Backends**
- Introduced `CodeScribeServer` as a single-instance backend runner with lazy
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

[Unreleased]: https://github.com/VetCoders/CodeScribe/compare/v0.7.0...HEAD
[v0.7.0]: https://github.com/VetCoders/CodeScribe/compare/v0.6.3...v0.7.0
[v0.6.3]: https://github.com/VetCoders/CodeScribe/compare/v0.6.2...v0.6.3
[v0.6.2]: https://github.com/VetCoders/CodeScribe/compare/v0.6.1...v0.6.2
[v0.6.1]: https://github.com/VetCoders/CodeScribe/compare/v0.6.0...v0.6.1
[v0.6.0]: https://github.com/VetCoders/CodeScribe/compare/19e05ad...v0.6.0
