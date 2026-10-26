---
name: control-codescribe
description:
  Drive and verify Codescribe locally with bin/control-codescribe. Use this CLI
  for every check of the repository, runtime contracts, and running build before
  shipping a PR.
---

# Control Codescribe (`bin/control-codescribe`)

Use `bin/control-codescribe` for every check of the running app and local
verification before merge. Every subcommand supports `--json` and exits non-zero
when the underlying check or service is unavailable.

## Commands

```sh
# 1. Verify local toolchains, env registry, gate ledger, and l10n catalog
bin/control-codescribe doctor --json

# 2. Verify running Codescribe.app process or live Voice Lab loopback (127.0.0.1:8765)
bin/control-codescribe dev --json

# 3. Verify surface map integrity (source file, canonical symbol, and test file)
bin/control-codescribe map --check --json

# 4. Probe live Voice Lab loopback endpoint (fails when Voice Lab is not listening)
bin/control-codescribe request GET /lab --contains "Voice Lab" --json

# 5. Execute hermetic contract suite or a specific test fixture
bin/control-codescribe execute --json
bin/control-codescribe execute --code-file scripts/tests/transcript-bus-path-test.sh --json

# 6. Verify UI surface source and run l10n catalog lint
bin/control-codescribe browse --path macos-overlay --record --json

# 7. Verify running Codescribe process (~/.codescribe/runtime/restart/<pid>.ready.json + lsof inodes)
#    or installed /Applications/Codescribe.app CSBuildCommit matches <sha> (never git HEAD)
bin/control-codescribe health --sha <expected-build-sha> --json
```

## Seed & Hermetic Environment

- Hermetic verification runs with `CODESCRIBE_DISABLE_KEYCHAIN=1` and an
  isolated `CODESCRIBE_TEST_DATA_DIR` so tests never touch the operator's real
  macOS Keychain or `~/.codescribe` history.
- Never paste secrets or `.env*` contents into fixtures, issues, or PR bodies.
- When another build is active or the operator is using the app, do not run
  `make install-app` without `make install-if-idle` / `make restart-if-idle`.

## When to Run Previews / Full Suite Gates

- **Rust core / app / bridge changes:** run `make check` and `make verify`.
- **Swift UI / copy changes:** run `make verify-l10n-catalog`,
  `make verify-l10n-bridge`, and `make test-swift`.
- **Release / appcast / website changes:** run
  `bin/control-codescribe preview --pr <n> --json` and
  `bash tests/release_appcast_contract.sh`.

## Proof in the Ship Loop

Before merging any PR:

1. Run `bin/control-codescribe doctor --json` and `bin/control-codescribe map --check --json`.
2. Run `bin/control-codescribe execute --json` (or `--code-file <test-script>`).
3. When testing a locally installed build (`make install-if-idle`), confirm the
   running process SHA with `bin/control-codescribe health --sha <build-sha> --json`.
4. Cite the exact commands and their results under `## Testing` in the PR body,
   and state explicitly what could not be verified (for example when Voice Lab
   is not running or `/Applications/Codescribe.app` was not reinstalled).

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
