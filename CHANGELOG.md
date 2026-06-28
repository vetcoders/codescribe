# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Public release hygiene** — release packaging, repository metadata, and public-facing docs are being aligned for a current `v0.12.x` public release.
- **Dual DMG release variants** — release automation now builds a standard notarized DMG with embedded Silero + embedder and runtime Whisper cache/download, plus a `_full` notarized DMG with Whisper embedded.
- **Memory footprint** — idle RAM cut from ~5 GB (peak ~10 GB) to ~0.8 GB. The Whisper and MiniLM embedder models now unload from GPU/host memory after a period of inactivity and reload transparently on next use (`CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS`, `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS`, default 300s, 0 disables).

### Fixed

- **Silero VAD reload leak** — the Silero ONNX session is now compiled once and shared process-wide instead of being rebuilt per recording (which leaked native ORT memory over long sessions).
- **Allocator retention** — freed transient buffers are returned to the OS after each recording (`malloc_zone_pressure_relief` on macOS) instead of inflating the resident footprint across a session.

## [0.12.2] - 2026-06-22

> Public-readiness patch line for the assistive/dictation stack. This release keeps the `0.12.x` product shape but hardens the user-visible paths that made private builds feel finished while public releases lagged behind.

### Added

- **Tray startup readiness** — the tray now surfaces startup readiness instead of silently appearing idle while core runtime checks are still settling.
- **Pending follow-up preservation** — voice follow-ups survive finalization instead of being dropped as the recording state clears.

### Changed

- **Voice chat drawer I/O** — card disk operations moved off the main thread to reduce AppKit stalls in the assistant drawer.
- **Onboarding focus behavior** — onboarding stays visible without relying on always-on-top window behavior, and it refreshes when permission state drifts.

### Fixed

- **Assistive message duplication** — the first assistant message renders once instead of double-sending or double-rendering.
- **Raw recording final-pass truth** — raw stops require the correct final-pass behavior instead of silently mixing paths.
- **Dictation lexicon** — preserves Loctree/Vibecrafted vocabulary during dictation cleanup.
- **Settings shortcut copy** — removed fake shortcut customization affordances that did not map to runtime behavior.

## [0.12.1] - 2026-06-13

> Patch release for the editable overlay and assistive transcript handoff.

### Added

- **Editable dictation overlay output** — overlay results can be edited before downstream actions.
- **Audio archive as m4a blobs** — recordings can be retained in a smaller archive format.
- **Native `transcribe_audio` agent tool** — assistant tooling can transcribe an audio file through the same core STT path.

### Fixed

- **Toggle-to-voice-chat handoff** — finalized utterances append into the voice chat session instead of losing most of the spoken session.
- **Overlay button routing** — each overlay action maps to its own command path.
- **Drawer/settings layout clipping** — drawer rows and settings sections were tightened to avoid clipped content.

## [0.12.0] - 2026-06-12

> Minor release for public-source licensing, MCP bridge work, and the modern assistive UI surface.

### Added

- **Stdio MCP tool bridge** — Codescribe can load configured MCP tools and report MCP status honestly.
- **Thread search agent tool** — assistant tooling can search saved thread history.
- **Creator taxonomy shell and preview timing presets** — settings gained richer controls for creator workflows and live-preview cadence.

### Changed

- **License** — relicensed the public codescribe release surface from Apache-2.0 to FSL-1.1-ALv2 to support public availability while protecting against commercial repackaging; each version converts to Apache-2.0 after 2 years.
- **Voice chat UI** — modernized drawer rows, preserved raw markdown bubbles by default, and reduced streaming render cost.
- **UI module shape** — decomposed large settings, voice chat, onboarding, overlay, pipeline, hotkey, and shared-helper surfaces into responsibility modules.

### Fixed

- **Screenshot/asset safety** — agent screenshots are stored as bounded image assets instead of oversized inline payloads.
- **Overlay editability** — format results remain editable and pasteable through the overlay action contract.

## [0.11.2] - 2026-05-28

> Stabilization line for the hands-off transcript path and assistive runtime.

### Added

- **Thermal STT governor** — local transcription can back off under thermal pressure.
- **Build hash telemetry** — About/version surfaces include a short build hash for support and release diagnosis.

### Fixed

- **Hands-off continuous session** — toggle dictation is one continuous session: append utterances, retain audio, and send one assistant message.
- **Toggle-stop watchdog** — added protection against stuck toggle-stop states.
- **Chat overlay input stability** — restored interactive overlay behavior after floating-window focus regressions.
- **Agent stream and SSE robustness** — improved event parsing, retry behavior, and chain reset diagnostics.

## [0.10.0] - 2026-05-06

> Minor release. Embedded VAD contract hardened (zero-IO production path), legacy path-based VAD API hidden, several deprecated transcription/quality surfaces removed. Includes onboarding TOCTOU fix and AppKit overlay teardown contract completion.

### Breaking changes

- **Removed deprecated transcription helpers** — `transcribe_long`, `transcribe_long_with_language`, and the `transcribe_file(&Path, Option<&str>) -> Result<String>` shape are gone. Callers must migrate to the typed `TranscriptionVerdict` surface.
- **Removed `pub const DEFAULT_MODEL` from `core/stt/whisper/singleton.rs`** — re-exported from `core::config::models` instead. Update imports accordingly.
- **Removed `QualityLoopConfig`, `QualityDaemonState`, and `mark_daemon_unavailable`** from the quality public surface. Replaced by the `qube_lifecycle` subsystem.
- **Renamed quality daemon state type** — `read_daemon_state` and `write_daemon_state` now return `QubeDaemonState` instead of `QualityDaemonState`.
- **Hidden legacy path-based VAD loaders** — `SileroVad::new(&Path, ...)` and `AccumulatingVad::with_config(&Path, ...)` are now `#[doc(hidden)]`. Embedded path is canonical via `AccumulatingVad::new(sample_rate)`. The path-based shape is retained only for dev/test overrides.

### Added

- **Embedded Silero VAD as production default** — `RecorderVad` now goes through `AccumulatingVad::new(sample_rate)` (embedded blob via `commit_from_memory`), eliminating the disk-path fallthrough that disabled auto-silence on fresh machines. Regression-locked by new unit test `embedded_vad_loads_without_disk_file`.
- **`TranscriptionVerdict` typed truth surface** — replaces ad-hoc `Result<String>` shape across the transcription boundary; carries confidence flags and adjudication state explicitly.
- **`qube_lifecycle` subsystem** — supersedes the removed `QualityLoopConfig`/`QualityDaemonState` surface with a coherent state machine for daemon lifecycle (start/stop/health probes).

### Fixed

- **TOCTOU lock in onboarding** — replaced check-then-create file lock with `flock(2)` to prevent racing first-run setups across simultaneously launched codescribe instances.
- **NSGlassEffectView retain balance** — UI overlay now autoreleases the glass effect view to balance its explicit retain on construction; prevents a steady leak of glass overlays under heavy use on macOS 26+.
- **ObjC release contract on overlay teardown** — completed the `release` pairing for all overlay subviews so teardown does not leak under ARC-incompatible call paths.

## [v0.9.2] – 2026-04-18

> Patch release. Big-ticket items (typed transcription flags, toggle final-pass adjudication, short-text formatting truth guard) hardened from `0.9.1`. L2 config loader rewrite landed in `0.9.2`; the follow-up parity work in `0.9.3` certifies the already-green loader tests and corrects the shipped changelog narrative.

### Added

- **Typed transcription flags + toggle adjudication** ([091dd67](https://github.com/vetcoders/codescribe/commit/091dd67)) — `TranscriptionConfidenceFlag` enum extended; `Vec<String>` confidence flags converted to typed `Vec<TranscriptionConfidenceFlag>` across `RecordingTruthVerdict` boundary. Toggle mode now adjudicates session truth via the same final-pass pipeline as hold mode (no more 80% speech loss in long toggle sessions). Closes Marbles_truth_plan **L9** + research **Q10/LIE A/Q7**.
- **`final-pass` env toggle for runtime experimentation** ([42a09e7](https://github.com/vetcoders/codescribe/commit/42a09e7)) — `CODESCRIBE_LOCAL_STT_FINAL_PASS=0|1` (default `1`) lets ops disable the saved-WAV adjudicator without rebuild. `Vec::contains` cleanup on flag iteration.
- **Centralized env handling + embedded-Whisper documentation** ([fb30db2](https://github.com/vetcoders/codescribe/commit/fb30db2)) — env-var loading consolidated in one path; README + `.env.example` updated to declare embedded-first Whisper as canonical and `CODESCRIBE_NO_EMBED=1` as opt-out.

### Changed

- **Config loader rewrite** ([0a9bd99](https://github.com/vetcoders/codescribe/commit/0a9bd99)) — `core/config/{loader,migrate,mod}.rs` substantively refactored to enforce priority `settings.json > promoted env > defaults`. Lays infrastructure for upcoming Settings Creator. **Test parity** (verified `0.9.3`): both `test_load_prefers_settings_json_over_promoted_env_file_values` and `test_runtime_env_does_not_persist_into_settings_during_migration` pass on this commit. The L1 marble that flagged them was already converged by `0a9bd99` (inject_file_env_for_runtime skips promoted keys) and `43d67d1` (migrate_if_needed early-returns when `.env` snapshot is absent or empty); the CHANGELOG-as-shipped lagged the actual fix state. Functional impact: none.
- **Sort + collapsible match hygiene** (clippy) — `sort_by(|a,b| b.x.cmp(&a.x))` → `sort_by_key(|b| std::cmp::Reverse(b.x))` across `core/agent/thread_index.rs`, `core/quality/qube_daemon.rs`, `app/ui/shared/helpers.rs`, `app/ui/voice_chat/api.rs`. Collapsible `match` → guard pattern in `core/agent/thread_index.rs`, `app/controller/helpers.rs`, `app/ui/voice_chat/api.rs`. Zero behavior change, idiomatic Rust 2024.

### Fixed

- **Short-text formatting truth guard** ([ab9a9c6](https://github.com/vetcoders/codescribe/commit/ab9a9c6) — L1 marble) — non-assistive AI formatting now hard-skips only inputs `<10` chars; `AiNoop` detection narrowed to whitespace-only echoes. Punctuation and capitalization changes are preserved as legitimate formatting work. Short `FormattedTranscript` outputs in the 10–23 char band re-entered the controller quality gate (previously bypassed). Closes regression in `e2e_prompts_and_history`.

### Internal

- **Marbles convergence loops** — L1 codex marble closed `0.9.2` short-text quality gate gap. Config loader parity is now certified green; `0.9.3` closes the documentation lag and adds defense-in-depth regression coverage.
- **Build pipeline parity** — `release-codescribe` (embedded models) + `release-qube` (`CODESCRIBE_NO_EMBED=1`, isolated `target-noembed/`) split preserved from `0.9.1`. DMG slim ~1.3 GB (vs `0.9.0` legacy ~3.7 GB).

## [v0.9.1] – 2026-04-16

> Patch release. **Critical Silero VAD fix for fresh-machine deployments** + DMG size optimization via build-pipeline split.

### Fixed

- **Silero VAD embedded path** ([8b0e278](https://github.com/vetcoders/codescribe/commit/8b0e278)) — Silero ONNX model was embedded in the binary via `include_bytes!`, but runtime called `Session::commit_from_file(path)` against `~/.codescribe/models/silero_vad.onnx` which doesn't exist on fresh machines. Result: every recording on freshly-installed `0.9.0` DMG returned `vad_no_speech_detected`, regardless of audio content. Fix: new `SileroVad::new_embedded(config)` and `AccumulatingVad::with_config_embedded` use `Session::builder().commit_from_memory(embedded::MODEL)` (ort 2.0.0-rc.11 API). `core/audio/chunker.rs::init_silero_vad` rewired to embedded path; legacy `SileroVad::new(model_path, ...)` kept as dev/test override only. Verified empirically against a real-device `Sesja 1` recording (53-char Polish transcript with 57% speech detected vs prior 0% speech under `0.9.0`).

### Changed

- **Slim DMG via build-pipeline split** — `Makefile` target `release` split into `release-codescribe` (embedded Whisper + MiniLM + Silero) and `release-qube` (`CODESCRIBE_NO_EMBED=1`, isolated `target-noembed/` directory). `qube-daemon` and `qube-report` binaries shrank from ~1.3 GB each (each had its own `include_bytes!()` baked-in models — Cargo doesn't deduplicate `__DATA` segments across workspace binaries) to **24 MB each**, resolving runtime models from HF cache instead. Bundle dropped from **4.0 GB → 1.4 GB**, signed+notarized DMG from **3.7 GB → 1.2 GB** (~67% reduction). `qube-*` binaries continue to function as vetcoders-internal CLI tools without per-binary model embedding overhead.
- **`.gitignore`** — added `target-noembed/` (build-pipeline-split workspace artifact directory).

### Internal

- **Notarytool credentials profile** documented — `xcrun notarytool store-credentials VSNotary --apple-id ... --team-id MW223P3NPX --password ...` is the required one-time setup for signed DMG release pipeline.

## [v0.9.0] – 2026-04-16 (PR #26 — `feat/the-intents-engine`)

> Version bumped from `0.8.1` → `0.9.0` to truthfully signal the breaking changes below (SemVer pre-1.0 minor bump). Release tag remains on this PR.

### Breaking

- **CLI binaries renamed** – `codescribe-quality` → `qube-report`, `codescribe-loop` → `qube-daemon`. External launchd plists, cron entries, and shell scripts must be updated. Install targets (`make install`, `make bundle`) now ship the renamed binaries.
- **Public API removals in `codescribe-core`** – `stt::whisper::singleton::transcribe_file(path, language) -> Result<String>` was removed outright. `pub const DEFAULT_MODEL` is preserved as a re-export from `config::models`. Callers migrate to `stt::whisper::singleton::transcribe_file_verdict(path, language, FileTranscriptionOptions)` returning `TranscriptionVerdict`.
- **Quality daemon state type** – `QualityDaemonState` renamed to `QubeDaemonState` across the public surface.

### Added

- **Truth-surface adjudication** – New `RecordingTruthVerdict`, `RecordingTranscriptSource`, `RecordingFallbackClass`, `FinalPassVerdict`, `VadVerdict` structs replace silent degradation with explicit verdicts. Controller and overlay now render truth flags (`truth_review_trigger`, `truth_display_status`, `push_truth_flag`).
- **File transcription verdict-first** – `transcribe_file_verdict` exposes provenance (embedded vs. runtime, VAD sparkline preservation, final-pass artifact rejection).
- **Assistive preview mode + context cache** – Double-tap Right Option now engages assistive mode with a preview window and LLM context chaining.
- **Veterinary seed + lexicon variants** – Expanded Polish veterinary corpus assets in `core/assets/`.
- **Qube protocol CLI alignment** – `qube-report` / `qube-daemon` binaries and `QUBE_DAEMON_AUTOSTART` settings flag.

### Changed

- **Runtime model resolution hardened** – `resolve_runtime_whisper_model_path` clarifies precedence (`CODESCRIBE_MODEL_PATH` → bundled Resources → `../../models` → `~/.codescribe/models` → HF cache) and `canonicalize_or_self` now logs a warning on canonicalization failure instead of silently swallowing the error.
- **Embedded-first Whisper remains canonical** – Release builds embed the Whisper payload by default; runtime resolution is the opt-in fallback (`CODESCRIBE_NO_EMBED=1` or missing model). README updated to reflect this truth.
- **Settings JSON migrations** – `qube_daemon_autostart` promoted to the v2 `system` section; legacy settings continue to load via alias.
- **Overlay live-preview stability** – New `CODESCRIBE_OVERLAY_STABLE_PREVIEW` env flag gates stable-word-boundary trimming in live mode (default off).

### Fixed

- **Overlay unit tests isolated** – `test_overlay_visible_text_live_mode_defaults_to_exact_text` / `..._decision_mode_uses_exact_text` now use `#[serial]` + a scoped `OverlayStablePreviewEnvGuard` so sibling tests cannot pollute `CODESCRIBE_OVERLAY_STABLE_PREVIEW`.
- **`rustls-webpki` bumped to 0.103.12** – Addresses RUSTSEC-2026-0098 and RUSTSEC-2026-0099 (name-constraint handling for URI names / wildcard certificates).
- **Env-mutation `unsafe` blocks in `core/config/loader.rs` / `core/config/models.rs`** now carry `// SAFETY:` justifications documenting the single-threaded init invariant per Rust 2024 norms.
- **Quality daemon autostart surface** – The settings toggle label/description now tells users truthfully that the tray app does not spawn the daemon; external `qube-daemon --daemon` is required.

### Internal

- **Tray handler** – Notification text now points users to `qube-daemon --daemon` when no quality report is available.
- **Historical ADRs annotated** – `docs/ADR/2026-01-*` and `docs/future/FEASIBILITY_ANALYSIS.md` now carry historical-snapshot disclaimers explaining path drift after the `ui/` refactor and CLI rename.

## [v0.7.14] – 2026-02-07

### Added

- **Settings window (Bootstrap)** with tiered config (settings.json) + Keychain-backed API keys.
- **Fn-first hotkeys** (Globe/Fn as default hold modifier) with Shift/Cmd modifiers for Chat/Selection.
- **Configurable double‑tap interval** and **toggle silence auto‑send** (hands‑off UX).
- **MiniLM embedder** (paraphrase‑multilingual‑MiniLM‑L12‑v2) embedded by default for lightweight semantic gating.
- **Model caching in `make install-app`** (Whisper + embedder auto‑download if missing).

### Changed

- **Default hotkeys** → Hold `Fn` + double‑tap `Option` (left=normal, right=assistive).
- **Buffered streaming default** for smoother live transcription display.
- **Token limits default to 0** (API decides) to avoid truncation.

### Fixed

- **UTF‑8 slicing panic** in streaming overlap (diacritics/emoji safe).
- **Toggle streaming append** now keeps a single bubble per session (no spam bubbles).
- **Overlay header controls** restored on top of split view.
- **Bootstrap deadlocks** removed by shortening lock scopes during UI build.

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

- **Whisper Live (streaming transcription)** – Local transcription now happens _during recording_.
  Audio from the CPAL callback is chunked and processed in the background, so on `stop()` we only
  finalize the last chunk for near-instant time-to-paste.
- **StreamingRecorder** – New streaming capture/transcription pipeline built around a non-blocking
  channel from the audio callback, plus overlap + deduplication between chunks.
- **DMG packaging improvements (embedded-only)** – Release packaging is now aligned with the
  embedded-model strategy (no bundling `Resources/models/*` that would duplicate ~900MB).

### Changed

- **Docs & pitch** – Documentation and README now highlight the core differentiator: embedded Whisper
  - live streaming transcription.

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
  three-tab interface (Voice Lab, Teacher, Settings). ([a275ae8](https://github.com/vetcoders/codescribe/commit/a275ae8),
  [7aa0754](https://github.com/vetcoders/codescribe/commit/7aa0754))
- **Pure Rust local Whisper STT (Metal GPU)** – Added local Whisper inference via
  `candle-transformers` (Metal acceleration), with long-audio chunking + language detection.
  ([268f5d0](https://github.com/vetcoders/codescribe/commit/268f5d0),
  [69ed294](https://github.com/vetcoders/codescribe/commit/69ed294))
- **Whisper decoding controls** – Added `DecodingParams` (mlx_whisper-compatible) including
  n-gram blocking and streaming callback support. ([69574fb](https://github.com/vetcoders/codescribe/commit/69574fb),
  [cc0d8aa](https://github.com/vetcoders/codescribe/commit/cc0d8aa))
- **CLI transcription + E2E pipeline tests** – Added file transcription flows and a comprehensive
  end-to-end pipeline test suite. ([d7bdb4b](https://github.com/vetcoders/codescribe/commit/d7bdb4b),
  [d46c62c](https://github.com/vetcoders/codescribe/commit/d46c62c))
- **Config convenience** – Added `--config` flag to open/create the config file. ([535270c](https://github.com/vetcoders/codescribe/commit/535270c))
- **UX updates** – Added badge modes + Dock icon behavior and tightened environment/API key
  requirements. ([7946c17](https://github.com/vetcoders/codescribe/commit/7946c17))

### Changed

- **License** – Switched the project license to Apache 2.0 and added release scripts/docs.
  ([e0e7ec1](https://github.com/vetcoders/codescribe/commit/e0e7ec1))
- **Backend architecture** – Removed the Python backend and updated the Rust CI pipeline to match.
  ([5c65481](https://github.com/vetcoders/codescribe/commit/5c65481))
- **AI formatting pipeline** – Improved configuration, workflows, and Harmony support; refined
  formatting behavior and defaults. ([e11400c](https://github.com/vetcoders/codescribe/commit/e11400c),
  [8a3157f](https://github.com/vetcoders/codescribe/commit/8a3157f),
  [d46c62c](https://github.com/vetcoders/codescribe/commit/d46c62c))
- **Tray menu + local STT integration** – Refactored tray menu plumbing while integrating the local
  Whisper engine and improving related behavior. ([16021b1](https://github.com/vetcoders/codescribe/commit/16021b1))
- **Local model packaging/loading** – Bundled a default model and updated model loading logic.
  ([13378fe](https://github.com/vetcoders/codescribe/commit/13378fe))
- **Cloud/STT provider work** – Refactored lab assets and migrated cloud provider integration.
  ([8392cb9](https://github.com/vetcoders/codescribe/commit/8392cb9))
- **Configuration consolidation** – Deduplicated configuration to a single source of truth.
  ([217a336](https://github.com/vetcoders/codescribe/commit/217a336))
- **Error handling/refactors** – Refactored Whisper engine imports and adopted `anyhow`.
  ([b9ac5d9](https://github.com/vetcoders/codescribe/commit/b9ac5d9))
- **Repository maintenance** – Restructured the repo and added conversation session tracking.
  ([07fe69f](https://github.com/vetcoders/codescribe/commit/07fe69f))
- **Developer ergonomics** – Applied `cargo fmt`-driven formatting fixes.
  ([f8e04ef](https://github.com/vetcoders/codescribe/commit/f8e04ef))

### Fixed

- **Stability** – Handled poisoned mutexes via `into_inner()` fallback to avoid cascading failures
  after panics. ([b7591ab](https://github.com/vetcoders/codescribe/commit/b7591ab))
- **Backend cleanup** – Ensured backend processes are killed on all known ports.
  ([417b002](https://github.com/vetcoders/codescribe/commit/417b002))

### Removed

- **Cleanup** – Removed unused and deprecated code to keep the build clean.
  ([68469dc](https://github.com/vetcoders/codescribe/commit/68469dc))

### Changed (Internal)

- **Foundations** – Landed the initial Rust-based architecture groundwork.
  ([5a17c3a](https://github.com/vetcoders/codescribe/commit/5a17c3a))

## v0.4.3 – 2025-11-21

- Internal updates.

## v0.4.1 – 2025-11-11

- Internal updates.

## v0.4.0 – 2025-11-11

- **License clarification** – Switched from MIT to BSD 4-Clause.
- **Configurator hardening** – `hardware_detector.py` cross-platform improvements.
- **First-run portability** – Onboarding config improvements.
- **Backend & API hardening** – Robustness improvements.
- **Tooling & packaging** – Packaging script enhancements.
- **CI & types** – Type checking and CI improvements.
- **Menu robustness** – Tray menu stability fixes.

[unreleased]: https://github.com/vetcoders/codescribe/compare/v0.12.2...HEAD
[0.12.2]: https://github.com/vetcoders/codescribe/compare/v0.12.1...v0.12.2
[0.12.1]: https://github.com/vetcoders/codescribe/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/vetcoders/codescribe/compare/v0.11.2...v0.12.0
[0.11.2]: https://github.com/vetcoders/codescribe/compare/v0.10.0...v0.11.2
[0.10.0]: https://github.com/vetcoders/codescribe/compare/v0.9.2...v0.10.0
[v0.9.2]: https://github.com/vetcoders/codescribe/compare/v0.9.1...v0.9.2
[v0.9.1]: https://github.com/vetcoders/codescribe/compare/v0.9.0...v0.9.1
[v0.9.0]: https://github.com/vetcoders/codescribe/compare/v0.8.0...v0.9.0
[v0.7.14]: https://github.com/vetcoders/codescribe/compare/v0.7.2-dev...v0.7.14
[v0.7.2-dev]: https://github.com/vetcoders/codescribe/compare/v0.7.0...v0.7.2-dev
[v0.7.0]: https://github.com/vetcoders/codescribe/compare/v0.6.3...v0.7.0
[v0.6.3]: https://github.com/vetcoders/codescribe/compare/v0.6.2...v0.6.3
[v0.6.2]: https://github.com/vetcoders/codescribe/compare/v0.6.1...v0.6.2
[v0.6.1]: https://github.com/vetcoders/codescribe/compare/v0.6.0...v0.6.1
[v0.6.0]: https://github.com/vetcoders/codescribe/compare/19e05ad...v0.6.0
