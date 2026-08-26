# Clean Transcript Bus

Codescribe publishes one private, append-only NDJSON stream. The Bus observes
session lifecycle plus occurrence-authenticated `TranscriptRevision` entries;
it does not own a transcript document and accepts no arbitrary product text.
Dictation, Agent, and Assistive share the same capture and ledger authority.
Mode changes only the downstream delivery consumer.

This Bus never opens a microphone, scrapes SwiftUI, re-transcribes saved audio,
folds raw engine events, or reconstructs text from overlay deltas.

## Path contract

Resolution order:

1. `CODESCRIBE_TRANSCRIPT_BUS_PATH`
2. `$XDG_STATE_HOME/codescribe/transcript-events.jsonl`
3. `$CODESCRIBE_DATA_DIR/transcript-events.jsonl`, with the normal default of
   `~/.codescribe/transcript-events.jsonl`

The parent directory is created when needed. On Unix the file is forced to
mode `0600`. Every accepted line is flushed before publication returns. No
host, date, room, or control-plane path is embedded in Codescribe.

## Event families

`codescribe.transcript.v1` now carries lifecycle only. `publish_started` emits
one empty `session_started` event for the controller-owned session, and
`publish_ended` emits one empty `session_ended` event when the controller
leaves that session (every path back to Idle, including zero-seal takes and
stop-timeout recovery). Neither can publish document text. A
`session_started` without a later `session_ended` for the same `session_id`
means the take is still live; `scripts/install-if-idle.sh` keys on exactly
that pair.

`session_ended` carries one typed `end_reason` (`TranscriptSessionEndReason`):
`completed` for a take that reached the serialized stop path,
`start_superseded` when a key-up or reschedule invalidated a hold start after
`session_started` and before the take became an active recording, and
`start_failed` when the recorder could not be started after the session was
announced. The controller has exactly one terminal publisher
(`end_transcript_bus`); the delayed hold start unwinds every pre-active exit
through `unwind_hold_start`, so no started session is left without its
terminal line. The first terminal line wins; later calls are no-ops.

A product recording can only begin after **acoustic admission**: the
controller resolves the input device without opening it, requires a measured
`EnergyCalibration` profile for that device from the immutable settings
snapshot (`energy-calibration.json` beside `settings.json`, see
`core/config/energy_calibration.rs`) and an armed Silero seal lane
(`CODESCRIBE_SILERO_FUSION`). A refused start writes nothing to the Bus and
opens no microphone; the refusal reaches the overlay as an
`admission_refused` warning with one actionable `admission_*` code.

`codescribe.transcript-evidence.v1` is the committed projection family. Every
line is created only by `TranscriptBus::publish_revision(revision, ledger)` and
contains:

- Bus `sequence` and `emitted_at` metadata;
- controller `session_id` and `mode`;
- reducer revision, action, document index, complete `rendered_text`, and the
  entry label;
- exact occurrence identity: session, capture epoch, sample start, sample end;
- the matching acoustic serial plus word-evidence, layer-decision, seal, and
  manual-edit receipts copied from the ledger-owned reducer entry.

The Bus skips entries the ledger cannot authenticate. It cannot admit an
occurrence, choose a label, infer identity, perform text-tail matching, or mint
a seal. A terminal `LedgerSeal` reducer action marks the writer sealed. No
arbitrary string can close committed Bus truth.

## Authority boundary

```text
OccurrenceIdentity + AcousticLedger receipts
  → TranscriptReducer.document_by_occurrence
  → TranscriptRevision
  → TranscriptBus::publish_revision
  → CsTranscriptProjectionEvent
  → OverlayState.applyTranscriptProjection
```

`EngineEvent::Preview` is ephemeral overlay paint. Raw `UtteranceFinal`,
`Correction`, `ReplaceRange`, and `InsertAnnotation` events are observation or
diagnostics. `Stats` and `SessionFinalised` are lifecycle. None can enter Bus
truth, delivery, history, clipboard, final controller text, or a terminal seal.

There is no draft API, draft storage, arbitrary-text `publish_sealed` API, or
raw-event `DeltaSinkAdapter`. Consumers must observe the authenticated evidence
projection rather than reduce engine text themselves.

## Consumer contract

An external consumer follows the resolved path with an ordinary NDJSON tailer:

```bash
tail -F "$HOME/.codescribe/transcript-events.jsonl"
```

That is the non-XDG default. With `XDG_STATE_HOME` set, follow
`$XDG_STATE_HOME/codescribe/transcript-events.jsonl`; an explicit bus-path
override wins over both. A consumer must tolerate both schemas and must not
interpret `session_started` or raw IPC events as permission to mutate product
state.

Named-agent demultiplexing remains a read-only observer over this same file; it
never opens audio or changes transcript text. State change is permitted only by
the downstream consumer of a ledger-authenticated projection, never by a draft
or preview envelope.

## C11 evidence boundary

`484095ce` was the last executable-code cut before documentation successor
`d57196ab`. C11 is the next structural executable cut; its actual commit hash is
recorded only in the durable C11 report. Compiler, tests, runtime, app, install,
and release behavior are `NOT_ASSESSED` under the C11 embargo.
