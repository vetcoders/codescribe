---
name: ship-pr
description: Shepherd a pull request to ready-for-merge or parked with npx
  kcd-skills@latest autopilot ship-pr. Use when a PR is ready to finish.
---

# Ship PR

1. Fill `.github/PULL_REQUEST_TEMPLATE.md`, including `## Intent`, `Door`,
   `Cleanup`, `## Why`, `## Testing`, and an explicit `Risk: low | medium | high`
   under `## System recap`.
   - **High risk (Always Human):** any change touching **Release, signing &
     appcast** (`release.yml`, `scripts/release*`, `scripts/build-app.sh`,
     `scripts/build-dmg.sh`, `scripts/entitlements.plist`,
     `scripts/generate-appcast.sh`, `macos/project.yml`, Sparkle/CSK1 keys) or
     **Core ledger, bus & FFI contracts** (`core/pipeline/acoustic_ledger.rs`,
     `app/presentation/emitter.rs`, `app/presentation/transcript_bus.rs`,
     `bridge/`).
   - **Medium risk:** extends an existing primitive or modifies test
     expectations alongside production code (requires AI review + rung 4
     `bin/control-codescribe` proof + explicit `## Why` and `## Testing`).
   - **Low risk:** composes existing primitives, docs, or isolated tests with
     green CI.
2. Run the deterministic ship-PR step:
   ```sh
   npx --yes kcd-skills@latest autopilot ship-pr <pr-number> --risk <low|medium|high> --json
   ```
   (or pass gathered state via `--input state.json`; see `--schema`).
3. Execute the single action returned: fix CI failures, address review threads
   (fix the code or reply with concrete evidence), resolve conflicts by reading
   both sides' intent, or wait for checks.
4. Prove the PR locally with `bin/control-codescribe`:
   ```sh
   bin/control-codescribe doctor --json
   bin/control-codescribe map --check --json
   bin/control-codescribe execute --json
   ```
   Cite those commands and their output under `## Testing`.
5. Repeat step 2 until all checks and review threads are green and the loop
   parks for the maintainer (`autoMergeFor: []` — the human maintainer owns
   merge and auto-merge enablement).
6. After merge and local installation (`make install-if-idle`), confirm the
   running process SHA:
   ```sh
   bin/control-codescribe health --sha <merge-sha> --json
   ```

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
