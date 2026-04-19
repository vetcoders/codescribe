---
track: qube-daemon
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

# Track 2: Qube Daemon

## Cascade

This track is downstream of:

1. [Master Report](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:1)
2. [Master Plan](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:1)
3. This track plan

Primary master-plan anchors:

- [Faza 3. Uczynić Provenance Częścią Artefaktu](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:66)
- [Faza 7. Zatrzymać Korekty, Które Pogarszają](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:151)
- [Faza 9. Zbudować Truth QA zamiast tylko STT QA](/Users/polyversai/Libraxis/CodeScribe/docs/resurrection-plan-2026-04-12.md:197)

Primary report anchors:

- [Corpus Snapshot](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:14)
- [Current State / 4. Post-processing sometimes makes a better transcript worse](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:124)
- [What The App Could Give](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:219)

## Track Identity

Expected deliverable:

- `quality+loop` reborn as `qube daemon`
- same live logic family as today
- new name, clean boundary, independent daemon shape
- responsible for harvesting `~/.codescribe/transcriptions` and feeding dictionary/postprocess learning

This track is not the microphone runtime.
This track is not the UI.
This track is the truth-learning backplane.

## Branch Truth Rule

All work in this track must follow the live path and truth of `develop`.

Hard rules:

- No "future ML platform" detours.
- No parallel daemon stack beside the existing quality loop.
- Rename and extract from what exists.
- Radical moves are allowed only when they simplify the real `develop` daemon path and reduce hidden coupling.

## Mandatory Entry

HARD REQUIREMENT:

- Every agent entering this track must start with `vc-init`, exactly as required by `vc-marbles`.
- No extraction or rename before:
  - `loctree` map of `core/quality`, `bin/codescribe_loop.rs`, `bin/codescribe_quality.rs`
  - `ai-contexters` / `aicx` recovery of current project intent
  - live verification of where daemon-state coupling touches the app host

## Why-Matrix Shape

Recommended `vc-why-matrix` posture:

- `Claude` lead:
  - forensic decomposition of the current quality loop/report/daemon surface
  - identify hidden coupling to the app host and transcriptions archive assumptions

- `Codex` execution lane:
  - exact extraction of daemon boundaries
  - rename, bin restructuring, state-file contracts, and tests

- `Gemini` challenge lane:
  - strip dead or ceremonial surfaces
  - force the daemon to become one crisp product subsystem rather than "CLI plus leftovers"

## Goal

- Turn the already-existing quality loop into an explicit, independent subsystem.
- Make `qube daemon` the owner of archive harvesting, mismatch analysis, lexicon updates, and quality-loop lifecycle.
- Stop leaking daemon identity through accidental app-host coupling and old names.

## Audit stanu kodu — 2026-04-15

### Odhaczone w kodzie

- [x] Name the subsystem honestly.
- [x] Isolate daemon ownership.
- [x] Make outputs explicit.
- [x] Build truth QA around the loop.

### Częściowo dowiezione

- Reduce app-host entanglement.
  Własność subsystemu siedzi już w `core/quality/qube_daemon.rs` i `bin/qube_daemon.rs`, ale `app` nadal obserwuje i steruje daemonem przez settings/tray, więc coupling jest zredukowany, nie wyzerowany.

### Acceptance snapshot

- [x] `qube daemon` jest nazwanym subsystemem na aktualnym branchu.
- [x] Wejściem jest archiwum transkrypcji, nie pamięć runtime.
- [x] Wyjścia są trwałe i inspectable (`report.json`, `analysis.*`, history, daemon state, lexicon updates).
- [x] Quality-loop logic zachowuje tę samą rodzinę odpowiedzialności, ale z jawnym ownership.
- App coupling jest już głównie obserwacyjne, ale jeszcze nie całkiem niewidoczne w powierzchniach sterowania.

## Scope

In scope:

- `core/quality/quality_loop.rs`
- `core/quality/quality_report.rs`
- `core/quality/mod.rs`
- `bin/codescribe_loop.rs`
- `bin/codescribe_quality.rs`
- minimal supervising seams in `bin/codescribe.rs` if needed to decouple availability/state checks
- docs and naming surfaces that define the daemon publicly

Out of scope:

- engine transcript semantics inside `core/stt` and `core/vad`
- app UI truth routing and cloud fallback UX

## Truth Targets

This track exists to remove these lies:

- the learning loop is "just another CLI"
- daemon availability is an app-side incidental concern
- quality loop and quality report are conceptually separate products instead of one operating subsystem
- lexicon evolution is a side effect rather than a governed output

## Action Plan

1. Name the subsystem honestly.
   - `qube daemon` becomes the product name for this lane.
   - Old binary names can survive as aliases only if they clearly point to the new authority.

2. Isolate daemon ownership.
   - Archive ingestion from `~/.codescribe/transcriptions`
   - report generation
   - mismatch detection
   - dictionary / lexicon suggestion lifecycle
   - daemon state persistence

3. Make outputs explicit.
   - reports
   - lexicon.custom updates
   - mismatch summaries
   - daemon health/state artifacts

4. Reduce app-host entanglement.
   - the app may observe daemon availability
   - the daemon must not conceptually live inside the app

5. Build truth QA around the loop.
   - test the harvest -> report -> suggestion -> lexicon path as one system

## Acceptance

- `qube daemon` is a named, independent subsystem on `develop`.
- Its input is the transcriptions archive, not ad hoc runtime memory.
- Its outputs are durable and inspectable.
- Quality-loop logic stays the same in spirit, but ownership becomes explicit.
- App coupling is reduced to consumption/observation, not identity.

## Gates

- targeted `cargo test` for `core/quality`
- targeted tests for daemon/bin flows
- `make lint`
- `make test-quick`
- broader `make check` if daemon extraction crosses crate boundaries

## Marble Mode

If executed under `vc-marbles`, each round should attack one daemon lie:

- "loop logic exists but has no honest product identity"
- "archive harvesting is implicit"
- "daemon lifecycle is coupled to app-host assumptions"
- "lexicon update path exists but is not explicit enough"

One round, one cut, one report.

## Parallel Boundary

This track can run in parallel with:

- `core` truth engine track
- `app` truth surface track

Contract boundary:

- `qube daemon` consumes transcripts and emits learning artifacts
- `core` remains the runtime transcription engine
- `app` consumes daemon signals/artifacts but does not own loop internals

## Radical Move Threshold

Radical cuts are allowed if they remove naming drift or hidden coupling on `develop`.

Examples of allowed radical moves:

- promote `codescribe_loop` into a clearly-authoritative `qube daemon` binary surface
- collapse redundant quality surfaces
- move daemon state ownership out of accidental app control points

Forbidden moves:

- inventing a net-new daemon architecture beside the existing loop
- turning the track into a speculative ML platform rewrite
