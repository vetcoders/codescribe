---
track: app
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

# Track 3: App Truth Surface

## Cascade

This track is downstream of:

1. [Master Report](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:1)
2. [Master Plan](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:1)
3. This track plan

Primary master-plan anchors:

- [Faza 2. Rozdzielić Draft od Werdyktu](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:47)
- [Faza 3. Uczynić Provenance Częścią Artefaktu](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:66)
- [Faza 4. Zmienić App z \"Wybieracza Tekstu\" w \"Sędziego Prawdy\"](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:89)
- [Faza 5. Ujawnić Prawdę VAD i Braku Mowy](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:109)
- [Faza 6. Ucywilizować Fallbacki](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:130)
- [Faza 8. Rozdzielić Kategorię \"Transcription\" od Kategorii \"Interpretation\"](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:176)
- [Faza 10. Przepisać Obietnicę Produktu na Język UI i Onboardingu](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:220)

Primary report anchors:

- [Current State / 1. The app does not have one transcript truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:34)
- [Current State / 2. The product already knows when audio is mostly silence, but hides that truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:58)
- [Current State / 3. Cloud failure currently degrades to streaming truth without making the downgrade legible](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:93)
- [Current State / 5. AI is powerful, but the product mixes categories](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:154)
- [Is The Bottleneck In `app` Or `core`?](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:258)

## Track Identity

Expected deliverable:

- `app` that does not lie
- `app` that uses the engine and the daemon as well as possible
- `app` that only uses cloud fallback when the engine signals failure or insufficiency in a structured way

This track is where product promise either becomes true or gets betrayed.

## Branch Truth Rule

All work in this track must follow the live path and runtime truth of `develop`.

Hard rules:

- No shadow controllers.
- No speculative UX shell living beside the current app.
- No experimental transcript-routing lattice.
- If a radical move is needed, it must simplify the real `develop` controller/UI path and reduce deception.

## Mandatory Entry

HARD REQUIREMENT:

- Every agent entering this track must start with `vc-init`, exactly as required by `vc-marbles`.
- No edits before:
  - `loctree` mapping of `app/controller`, `app/presentation`, relevant `app/ui` surfaces, and any touched CLI seam
  - `ai-contexters` / `aicx` recovery of current project intent
  - direct review of runtime logs and current truth-routing code

## Why-Matrix Shape

Recommended `vc-why-matrix` posture:

- `Codex` lead:
  - exact implementation of adjudication, source labeling, fallback policy, and UI truth states

- `Claude` sidecar:
  - forensic review of controller routing, failure edges, and misleading product states
  - log-backed validation that the app matches runtime truth

- `Gemini` challenge lane:
  - strip rotten mode overlaps
  - challenge deceptive UX categories
  - allow fearless simplification if the controller mode graph is the lie

## Goal

- Turn the app from a hidden text chooser into an explicit judge of transcript truth.
- Ensure the app never silently upgrades a draft into a verdict.
- Ensure the app never silently downgrades a failure into a fake success.

## Scope

In scope:

- `app/controller/*`
- `app/presentation/*`
- relevant `app/ui/*` status, drawer, settings, and onboarding surfaces
- any required seam in `bin/codescribe.rs` if fallback/state policy crosses there
- archive and save/paste truth surfaces

Out of scope:

- engine internals inside `core/stt`, `core/vad`, `core/pipeline`
- daemon internals of quality loop and report generation

## Truth Targets

This track exists to remove these lies:

- draft and verdict are treated as the same thing
- fallback and success are treated as the same thing
- transcript and interpretation are treated as the same thing
- no-speech and low-confidence conditions are hidden from the user
- paste/save actions happen without enough truth to justify them

## Action Plan

1. Separate `draft` from `verdict`.
   - Streaming/live text becomes preview only.
   - Final transcript becomes an explicit adjudicated state.

2. Make provenance visible.
   - show and persist:
     - local final-pass
     - streaming fallback
     - cloud primary
     - cloud fallback
     - no-speech
     - low-confidence

3. Introduce a truth-adjudicator in the app layer.
   - one place decides:
     - what to paste
     - what to save
     - when to block
     - when to request confirmation

4. Reclassify AI outputs.
   - transcript
   - formatted transcript
   - assistant interpretation
   - formatting failed / raw preserved

5. Civilize cloud fallback.
   - cloud fallback is allowed
   - hidden cloud fallback is not allowed
   - fallback may be used only when engine failure or insufficiency is structurally signaled

6. Rewrite product language.
   - the UI must promise what the runtime really does

## Acceptance

- The app visibly distinguishes preview from final verdict.
- Provenance and confidence states are visible and persisted.
- Auto-paste is blocked or downgraded when truth is weak.
- Cloud fallback is explicit and policy-driven.
- AI categories are split so the user knows whether they are reading transcription or interpretation.

## Gates

- targeted tests for controller routing and save/paste behavior
- targeted tests for settings/status/UI state transitions
- `make lint`
- `make test-quick`
- broader `make check` if controller changes ripple across app and core seams

## Marble Mode

If executed under `vc-marbles`, each round should attack one product lie:

- "streaming preview is masquerading as final truth"
- "fallback path is silent"
- "AI output category is mislabeled"
- "no-speech truth exists but does not reach the user"

One round, one lie removed, one report.

## Parallel Boundary

This track can run in parallel with:

- `core` truth engine track
- `qube daemon` track

Contract boundary:

- `app` consumes structured engine truth from `core`
- `app` consumes learning artifacts and daemon health from `qube daemon`
- `app` does not redefine engine semantics and does not own loop internals

## Radical Move Threshold

Radical cuts are allowed if they simplify the real product path on `develop`.

Examples of allowed radical moves:

- replacing scattered transcript selection with one adjudicator
- deleting deceptive or duplicate status states
- collapsing overlapping mode branches that produce ambiguous truth

Forbidden moves:

- parallel controller experiments
- speculative second UX shell
- keeping misleading states "for compatibility" when they directly preserve product lies
