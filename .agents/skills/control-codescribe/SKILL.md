---
name: control-codescribe
description:
  Drive and verify Codescribe locally with bin/control-codescribe. Use this CLI
  for every check of the repository, runtime contracts, and running build before
  shipping a PR.
---

# Control Codescribe (`bin/control-codescribe`)

Use `bin/control-codescribe` for every check of the running app and local
verification before merge. Every subcommand supports `--json`.

## Commands

```sh
# 1. Verify local toolchains, env registry, gate ledger, and l10n catalog
bin/control-codescribe doctor --json

# 2. Check local dev surface and installed /Applications/Codescribe.app stamp
bin/control-codescribe dev --json

# 3. Verify surface map integrity (core, reducer, bus, overlay, settings, site)
bin/control-codescribe map --check --json

# 4. Probe local health or Voice Lab endpoint
bin/control-codescribe request GET /health --contains '"status": "ok"' --json

# 5. Execute structural + env contract suite or a specific test fixture
bin/control-codescribe execute --json
bin/control-codescribe execute --code-file scripts/tests/transcript-bus-path-test.sh --json

# 6. Inspect UI/surface verification recipe
bin/control-codescribe browse --path macos-overlay --record --json

# 7. Verify checkout or installed app build commit matches expected SHA
bin/control-codescribe health --sha "$(git rev-parse --short HEAD)" --json
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

Before merging any PR (and required for `medium`-risk PRs):

1. Run `bin/control-codescribe doctor --json` and `bin/control-codescribe map --check --json`.
2. Run `bin/control-codescribe execute --json` (or `--code-file <test-script>`)
   and `bin/control-codescribe request GET /health --contains '"status": "ok"'`.
3. Cite the exact commands and their results under `## Testing` in the PR body.
4. After merge or local install, confirm the SHA with
   `bin/control-codescribe health --sha <merge-sha> --json`.

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
