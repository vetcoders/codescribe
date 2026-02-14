# IPC Event Stream Contract

This document defines the CodeScribe IPC push stream used by Vista Desktop.

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
    "end_ts": 5.10,
    "segments": [
      { "text": "hello world", "start_ts": 3.24, "end_ts": 4.30 },
      { "text": "again", "start_ts": 4.30, "end_ts": 5.10 }
    ]
  }
}
```

`segments` come from native Whisper timestamp tokens (`<|0.00|>` ... `<|30.00|>`) and are available for both Candle and ONNX STT paths.

## Security and Sanitization

- `raw_text` is internal engine data and is **never** emitted in IPC event payloads.
- IPC wire mapping is centralized in `core/ipc/types.rs` (`EngineEventWire`).

## Versioning

- Contract authority: `core/ipc/types.rs`
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
