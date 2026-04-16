# ADR: CodeScribe Standalone + Vista Agent Integration

**Status:** Proposed
**Date:** 2026-02-15
**Owners:** CodeScribe + Vista teams
**Related:** `core/ipc/types.rs`, `app/ipc/server.rs`, `src-tauri/src/commands/codescribe_engine.rs` (Vista)

## Context

CodeScribe is a standalone product with proven value (STT + lexicon + quality loop + assistive formatting).
Vista also needs this capability as a built-in system agent (VistaScribe), using shared runtime behavior.

This ADR defines:

1. Responsibility split between repositories.
2. Final IPC boundaries (`audio_in`, `engine_events`, `feedback`).
3. 3-stage migration from loose integration to stable dual-product operation.

Contract identity:

- `VistaScribe = codescribe-core + qube-daemon + qube-report` (CLI binaries renamed from `codescribe-loop` / `codescribe-quality` in 0.8.1).
- `VistaScribe` branding applies only to Vista UI/agent surface; runtime modules remain CodeScribe-owned.

## Decision

### 1) Responsibility split (final)

| Area              | CodeScribe repo owns                                             | Vista repo owns                                                           |
| ----------------- | ---------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Runtime engine    | STT/VAD pipeline, lexicon, postprocess, quality loop, IPC server | Process lifecycle host (start/stop/restart/supervisor), relay to frontend |
| IPC contract      | Command and event schema source of truth                         | Typed client adapter + compatibility layer                                |
| Audio capture     | Runtime-side recorder and hot path                               | UX control over when recording starts/stops                               |
| UX                | Full standalone UX and optional debug/runtime UX                 | Product UI: tray, always-on-top overlay, workflow UX                      |
| Feedback learning | Validation + persistence + quality artifacts                     | Feedback collection UX and delivery/retry                                 |
| Packaging         | Standalone app + runtime binary + contract docs                  | Vista packaging for embedded/sidecar runtime option                       |

Boundary rule: CodeScribe remains a standalone product and runtime authority. Vista consumes it as an agent/runtime dependency, with UI-level branding only.

### 2) Final IPC boundaries

Contract authority remains in `core/ipc/types.rs` and `app/ipc/server.rs`.

#### A. `audio_in` boundary (control lane)

`audio_in` in v1 is command-based control, not raw PCM transport.

- `StartRecording { assistive: bool }`
- `StopRecording`
- `TranscribeFile { path }`
- `GetStatus` (state visibility)

Notes:

- Raw audio chunk streaming over IPC is explicitly out of scope for v1.
- Runtime owns microphone device details and recorder lifetime safety.

#### B. `engine_events` boundary (push lane)

Connection-level subscription:

- `Subscribe`
- `Unsubscribe`

Server push envelope:

- `IpcResponse::Event(IpcEvent)`
- `IpcEventPayload::StateChange { from, to }`
- `IpcEventPayload::Engine(EngineEventWire)`

Guaranteed wire properties:

- `timestamp` is RFC3339 UTC wall-clock.
- `utterance_final.segments[]` are Whisper-native timestamps.
- `raw_text` never leaves runtime over IPC.

#### C. `feedback` boundary (learning lane)

Final target command (runtime side):

- `SubmitFeedback { transcript, corrected_text?, accepted?, quality_score?, tags?, utterance_id?, metadata? }`

Delivery semantics:

- At-least-once from Vista host.
- Idempotency keyed by `utterance_id + metadata.client_feedback_id` (or equivalent).
- Runtime validates and persists feedback into quality/lexicon loop artifacts.

Compatibility note:

- If runtime lacks `SubmitFeedback`, host queues NDJSON feedback locally and retries later.

### 3) Migration plan (3 stages)

#### Stage 1: External daemon bridge (now)

Goal: Vista controls existing CodeScribe daemon as external process while CodeScribe app continues independently.

- Vista uses process manager + IPC relay (`codescribe_engine` module).
- Runtime remains distributed as standalone binary/app.
- Vista UI (tray + always-on-top) consumes runtime events.
- Feedback may fall back to local queue when `SubmitFeedback` is unavailable.

Exit criteria:

- Stable start/stop/restart + heartbeat.
- Stable `Subscribe` event relay.
- No regression in transcript quality path.

Rollback:

- Disable auto-subscribe and keep command-only mode.

#### Stage 2: Contract hardening + managed runtime options

Goal: stabilize one runtime contract for both products, with optional Vista-managed sidecar packaging.

- Keep strict runtime entrypoint (`codescribe daemon`) as supported integration mode.
- Add/complete runtime-side `SubmitFeedback` to remove host fallback dependency.
- Enforce IPC contract/version policy across both repos.
- Vista may ship managed runtime artifact, while CodeScribe standalone distribution remains unchanged.

Exit criteria:

- Contract tests pass on both repos.
- Feedback delivery acknowledged by runtime.
- Both deployment modes (external runtime or managed runtime) are supported by Vista host.

Rollback:

- Keep previous runtime artifact/protocol compatibility adapter.

#### Stage 3: Dual-product steady state

Goal: keep CodeScribe standalone strong, and keep Vista agent integration first-class.

- CodeScribe continues as independent product roadmap.
- Vista ships VistaScribe agent UX powered by CodeScribe runtime contract.
- Shared runtime improvements benefit both products without code fork.

Exit criteria:

- Feature parity targets for shared runtime path are defined and monitored.
- Independent release trains are operational (CodeScribe standalone + Vista).
- Support runbooks exist for both standalone and embedded-agent scenarios.

Rollback:

- Revert Vista to Stage-2 bridge/managed runtime mode without affecting standalone CodeScribe.

## Consequences

Positive:

- Preserves CodeScribe product identity and velocity.
- Vista gains mature speech agent capabilities quickly.
- Shared runtime contract minimizes duplicated speech logic.

Trade-offs:

- Requires strict contract governance between repos.
- Two product surfaces must remain compatibility-tested.

## Contract governance

1. Wire contract source of truth: `core/ipc/types.rs`.
2. Any breaking IPC change requires paired ADR update in both repos.
3. Additive fields/events are allowed if Vista parser remains tolerant.
4. `raw_text` remains runtime-internal and forbidden on wire.
