# Architecture Decision Records & Standing "No" Decisions

This directory records multi-PR architectural decisions and the standing "no"
rules that keep agents from re-proposing rejected designs. Every record states
**Decision**, **Why**, and **Revisit if**.

## Standing "No" Decisions

### NO-1: Never add a second owner, parallel reducer, or wrapper layer

- **Decision:** Each runtime responsibility has one owner (`AGENTS.md`). Swift
  (`OverlayState.swift`) is a read-only projection consumer and never owns
  transcript segments, merges previews, or re-reduces events. Competing paths
  are replaced at the owner rather than wrapped.
- **Why:** Parallel state and wrapper layers caused split-brain transcript
  projections and silent delivery drift between the overlay and the Transcript
  Bus.
- **Revisit if:** The single-process Rust core + Swift UI architecture is split
  into separate network-isolated processes with an explicit versioned wire
  protocol.

### NO-2: Never key spoken occurrences by text equality

- **Decision:** `AcousticLedger` (`core/pipeline/acoustic_ledger.rs`) keys every
  spoken occurrence by capture epoch and PCM span; text is an attached label,
  never an identity key.
- **Why:** Repeated words and incremental STT hypotheses collide under string
  equality, silently dropping or merging distinct utterances.
- **Revisit if:** Never for live audio capture; text-only synthetic fixtures
  still mint explicit occurrence IDs before entering the reducer.

### NO-3: Never separate provider credentials from their endpoint

- **Decision:** Credentials and endpoints live together as one atomic provider
  reference (`ADR 2026-08-14`). Lanes select a provider reference and model,
  never a raw URL field.
- **Why:** Splitting keys and URLs across panels caused Responses chain
  poisoning (`previous_response_not_found`) and silent 401s when keys rotated.
- **Revisit if:** A provider introduces multi-region dynamic endpoint discovery
  authenticated by a single global identity token that scopes conversation state
  independently of the endpoint host.

### NO-4: Never hand-edit generated bindings or bypass the localization catalog

- **Decision:** Regenerate `macos/Codescribe/Bridge/codescribe_ffi.swift` via
  `make app-bindings` and synchronize `Localizable.xcstrings` via `make l10n-build`
  - `make l10n-sync` + `make verify-l10n-catalog`. Never pass raw untranslated
    user-facing strings to AppKit/SwiftUI or edit operator prompts
    (`DEFAULT_ASSISTIVE_PROMPT`, `~/.codescribe/prompts/*`).
- **Why:** Hand-edited bindings drift on the next Rust build, and uncataloged
  strings fail `verify-l10n-sync` in CI and lose Polish localization.
- **Revisit if:** UniFFI or Xcode String Catalogs are replaced as the bridge or
  localization toolchain.

### NO-5: Merge and high-risk ("Always Human") surfaces stay on the operator's button

- **Decision:** Merging to trunk (`main`/`develop`), enabling auto-merge,
  modifying repository rulesets/labels/settings, and changes touching **Release,
  signing & appcast** (`release.yml`, `scripts/release*.sh`,
  `scripts/build-app.sh`, `scripts/build-dmg.sh`, `scripts/entitlements.plist`,
  `scripts/generate-appcast.sh`, `macos/project.yml`, Sparkle/CSK1 keys) or
  **Core ledger, bus & FFI contracts** (`core/pipeline/acoustic_ledger.rs`,
  `app/presentation/emitter.rs`, `app/presentation/transcript_bus.rs`,
  `bridge/`) require explicit human approval (`autoMergeFor: []`). Agents never
  run `git push --force`, `git commit --no-verify`, or copy secrets across
  machines.
- **Why:** These surfaces are one-way doors affecting notarized distribution
  trust, cross-agent bus schemas, and physical occurrence invariants.
- **Revisit if:** Automated formal equivalence proofs and hardware-isolated
  notarization sandboxes cover the full release and bus contract surface.

## Index of ADRs

| ADR                                                                                                    | Status                                      | Summary                                                                                                                             | Revisit if                                                                             |
| ------------------------------------------------------------------------------------------------------ | ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| [`2026-05-26-LAYERED_INCREMENTAL_TRANSCRIPTION.md`](./2026-05-26-LAYERED_INCREMENTAL_TRANSCRIPTION.md) | Superseded by `docs/THE_ENGINE_CONTRACT.md` | Historical 5-layer proposal replaced by the 4-layer ledger engine (Apple, Whisper, Lexicon + Light+, Responses formatter).          | A new on-device acoustic model replaces the dual Apple + Whisper observation split.    |
| [`2026-05-28-Correction-Continuous-Hands-Off.md`](./2026-05-28-Correction-Continuous-Hands-Off.md)     | Accepted                                    | Continuous hands-off correction without destructive mid-utterance rewrites.                                                         | Real-time token-level speculative decoding lands with zero cursor jump in target apps. |
| [`2026-08-14-PROVIDER_CREDENTIAL_TOPOLOGY.md`](./2026-08-14-PROVIDER_CREDENTIAL_TOPOLOGY.md)           | Accepted                                    | Keys and endpoints live together in one 3-tier provider registry; chain state is keyed by `(provider_ref, credential_fingerprint)`. | Upstream LLM APIs decouple server-side conversation chains from minting credentials.   |

## Template for New ADRs

```markdown
# ADR YYYY-MM-DD — <Short title>

Status: proposed | accepted | superseded by <ADR>

## Decision

<The choice, phrased clearly including what not to do.>

## Why

<Measured evidence or constraints that forced the decision.>

## Revisit if

<Concrete, falsifiable conditions that would change the answer.>
```

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
