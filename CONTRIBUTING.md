# Contributing to CodeScribe

CodeScribe is a native macOS Rust application. This repo is not a Python service and it is not a generic cross-platform template. Contribute against the current runtime truth.

## Ground Rules

- Target platform: **Apple Silicon macOS**
- Preferred contribution path: topic branch → PR into `develop`
- Do not commit secrets, machine-specific paths, or captured user data
- Keep user-facing copy and docs aligned with the real runtime surface
- If an architecture note or README section describes a future-only surface, say so explicitly

## Local Setup

1. Install Rust (toolchain new enough for edition 2024).
2. Clone the repo.
3. Copy or create your local config when needed:
   ```bash
   cp .env.example ~/.codescribe/.env
   ```
4. Build or install:
   ```bash
   make build
   make install
   ```

Useful commands:

```bash
make build
make release
make install
make install-app
codescribe --version
codescribe --config
```

## Required Local Gates

Run these before opening a PR:

```bash
cargo fmt --all
cargo clippy -- -D warnings
cargo test
semgrep --config auto --error --quiet
```

When touching a focused subsystem, run the most relevant targeted tests too. Examples:

```bash
cargo test action_handler_registers_core_settings_selectors -- --nocapture
cargo test --test e2e_round_trip -- --nocapture
```

## CI / GitHub Actions

Current workflows in this repo:

1. `rust.yml`
2. `semgrep.yml`
3. `release.yml`
4. `pages.yml`

Do not reference deleted pipelines from older repo eras.

## What Good Changes Look Like

- Runtime behavior improved, not just code style
- Settings, docs, and install path still tell the truth after the change
- New settings are persisted through `settings.json` / Keychain / `.env` correctly
- UI changes include screenshots or a short runtime note in the PR
- Tests cover the real contract you changed

## PR Checklist

- [ ] `cargo fmt --all` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] `semgrep --config auto --error --quiet` passes
- [ ] PR description explains user impact and runtime impact
- [ ] Docs/settings/install surface were updated if behavior changed
- [ ] Screenshots or runtime notes are attached for visible UI changes

## Packaging Truth

- `make install-app` builds and installs the macOS `.app`
- release DMGs are produced by the release workflow on version tags
- source install is still the guaranteed path from inside this repo

## Getting Help

Open a draft PR or issue if the code shape is unclear. If the repo lies, fix the docs with the code instead of leaving a note for “later”.
