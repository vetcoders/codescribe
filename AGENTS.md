# Codescribe — Agent Guide

Codescribe is a macOS dictation app. A Rust core (`core/`, `app/`) does audio
capture, speech recognition and transcript assembly; a Swift app (`macos/`)
draws the UI and talks to the core through UniFFI bindings (`bridge/`).
This file tells a coding agent where authority lives and how to verify a change.

## One owner per responsibility

Each runtime responsibility has exactly one owner. Change the owner; do not add
a parallel path, a wrapper or a fallback next to it. When a second owner is
found, remove it rather than wrapping it.

| Responsibility | Owner |
| --- | --- |
| Identity of every spoken occurrence | `core/pipeline/acoustic_ledger.rs`. Text is a label attached to an occurrence, never its key. |
| Sealing an utterance | PCM energy plus Silero VAD valleys on the same capture epoch. String equality never creates, merges or deletes an occurrence. |
| Microphone | `RecordingController` (`app/controller/mod.rs`). Dictation, agent and assistive modes route downstream; none opens its own recorder. |
| Transcript document | `TranscriptReducer` in `app/presentation/emitter.rs`. `OverlayState.swift` is a projection and keeps no document state of its own. |
| Transcript bus | `app/presentation/transcript_bus.rs` only observes committed reducer events and never reinterprets the document. |
| Delivery routing | `app/controller/delivery_route.rs`. Delivery follows the user's explicit target, never OS focus alone. |
| Runtime configuration | `settings.json` → `core/config/loader.rs` → one immutable runtime snapshot. |

Terminal events release the microphone, the UI phase and the agent thread.

Avoid introducing `shim`, `compat`, `legacy`, `adapter-for-old`,
`fallback-to-previous` or `TODO remove` layers in code, diffs and commits.

## Contracts

- `docs/STT_CONTRACT.md` — recognition engines and adjudication.
- `docs/TRANSCRIPT_BUS.md` — bus events, privacy and paths.
- `docs/HOTKEYS_CONTRACT.md` — gestures, ownership and modes.
- `docs/DELIVERY_ROUTE.md` — destination selection.
- `docs/ENV_REGISTRY.toml` — every environment variable the app reads.
- `docs/LOCALIZATION.md` — String Catalogs and UI copy rules.

After changing the Rust bridge API, regenerate the Swift bindings with
`make app-bindings`. Do not edit `macos/Codescribe/Bridge/codescribe_ffi.swift`
by hand.

## Build and verify

| Command | What it does |
| --- | --- |
| `make check` | Formatting, Clippy, Semgrep, the env registry and the ledger gate. |
| `make verify` | Hermetic Rust tests and doctests (the CI contract). |
| `make test-swift` | Regenerates bindings and runs the Swift test suite (Swift 6 strict concurrency, warnings as errors). |
| `make install-app` | Builds and replaces `/Applications/Codescribe.app` without touching a running instance. |
| `make restart-if-idle` | Restarts the app only when no take is active or closing; exit code 75 means it refused. See `docs/RESTART_IF_IDLE.md`. |
| `make release-standard` | Notarized distribution DMG. A local `install-app` build is not a release. |

Worktrees share one Cargo `target` directory in the main checkout: `make` sets
`CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` from the common Git directory.
When calling Cargo directly, point both at that same absolute path.

Swift targets build without warning suppression; suppressors are not allowed.

## Commits

Every completed cut must have its own authored commit before handoff or
installation. Do not leave finished changes uncommitted for the user to commit.
Stage only the cut's files or hunks; preserve unrelated work. Source-only workers
return a committed checkpoint using `docs/COMPILE_EMBARGO.md` without running
compiler or test hooks; the designated integrator owns verification and admission.

Commit messages follow `.commitmsg`: a scoped subject, a body that explains
what changed and why, and an `Authored-By:` trailer.
