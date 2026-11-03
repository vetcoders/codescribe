## Intent

<!-- What the PR does in one or two sentences. -->

Door: two-way

<!-- two-way | one-way (release/signing/appcast, core ledger, bus schema, FFI contracts) -->

Cleanup: none

<!-- none | needed (#issue) | done -->

## Why

<!-- Why this change is needed and what problem or drift it prevents. -->

## User Impact

What does this improve or prevent for a real Codescribe user?

## Runtime Impact

- [ ] Recording, hotkeys, overlays, settings, or release packaging changed
- [ ] Public docs, onboarding, install path, or metadata changed
- [ ] No runtime behavior changed

## Testing

- [ ] `cargo fmt --all`
- [ ] `cargo clippy -- -D warnings`
- [ ] `cargo test`
- [ ] `make semgrep`
- [ ] `bin/control-codescribe doctor --json && bin/control-codescribe map --check --json`

<!-- Cite exact verification commands and results (e.g. make verify, make test-swift, bin/control-codescribe execute --json). If any test expectation changed alongside the code it tests, explain why here. State what you could not verify. -->

## System recap

Risk: low

<!-- low | medium | high (high: Release/signing/appcast or Core ledger/bus/FFI contracts) -->

Touches: docs (composes)

<!-- <primitive or area> (composes | extends | adds) -->

## Release Notes

Should this appear in `CHANGELOG.md`?

- [ ] Yes, added
- [ ] No, internal-only
