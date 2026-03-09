# IPC Event Stream Contract

This document defines the runtime and IPC event contract used by Vista Desktop.

## Contract Authority

- Wire schema authority: `core/ipc/types.rs` (`IpcEvent`, `IpcEventPayload`, `EngineEventWire`)
- Runtime entrypoint authority: `core/audio/streaming_recorder.rs`
- Engine event model authority: `core/pipeline/contracts.rs` (`EngineEvent`, `EventSink`)

## Runtime Contract (Single Path)

There is exactly one supported live runtime path:

1. `StreamingRecorder::set_event_sink(Some(...))`
2. `StreamingRecorder::start_event_session(...)`
3. `pipeline::streaming::transcription_session(...)`
4. `SttScheduler` serializes inference work
5. `EventSink` fanout distributes `EngineEvent` to presentation, IPC and session telemetry sinks

Controller wiring uses the same contract for hold/toggle sessions:

- presentation sink (`PresentationEmitter`)
- IPC sink (`IpcBroadcastSink`)
- telemetry sink (`SessionTelemetrySink`)

Legacy runtime paths are intentionally unsupported:

- no `set_delta_callback` API on `StreamingRecorder`
- no legacy worker-style symbols (`VadWorker`, `TranscriptionWorker`) in active runtime path
- no legacy IPC wire variants such as `engine.type = "vad_fallback"`

## Transport

- Socket: Unix domain socket at `~/.codescribe/ipc/codescribe.sock`
- Framing: NDJSON (`\n`-delimited JSON objects)
- Subscription lifecycle:

1. Client sends `IpcCommand::Subscribe`
2. Server responds with `IpcResponse::Ok`
3. Server pushes `IpcResponse::Event(...)` lines live
4. Client can send `IpcCommand::Unsubscribe` to stop push

`Subscribe` and `Unsubscribe` are connection-level controls handled by `tokio::select!` in the IPC server loop.

## Event Envelope

Every pushed event is wrapped in:

```json
{
  "Event": {
    "timestamp": "2026-02-14T19:31:52.123Z",
    "event": "state_change",
    "from": "idle",
    "to": "recording"
  }
}
```

The inner payload (`IpcEvent`) uses:

- `timestamp`: RFC3339 UTC with millisecond precision
- `event`: discriminator (`state_change` or `engine`)

## State Change Event

```json
{
  "Event": {
    "timestamp": "2026-02-14T19:31:52.123Z",
    "event": "state_change",
    "from": "idle",
    "to": "recording"
  }
}
```

Allowed state labels:

- `idle`
- `recording`
- `busy`
- `conversation`

## Engine Event

Engine events are tagged with `type`:

```json
{
  "Event": {
    "timestamp": "2026-02-14T19:31:54.040Z",
    "event": "engine",
    "type": "preview",
    "rev": 7,
    "text": "hello world"
  }
}
```

`utterance_final` includes segment timestamps:

```json
{
  "Event": {
    "timestamp": "2026-02-14T19:31:55.208Z",
    "event": "engine",
    "type": "utterance_final",
    "utterance_id": 12,
    "text": "hello world again",
    "start_ts": 3.24,
    "end_ts": 5.1,
    "segments": [
      { "text": "hello world", "start_ts": 3.24, "end_ts": 4.3 },
      { "text": "again", "start_ts": 4.3, "end_ts": 5.1 }
    ]
  }
}
```

`segments` come from native Whisper timestamp tokens (`<|0.00|>` ... `<|30.00|>`) and are available for both Candle and ONNX STT paths.

## Security and Sanitization

- `raw_text` is internal engine data and is never emitted in IPC payloads.
- IPC wire mapping is centralized in `core/ipc/types.rs` (`EngineEventWire`).

## Guardrails

Contract guard tests that block legacy regressions:

- `core/ipc/types.rs`:
  - `legacy_vad_fallback_wire_is_rejected`
  - `removed_legacy_wire_variants_are_rejected`
- `core/pipeline/tests/regressions.rs`:
  - `runtime_contract_blocks_legacy_delta_callback_api`
  - `runtime_contract_blocks_legacy_worker_symbols`
- `tests/e2e_cli_commands.rs`:
  - `test_cli_live_uses_event_sink_contract`

## Migration Notes (CLI / Tooling)

For old integrations that still depend on legacy callbacks or worker symbols:

1. Replace `set_delta_callback(...)` wiring with `set_event_sink(Some(Arc<dyn EventSink>))`.
2. Start sessions via `start_event_session(...)`.
3. If you only consume text deltas, bridge explicitly with `DeltaSinkAdapter`.
4. Consume `NoSpeech` and `Stats` from engine events (session telemetry sink), not from ad-hoc worker state.
5. Treat `vad_fallback` and other removed wire variants as hard errors.

## Versioning

- Backward-compatible changes:
  - adding new optional fields
  - adding new `engine.type` variants
- Breaking changes:
  - renaming/removing fields
  - changing existing field types
  - changing envelope shape (`IpcResponse::Event(IpcEvent)`)

Recommended policy:

1. Keep old fields for at least one Vista release cycle.
2. Gate new behavior behind tolerant parsing on the Vista side.
3. Document every wire change in this file and changelog.
