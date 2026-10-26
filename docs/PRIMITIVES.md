# Codescribe Primitives Catalog

Concise index of the repository's core building blocks and code roots. Each
responsibility has a single owner (`AGENTS.md`); extend the owner rather than
adding parallel state. For deep narrative detail, see `docs/ARCHITECTURE.md`.

## Workspace Roots

| Root       | Language                   | Role                                                                                                                              |
| ---------- | -------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `core/`    | Rust (`codescribe-core`)   | Audio capture, Silero VAD, Apple + Whisper STT, `AcousticLedger`, Lexicon + Light+, LLM formatting, config + Keychain.            |
| `app/`     | Rust (`codescribe`)        | `RecordingController`, `TranscriptReducer` (`emitter.rs`), `TranscriptBus`, `DeliveryRoute`, agent channel orchestration.         |
| `bridge/`  | Rust (`codescribe-ffi`)    | UniFFI surface (`bridge/src/lib.rs`) exported to Swift; regenerated via `make app-bindings`.                                      |
| `macos/`   | Swift 6 (`Codescribe.app`) | AppKit/SwiftUI shell, `OverlayState` projection consumer, Intercom multi-agent overlay, Settings panels, `Localizable.xcstrings`. |
| `bin/`     | Rust + Shell               | CLI entrypoints (`bin/codescribe.rs`, `bin/control-codescribe`).                                                                  |
| `scripts/` | Python + Bash              | Transcript Bus demux/MCP (`scripts/bus-demux.py`), build/notarization scripts, and structural/env/l10n verification gates.        |
| `site/`    | Astro / TypeScript         | Static release and documentation site (`site/src/`).                                                                              |

## Runtime Primitives

| Primitive                 | Canonical Path                                                    | Contract                                                                                                                   |
| ------------------------- | ----------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `AcousticLedger`          | `core/pipeline/acoustic_ledger.rs`                                | Physical occurrence identity and sealing; emits `LedgerMutation` and `LedgerSeal` receipts.                                |
| `RecordingController`     | `app/controller/mod.rs`                                           | Single microphone owner across dictation, agent, and assistive modes.                                                      |
| `TranscriptReducer`       | `app/presentation/emitter.rs`                                     | Reduces committed ledger receipts into an immutable transcript projection.                                                 |
| `TranscriptBus`           | `app/presentation/transcript_bus.rs`, `scripts/bus-demux.py`      | Append-only JSONL event bus and per-channel agent delivery/ACK/reply demux (`docs/TRANSCRIPT_BUS.md`).                     |
| `DeliveryRoute`           | `app/controller/delivery_route.rs`                                | Routes sealed transcripts to the operator's explicit target (`docs/DELIVERY_ROUTE.md`).                                    |
| `RuntimeSettingsSnapshot` | `core/config/loader.rs`                                           | Immutable runtime configuration snapshot loaded from `settings.json` and `docs/ENV_REGISTRY.toml`.                         |
| `OverlayState`            | `macos/Codescribe/Screens/Overlay/OverlayState.swift`             | Swift UI projection consumer (`applyTranscriptProjection`); holds no independent document state.                           |
| `OverlayIntercomWidget`   | `macos/Codescribe/Screens/Overlay/OverlayIntercomWidget.swift`    | Multi-agent channel roster, broadcast/directed composer, and voice-reply playback chrome.                                  |
| `ReleaseRollback`         | `docs/RELEASE_ROLLBACK_RUNBOOK.md`, `scripts/generate-appcast.sh` | Sparkle `appcast.xml` withdrawal, GitHub Release draft/pre-release demotion, and forward `CFBundleVersion` patch recovery. |

## Verification Primitives

| Gate               | Command                         | Scope                                                                                                                                 |
| ------------------ | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| Control CLI        | `bin/control-codescribe doctor` | Local environment, binary/bus health, and running SHA verification (`--sha`).                                                         |
| Static gate        | `make check`                    | `cargo fmt`, `prettier`, `cargo clippy`, `semgrep`, `validate-envs.sh`, `validate-gates.sh`, `l10n-lint.py`, `l10n-bridge-census.py`. |
| Hermetic test gate | `make verify`                   | Structural verifier, bus demux unit/integration tests, workspace Rust tests, and doctests.                                            |
| Swift test gate    | `make test-swift`               | UniFFI binding check + Swift 6 strict-concurrency test suite.                                                                         |

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
