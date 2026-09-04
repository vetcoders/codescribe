# Clean Transcript Bus

Codescribe publishes one private, append-only NDJSON stream. The Bus observes
session lifecycle plus occurrence-authenticated `TranscriptRevision` entries;
it does not own a transcript document and accepts no arbitrary product text.
Dictation, Agent, and Assistive share the same capture and ledger authority.
Mode changes only the downstream delivery consumer.

This Bus never opens a microphone, scrapes SwiftUI, re-transcribes audio,
folds raw engine events, or reconstructs text from overlay deltas. It can copy
the ledger's seal-coverage receipt and the engine's local final-pass comparison
receipt; neither gives the Bus mutation authority.

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
stop-timeout recovery). Neither can publish document text. The terminal row
does carry the already-resolved projection phase and action availability so a
file tailer can combine it with the last authenticated render without inventing
UI policy.

One microphone: the live app take is the most recently started app session
that has no later `session_ended` (or legacy `transcript_sealed`) for that
same `session_id`. Historical `session_started` rows without terminals are
abandoned takes — crash residue or buses written before the controller
always published an end — not a live recording.
`scripts/install-if-idle.sh` keys on that current pair, plus any unpaired
`source=cli_file_verdict` session (the CLI does not hold the install flock)
and the process-lifetime runtime lock. Never tear down the app mid-take;
never refuse install forever because an old session lacked an end line.

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
(`audio.seal_lane_armed`, default `true`). The optional power-user
`CODESCRIBE_SILERO_FUSION` env value overrides that field when present in
either direction. A refused start writes nothing to the Bus and opens no
microphone; the refusal reaches the overlay as an
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
- optional additive `seal_coverage` evidence: measured speech/covered sample
  counts, uncovered PCM ranges, ratio, threshold, and `complete|incomplete`;
- optional additive `comparison`: SHA-256, character count, and rendered text
  for the pre-repair Apple-lane document and the whole-session local Whisper
  pass. These fields remain inside `codescribe.transcript-evidence.v1`; older
  readers may ignore them and Rust decoding defaults them to absent.
- the complete canvas contract: `phase`, `can_paste`, `can_insert`,
  `can_copy`, `can_retranscribe`, `can_format`, and `terminal`.

The projection contract is one snapshot, not a bag of Swift inputs:

| Field | Source of truth |
|---|---|
| `reducer_revision`, `rendered_text` | Exact committed reducer revision |
| `phase` | `listening` for open book revisions, `finalizing` after a terminal ledger seal, then `formatted` or `no_speech` from `session_ended` plus the last committed render; failed/superseded starts are `error` |
| `can_paste` | The delivery throne selects `ClipboardPaste`, a latched target exists, and the take has ended |
| `can_insert` | The delivery throne selects `ClipboardPaste` or `DeferredInsert`, and the take has ended |
| `can_copy` | The committed render is non-empty |
| `can_retranscribe` | The session WAV exists and the take has ended |
| `can_format` | The take has ended and the committed render is non-empty |
| `terminal` | The controller's unique `session_ended` transition was processed |

`resolve_delivery_route(OverlayInsert, ...)` remains the only destination
decision. The projection layer queries its result; it does not create another
paste policy. The controller snapshots the target before overlay focus can
replace it, and checks the session-owned WAV only after the stop path has had a
chance to retain it.

The Bus skips entries the ledger cannot authenticate. It cannot admit an
occurrence, choose a label, infer identity, perform text-tail matching, or mint
a seal. `seal_coverage` is emitted before terminal finality. A terminal
`LedgerSeal` reducer action marks the writer sealed only when the latest
coverage is not incomplete. No arbitrary string can close committed Bus truth.

Controller-authenticated context captures enter the same presentation reducer
as `RecordContextMarker` actions (`record_context_marker` on the Bus). The
reducer anchors each marker at the captured character position and renders it
into every later complete `rendered_text` revision, preserving capture order at
equal positions. Swift receives that finished projection verbatim; it does not
insert, pad, retain, or replay marker text.

## Authority boundary

```text
OccurrenceIdentity + AcousticLedger receipts
  → TranscriptReducer.document_by_occurrence
  + controller-authenticated context markers
  → TranscriptRevision
  → TranscriptBus::publish_revision
  → CsTranscriptProjectionEvent
  → OverlayState.applyTranscriptProjection

session_ended + last committed Bus render + delivery/audio snapshot
  → the same CsTranscriptProjectionEvent (`terminal=true`)
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

Each Bus `session_id` owns one wav at `~/.codescribe/sessions/<session_id>.wav`
(or `$CODESCRIBE_DATA_DIR/sessions/<session_id>.wav`). The controller copies
the take there at stop. `bus-demux` assigns that path onto every envelope for
the session. It must not read or emit `last_session.wav`: that file is only the
latest-take alias for overlay Retranscribe and `codescribe transcribe last`.
Hold and toggle/double-tap both land in the same daily bag
`~/.codescribe/transcriptions/YYYY-MM-DD/` (paired m4a/wav + txt). The agent
follower is a session observer of that bag, not a second archive.

`codescribe transcribe live` uses the shared Rust projection reader and writes
`codescribe.transcript-projection.v1` JSONL to stdout. Every output row is an
exact full `rendered_text` snapshot with `kind=live_revision|terminal_seal` and
the source session, sequence, reducer revision/action, occurrence coordinates,
document index, phase, five action bits, and terminal flag. Consumers replace
their displayed snapshot when metadata is newer; stdout never pretends that a
textual suffix can encode a replacement. A text-free `session_ended` row emits
one terminal projection by combining its control fields with the last committed
render for that session; old rows without the additive fields retain the same
deterministic formatted/no-speech fallback. On macOS the follower wakes from
kqueue vnode events; a bounded timeout exists only to recover from a missed
rotation/replacement watch.

## C11 evidence boundary

`484095ce` was the last executable-code cut before documentation successor
`d57196ab`. C11 is the next structural executable cut; its actual commit hash is
recorded only in the durable C11 report. Compiler, tests, runtime, app, install,
and release behavior are `NOT_ASSESSED` under the C11 embargo.
