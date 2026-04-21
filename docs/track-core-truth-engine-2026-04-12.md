---
track: core
project: CodeScribe
branch_truth: develop
status: drafted
skill_frame:
  - vc-agents
  - vc-marbles
hard_requirement: vc-init
master_report: /Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md
master_plan: /Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md
---

# Track 1: Core Truth Engine

## Cascade

This track is downstream of:

1. [Master Report](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:1)
2. [Master Plan](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:1)
3. This track plan

Primary master-plan anchors:

- [Faza 1. Spisać Konstytucję Prawdy](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:23)
- [Faza 3. Uczynić Provenance Częścią Artefaktu](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:66)
- [Faza 5. Ujawnić Prawdę VAD i Braku Mowy](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:109)
- [Faza 7. Zatrzymać Korekty, Które Pogarszają](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:151)
- [Faza 9. Zbudować Truth QA zamiast tylko STT QA](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:197)

Primary report anchors:

- [Current State / 1. The app does not have one transcript truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:34)
- [Current State / 2. The product already knows when audio is mostly silence, but hides that truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:58)
- [Current State / 4. Post-processing sometimes makes a better transcript worse](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:124)
- [Is The Bottleneck In `app` Or `core`?](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:258)

## Track Identity

Expected deliverable:

- `core` as a standalone transcription engine, equipped with embedded Whisper v3 Turbo, supported by Silero VAD in the processing phase, plus optional embedded dictionary-driven final pass when requested.

This track is not about UI.
This track is not about daemon orchestration.
This track is about making the engine tell the truth in a structured, consumable way.

## Branch Truth Rule

All work in this track must follow the path and runtime truth of the current `develop` branch.

Hard rules:

- No speculative side architectures.
- No lab-only experimental path becoming the new default by accident.
- No "temporary" duplicate contracts.
- If a radical move is needed, it must be a radical simplification of the live truth on `develop`, not an experiment running beside it.

## Mandatory Entry

HARD REQUIREMENT:

- Every agent entering this track must start with `vc-init`, exactly in the spirit of `vc-marbles`.
- No editing before `vc-init`.
- `vc-init` here means, at minimum:
  - `loctree` mapping of the touched surface
  - `ai-contexters` / `aicx` context recovery for fresh project intent
  - current-tree validation of contracts and hotspots

This is not optional ceremony.
This is the anti-drift guard.

## Why-Matrix Shape

Recommended `vc-why-matrix` posture:

- `Codex` lead:
  - precision surgery
  - contract-gated implementation
  - exact refactors across `core/stt`, `core/vad`, `core/pipeline`, `core/config`

- `Claude` sidecar:
  - forensic audit of where signal, confidence, and provenance are lost
  - confirm whether metadata dies inside engine return types or at core/app seams

- `Gemini` challenge lane:
  - challenge duplicate model-resolution paths
  - force simplification if embedded/runtime-managed model stories are still parallel lies

## Goal

- Make `core` the authoritative, structured transcript engine.
- Preserve engine truth as metadata, not just text.
- Ensure optional final-pass behavior is explicit, requested, and contract-visible.

## Audit stanu kodu — 2026-04-15

### Odhaczone w kodzie

- [x] Make embedded Whisper v3 Turbo a clear runtime truth.
- [x] Keep Silero VAD as hard processing truth.
- [x] Formalize optional embedded dictionary-driven final pass.
- [x] Add truth QA fixtures at the `core` contract layer.
- [x] Normalize the live STT router around a structured transcript contract.

### Częściowo dowiezione

- Some advanced backend helpers still expose string-returning convenience wrappers, but the live router and batch quality path now stay on `RawTranscript`.

### Acceptance snapshot

- [x] `core` potrafi emitować structured transcription verdict.
- [x] Embedded Whisper v3 Turbo jest jawną domyślną prawdą runtime.
- [x] Silero VAD przeżywa API boundaries jako dane.
- [x] Final pass jest requestable i provenance-aware.
- [ ] Nie wszystkie string-only truth paths zostały jeszcze usunięte.

Live contract note:
- `TranscriptionVerdict` is expected to carry source truth plus engine provisioning provenance (`engine`, `mode`, `fallback_used`) alongside VAD, confidence flags, and final-pass metadata.

## Scope

In scope:

- `core/stt/whisper/*`
- `core/stt/*` contracts where needed
- `core/vad/*`
- `core/pipeline/contracts.rs`
- `core/pipeline/stream_postprocess.rs`
- `core/config/models.rs`
- `core/build.rs`
- minimal docs updates that explain the live contract

Out of scope:

- app UI, paste policy, drawer presentation, cloud fallback UX
- daemon extraction and archive-harvesting loop ownership

## Truth Targets

This track exists to remove these lies:

- The engine knows more than it returns.
- File-based adjudication collapses to plain text too early.
- Low-confidence and VAD truth exist only in logs.
- Final pass is treated as a toggle, not as a typed contract.
- Dictionary-driven cleanup can change text without a first-class truth boundary.

## Action Plan

1. Normalize the engine contract around a structured verdict.
   - Replace "string-only" return paths where final adjudication is expected.
   - Carry forward at least:
     - transcript text
     - source
     - VAD speech ratio / no-speech reason
     - confidence flags
     - final-pass provenance

2. Make embedded Whisper v3 Turbo a clear runtime truth.
   - Collapse ambiguity between embedded, cached, and runtime-managed model stories.
   - Preserve exactly one authoritative policy for `develop`.

3. Keep Silero VAD as hard processing truth.
   - VAD remains supervisor and prefilter.
   - Its verdict must become data, not only log output.

4. Formalize optional embedded dictionary-driven final pass.
   - It must be explicit and requestable.
   - It must not silently masquerade as base transcription.
   - It must preserve provenance so the app can expose what happened.

5. Add truth QA fixtures at the `core` contract layer.
   - Tests should assert verdict shape, not only text.

## Acceptance

- `core` can emit a structured transcription verdict instead of hiding truth in logs.
- Embedded Whisper v3 Turbo is the explicit default truth on `develop`.
- Silero VAD remains first-class in the processing chain and its outcome survives API boundaries.
- Optional dictionary-driven final pass is requestable and provenance-aware.
- No new parallel contract is introduced for the same engine truth.

## Gates

- Targeted `cargo test` for touched `core` modules
- `make lint`
- `make test-quick`
- broader `make check` if contract boundaries changed across crates

## Marble Mode

If executed under `vc-marbles`, this track should run as bounded truth-convergence rounds:

- one round = one concrete lie removed from the engine contract
- no history cosplay
- no broad rewrite without evidence
- one commit per round
- one factual delta report per round

Good round targets:

- "VAD truth exists but dies before the return type"
- "final pass exists but is not typed"
- "confidence warning exists only in logs"

## Parallel Boundary

This track can run in parallel with:

- `qube daemon` track
- `app` truth track

Contract boundary:

- `core` produces engine truth
- `qube daemon` may enrich dictionaries and loop signals
- `app` consumes verdicts but does not redefine engine semantics

## Radical Move Threshold

Radical cuts are allowed if and only if they reduce duplicate truth surfaces on `develop`.

Examples of allowed radical moves:

- deleting a second model-resolution path
- collapsing two final-pass contracts into one
- replacing string-only adjudication with a structured verdict

Examples of forbidden moves:

- introducing a shadow engine API "just to try it"
- keeping old and new contracts alive in parallel without a hard boundary
