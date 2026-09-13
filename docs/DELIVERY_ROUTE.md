# Delivery route — one throne for destination

> Founder 2026-08-15: the stop path was a fight for the throne. This file is
> the destination axis only. Mic lock, transcript truth, and agent-chain
> memory stay other thrones.

## Law

1. **Intent is frozen at session start** (or at an explicit overlay click).
   OS focus at stop time is not an input.
2. **`resolve_delivery_route` is the only function that picks a destination.**
   Auto-paste, overlay Insert, and To Agent consult it. They do not invent a
   second king.
3. **The Codescribe overlay canvas is never a legal Cmd+V target.** Its caret
   parks Paste Here. A positively latched Agent composer, Alacritty/Zellij
   (vc-terminal), Notes, or another foreign caret is legal; choosing the Agent
   route remains an explicit action. Assistive delivers as a first-class Agent
   message rather than synthesizing a focus-derived paste.
4. **Clipboard is borrowed, never stolen.** On release we snapshot the user's
   pasteboard, Cmd+V into the latched caret, then restore. The overlay must
   resign key first. The foreign target must then be observed as frontmost;
   Codescribe remaining frontmost is a veto. If Cmd+V cannot land, park ⌘⌥V
   and leave the user's pasteboard alone. Explicit overlay **Copy** is the only
   verb that writes the pasteboard on purpose and leaves it.

## Intent → route

| Intent            | Typical gesture                      | Route                                                                                                                |
| ----------------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| `AgentVoice`      | Double Right Option / assistive hold | `AgentComposer`                                                                                                      |
| `OverlayToAgent`  | overlay **To Agent**                 | `AgentComposer`                                                                                                      |
| `OrientDictation` | Hold Fn / Globe                      | `ClipboardPaste` if auto-paste; `OrientCanvas` if overlay caret / no auto-paste                                      |
| `OrientFormat`    | Double Left Option                   | same as dictation                                                                                                    |
| `OverlayInsert`   | overlay Insert / defer               | `ClipboardPaste` into a latched foreign caret (Alacritty, Notes, …); `DeferredInsert` when Codescribe owns the caret |
| `NotesOnly`       | save-only notes                      | `ArchiveOnly`                                                                                                        |

Vetoes that keep Orient off the paste gun: empty / no-speech, live-stream
session, quality-commit pending, overlay canvas holds the caret.

### Stop path (restored 2026-09-08)

The W0 authority demolition (`ac6d399b3`, 2026-08-24) removed the stop-path
intents together with the old transcript cone, and with them auto-paste on
release. Restored by `delivery_intent_from_session(assistive, force_ai, notes_save_only)` → `resolve_delivery_route` on both stop paths (toggle
Finish and hold release):

- `OrientCanvas` from the table is `ArchiveOnly` with
  `reason=auto_paste_disabled`: the overlay canvas already shows the
  committed document, nothing else moves.
- `AgentVoice` resolves to `AgentComposer` with
  `reason=assistive_first_class`; the stop path never pastes it.
- `live_stream_session` and `commit_required` have no producer on the stop
  path today; both are false by construction until one returns.
- Transport is `execute_clipboard_paste`, shared with overlay Insert:
  activate the latched target, confirm focus (bounded wait **or** the target
  observed frontmost afterwards), preflight the event tap, borrow the
  clipboard for one Cmd+V. Anything else parks Paste Here. A latched target
  that confirmed neither never yields to whoever happens to be frontmost
  (`clipboard_paste_may_post`); only an Insert with no latch may follow the
  external frontmost app.
- Exactly once per take (`claim_take_delivery`): the shared Stop operation
  returns its retained result on repeat. A duplicate direct handoff is refused.
- **Seal refused (degraded delivery).** When the ledger refuses the terminal
  seal, `TerminalSealRefused` carries the committed live document and the
  controller still delivers it with `seal_refused=true` on the same
  `delivery_route:` line. History keeps its `failed` verdict, the ledger and
  the coverage threshold are untouched, and no witness is minted: the user
  gets the words the overlay already shows. The original degraded-delivery
  choice was Claude's call on 2026-09-08. The shared refusal decision and its
  remaining receiver boundaries are recorded below.

Explicit overlay clicks do **not** inherit the live-stream or quality-commit
vetoes. The user asked to insert now. Any Codescribe caret still refuses Cmd+V
and arms Paste Here instead. Alacritty and other confirmed foreign targets get
Cmd+V; Agent requires the explicit Agent route.

`paste_text_from_overlay` and `defer_text_from_overlay` consult
`resolve_delivery_route`. They do not pick a destination on their own.

The target is latched before the recording overlay takes focus and survives
the terminal transition back to Idle. Start failure and explicit recovery
clear it. This ordering matters: Insert happens after the terminal transition,
so clearing the target as ordinary recording state makes every finished take
degrade to DeferredInsert even when the foreign caret was known.

## Composer Stop settlement (W2 source checkpoint, unverified)

The admitted capture handle, produced inside the controller start lock, is the
composer's Stop authority. Owned Stop goes directly to its named endpoint;
`isRecording` is telemetry and neither authorizes Stop nor acknowledges delivery.
The store keeps the initiating thread while settlement is owed. It reuses the
existing preparing presentation for pending settlement, including after a failure
banner expires. Delayed replies address both request ID and capture ID.

`RecordingController` retains one capture-addressed task/result slot. A caller
waits at most the existing `STOP_TIMEOUT` (currently 120 seconds, an existing
engineering budget, not a Founder-selected latency target). Expiry returns
`Pending`; it does not cancel recorder drain, formatting, delivery or terminal
reset. The task holds the existing serialization lock from identity admission
through its terminal side effects. Duplicate Stop observes that same result;
completed retention is one slot, replaced only after its task exits. This is a
bounded caller outcome, not a finite settlement guarantee for an indefinitely
stalled dependency. A pending take still owes a terminal receipt or recoverable
failure and prevents another composer capture.

`CsConditionalStop` distinguishes `Stopped`, `AlreadyStopping`, `Pending`,
`ForeignCapture`, `NoLiveCapture` and `AdmissionUnavailable`. The first three
leave terminal/delivery ownership in place until the identity-aware consumer
finishes. Foreign/absent capture releases only that request; its capture-to-thread
delivery receipt survives for queued text. Admission unavailable means no task
was accepted; keep the same handle available for an explicit Stop retry. A
transport error preserves the pending destination. An expired banner is never a
terminal event. Early terminal text is retained until the start reply supplies
its authenticated handle, then joined to the initiating thread exactly once.

Named toggle captures use the existing toggle terminal processor even in
Assistive mode, so it reads the recorder's `CaptureTurnIntent` and preserves the
one-turn formatter/delivery route. No hands-free configuration is changed.

### Shared Stop convergence (rc-w2-stop-convergence, W2 source only)

Hold release, RAW toggle during hold, toggle Stop, external overlay/tray Stop,
and composer Stop now register or join the same controller settlement slot.
Hotkey Stop admission precedes mode-flag writes. Current capture selection uses
non-waiting registration/serialization/state/identity locks: contention refuses
admission rather than queuing an unaddressed gesture behind a replacement.
The registered task rechecks its named identity under `serial_lock` and retains
that lock through terminal processing. An idle Stop invalidates a not-yet-admitted
delayed hold generation. A foreign named Stop cannot invalidate that generation.

Routing now depends on the admitted take, not on its stopping surface:
SingleTurn toggle takes use the toggle terminal body, including Assistive;
HandsFree takes use the existing external-surface final-pass policy. Thus
Assistive HandsFree and explicitly disabled final-pass toggle gestures now use
the same generic processor as their external Stop. The existing one-turn
formatter, audio retention, typed processing errors and delivery claim remain.

The old hold/toggle watchdogs cancelled their processing futures, called
`recover_from_stuck_stop` and forced Idle even with a held recorder. They are
removed. The replacement contract is a bounded caller wait and an uncancelled,
tracked operation. Legacy void bridge Stop reports unresolved outcomes as errors;
it never maps Pending/AlreadyStopping/admission refusal to successful Stop.
Original May 13/14 hang evidence remains beside the phase timing logs. The old
forced-Idle serving-status test is replaced by barrier tests requiring Busy and
no terminal while held, followed by exact terminal settlement after release.
Actual completed failures retain the TranscriptionFailed lifecycle assertions.

After an addressed transport error, the store grants one explicit retry using
its original request ID and capture handle. Retry joins/retrieves the same
operation, makes no isRecording query and starts no take. This permission is
separate from recording presentation; lifecycle paint and banner expiry cannot
create it. An addressed transport-failure banner stays visible until retry or
terminal receipt: the real composer's preparing mic is disabled, whereas its
failure mic remains actionable on the owning thread. Retry success still does
not acknowledge delivery. Original thread,
draft and capture receipts remain until the addressed consumer delivers or
reconciles the capture. Late results/failures cannot mutate a replacement request.
A lost delivery event after a successful Stop reply still needs the outside-fence
projection/transport recovery owner; this cut adds no replayed document API.

Shutdown closes capture admission permanently for that controller, including
delayed hold tasks and conversation starts. The bridge also closes lazy
construction admission and retains the closed shared root until runtime teardown.
It requires a terminal Stop outcome plus quiescence checked under serialization:
no active task without a result, Idle, no session identity, inactive recorder,
and no running conversation task. Busy/Pending, lock contention, processing error
or missing receipt refuses shutdown without clearing capture ownership. A later
shutdown attempt can join the same owner. No application is stopped to verify this
source checkpoint. Runtime restart within the same process is not restored by
this shutdown contract.

**BOUNDARY — finite Stop is still open.** `StreamingRecorder::stop` awaits
`Recorder::stop`; that owner's `stop_tx.send`, stream destruction,
`SpillSink::finalize` thread join, buffer locks and WAV writes have no end-to-end
bound. `StreamingRecorder::complete_stop` awaits the transcription task, takes
transcript-buffer locks during drain, and locks the ledger. Its three-second drain
clock cannot bound lock acquisition. Controller routing/recorder/mode locks,
terminal formatter, delivery and terminal reset/publication also remain owned
potentially indefinite waits. Filesystem copies, archive publication and process
reap can block despite the inherited encoder budget. Safe finite recovery needs
the resource owner to return authenticated failure/audio receipts after it has
actually released or quarantined the resource; a timer in this controller cannot
authorize reuse or fabricate a seal. Pending alone fails full RC acceptance.

Conversation key-up still calls `stop_conversation_mode`, with its separate
Moshi loop/recorder/player cleanup. It has no capture-addressed settlement
identity and drops its timed-out JoinHandle. External Stop and shutdown now
refuse that unsupported state rather than run generic dictation cleanup on it.
This remains an explicit convergence boundary; the conversation engine/audio
owners are outside this fence. OS hotkey events queued before reaching controller
admission also carry no capture identity; this cut's successor guarantee starts
at controller admission, not at the physical key timestamp.

Tests added here exercise actual controller routes and barriers, bridge shutdown
receipt handling, and RealComposerDictation/AgentChatStore recovery with a fake
FFI transport. Hold processing still uses the inherited `cfg!(test)` short path;
this is not a microphone/archive end-to-end test. All tests are **UNRUN**.
BUILD/TEST/RUNTIME=NOT_ASSESSED. Astra must reconcile overlay recovery and generated
bindings, restore the relevant Rust and Swift gates at seam closure, and verify
installed behavior. This source checkpoint is not W2 structural closure.

### Producer failure recovery (W2 source checkpoint, tests UNRUN)

`StreamingRecorder::stop` now passes every archive outcome through the same
terminal tail: send the actual archive result to the worker, release lifecycle
notification ownership, join the transcription task, drain presentation, and
release the sink. A failed archive cannot skip that tail. A pending join retains
its handle in the recorder; this does not impose a deadline on the dependency.

`CaptureStopFailure` carries the bound session and capture epoch, the exact WAV
path **only if Recorder returned one**, and the original typed error. If both
archive and task fail, archive failure stays primary and the task failure is
retained separately. No successful seal, successful Stop, new path search, or
`last_session.wav` lookup is used to repair a processing failure.

The common hold/toggle consumer compares the evidence with both the controller
take id and recorder identity frozen before Stop. Missing or mismatched identity
refuses retention. For matching evidence it uses the existing daily archive and
session-WAV copy path; the latest alias is an output convenience, never identity.
Every destination is attempted, failures remain visible through the existing
`transcription_failed` warning, and the original WAV is not removed. Copy failure
adds context without replacing the typed processing error. The owned Stop slot
retains the error for duplicate callers; the existing terminal epilogue ends the
failed take once, without resetting a successor or claiming delivery.

#### Controller WAV filesystem boundary (W2 source checkpoint, tests UNRUN)

The controller opens the source once, refuses a symlink leaf, and checks the
opened descriptor is a regular file before invoking any archive callback.
Nonblocking open lets it reject a FIFO without waiting for a writer. Both WAV
copies read that held descriptor, including when the source pathname is replaced
between admission and copying. This pins file identity, not immutable contents:
a process already able to write that inode can still change the recording.

The configured root's existing parent is trusted configuration. Its resolution
is performed once (allowing platform aliases such as `/var`); the resolved
absolute components are then opened one at a time with `openat`, directory-only
and no-follow flags. The root leaf and `sessions` directory are created/opened
relative to held descriptors and cannot be symlinks. Parent traversal is refused;
ancestors must already exist. The source's existing parent is resolved and
opened the same way. This is no hard-coded home/temp prefix whitelist and does
not authenticate a hostile replacement of a trusted directory by another real
directory before admission.

Root and sessions handles are retained before the daily callback. Each copy
writes a fresh, exclusively created mode-0600 temporary inode in its pinned
directory, syncs the file, then atomically replaces the destination entry with
`renameat`. A destination symlink is replaced, never followed; a hardlink or the
source's own name is replaced without truncating its inode. Repeated retention
and latest-alias refresh remain supported. Renaming an admitted directory cannot
redirect writes through its replacement symlink, though the saved file may then
be reachable under the directory's new name. Logs identify the requested path,
not a promise that its current pathname still reaches that pinned directory.

Each admitted destination is attempted independently, including after daily
archive failure or refusal of the sessions directory. Copy errors leave existing
destination entries intact and attempt temporary-entry cleanup; cleanup failure
is reported. Source validation failure refuses all retention. The original typed
capture error and visible failure warning remain the consumer's result.

#### Daily archive filesystem and encoder boundary (W2 source checkpoint, tests UNRUN)

The daily callback now receives the same held source `File` used by the session
and latest-alias copies. Public pathname APIs admit a regular, non-symlink WAV
once, then delegate to this owner. No converter or fallback reopens its old
pathname. Identity is pinned; concurrent writes to the admitted inode remain
outside the guarantee.

The root supplied by `Config::config_dir()` is trusted configuration; that API
already canonicalizes an existing `CODESCRIBE_DATA_DIR` override, including its
leaf alias. This cut does not authenticate the original override pathname. The
supplied root's existing parent is resolved once, then walked with no-follow
directory descriptors. Supplied root, `transcriptions`, and day
leaves use `mkdirat`/`openat`; symlink directories are refused. Directory renames
cannot redirect subsequent writes into replacement links. Returned/logged paths
are requested locations, not authenticated claims that those pathnames still
reach the admitted directories.

One exclusively created reservation selects a common stem across `.m4a`, `.wav`
and `.txt`, including standalone text saves. Existing entries, including dangling
links and hardlinks, occupy a stem. Exhaustion fails instead of overwriting a
base name. Audio and transcript are staged in fresh mode-0600 files and published
with directory-relative, no-replace `linkat`. A late hostile destination entry
causes refusal; it is never opened or removed. Paired same-second takes retain
separate history rows; legacy text-only save families still collapse.

Committed speech produces a raw audio/text pair; no-speech produces a failed
pair with an empty marker and the fixed `(no speech)` title. Unavailable output
produces audio only and never persists its diagnostic. If text publication fails
after audio publication, audio remains and the operation reports failure. The
legacy standalone `HistoryEntry` return type still cannot represent a write
error: its path is an intended path and callers must not treat it as a receipt.

The converter receives only anonymous private staging files, via inherited
regular-file stdin/stdout and `/dev/fd/0`, `/dev/fd/1` on macOS. It cannot truncate
the admitted source or published artifacts. A 15-second archive conversion
budget is an agent implementation choice within the existing 120-second Stop
caller budget, not a Founder latency target. Polling owns the child until exit;
timeout/error/unwind kills and waits for that child. There is no detached waiter,
process-wide kill, diagnostic pipe, or unbounded stderr buffer. Stderr is discarded;
exit status and timeout remain visible diagnostics. Failed, hanging, or empty
successful conversion falls back to WAV read from the same admitted source.
Only successful nonempty encoded bytes are published as m4a. This validates
status and size, not codec decodability; real macOS decode remains mandatory.

Unix directory operations cover macOS and Linux; Linux reports unsupported AAC
conversion and retains WAV. Non-Unix daily writes refuse explicitly. The actual
macOS `afconvert` descriptor-path behavior is UNRUN; refusal safely selects WAV.

The threat model excludes arbitrary same-account mutation of owned staging or
admitted directory entries. File data is synced before publication, but directories
are not fsynced and the pair is not a crash-atomic transaction. Crash/panic or
cleanup failure can leave a reservation/staging entry; later allocation skips it.
Cleanup errors are logged. Audio already published is never rollback cleanup.
A stalled filesystem operation, process spawn or kernel child reap still has no hard deadline;
the 15-second child polling budget is not an end-to-end Stop settlement guarantee.

Still-unbounded Stop owners include serialization/recorder locks, recorder drain
and WAV finalization, transcription task join, presentation/ledger lock acquisition, final
adjudication/formatting, delivery transport, archive filesystem I/O, and terminal
reset/publication. This cut adds no cancellation authority to those owners.
Security scans establish neither finite settlement nor installed recovery.

For this new processing-failure path, committed text is **unavailable**: the
producer's shared string has no revision/occurrence authentication attached to
it. It is neither archived as speech nor delivered. The controller does not
rebuild a document from ledger internals. Existing authenticated Bus projections
remain owned by the emitter/Bus; exposing a failure-time authenticated snapshot
is a separate owner seam if required. Existing `TerminalSealRefused` degraded
text delivery and clean-stop behavior remain in their established branches.

Remaining acceptance: finite settlement of indefinitely hung dependencies,
transport loss without an eventual terminal receipt, failure-time authenticated
text handoff, and installed real Stop/recovery evidence. Producer tests inject
capture/archive ingress into the production stop tail; they do not exercise a
microphone or the private Recorder WAV writer. Consumer tests inject the daily
encoder and use real temporary-file copies. These seams are W3 falsifiers, not
executed end-to-end proof.

Generated bindings remain unchanged under W2. W3 must regenerate from
`bridge/src/recording.rs` and `bridge/src/hotkeys.rs` with `make app-bindings`,
then run the actual controller/bridge suites, Swift ownership/delivery suites,
and full `make check`, `make verify`, `make test-swift`. All newly authored tests
are UNRUN. Installed Stop latency, retained audio and exact real delivery remain
W4 obligations.

## Refusal recovery (rc-w2-refusal-recovery, W2 source checkpoint)

Both `process_recording` (hold/generic) and `stop_toggle_and_adjudicate_inner`
consume `process_terminal_stop_error` after the existing recorder terminal tail
and audio retention. A string mentioning "seal refused" is still an ordinary
failure. Only `TerminalSealRefused` with a ledger-issued finality refusal for the
current capture can reach degraded handoff. The private-field witness names the
session, capture epoch and refusal reason and retains the original optional
coverage receipt. Complete coverage is not an issued terminal seal. Nonempty
text must exactly match the already published unsealed Bus document, capture
epoch and coverage diagnostics (including matching absence).
The emitter accepts `SealCoverage` only when it matches the ledger's current
receipt. No preview, raw final, synthetic seal or changed coverage threshold
can satisfy these checks. Missing/mismatched Bus evidence fails closed; this
cut adds no replay or reconstruction path for a missing projection.

Three facts remain separate:

- Capture settlement releases resources through the existing serialized Stop.
  `Stopped` does not acknowledge a receiver or certify a ledger seal.
- Usable refused words end with `end_reason=coverage_refused` and
  `phase=coverage_refused`. The Bus clones the authenticated projection and
  retains the original optional coverage receipt; its `sealed` latch remains false.
  These existing wire phase names describe refused terminal finality, not a
  claim that the coverage measurement itself is incomplete.

### Which refusals reach this path

Coverage and terminal finality are separate facts. Normal nonempty Stop requires
an already-issued `LedgerSealReceipt` for the exact session and epoch covering
the current acoustic occurrence set. Stop only inspects this receipt; it never
calls `seal_terminal`, and inspection does not compare mutable label bytes.
Pending text recovery, non-complete coverage, or a missing/currently insufficient
terminal receipt refuse success. A complete measurement remains complete in the
refusal payload; it is never rewritten to manufacture an acoustic hole.

| Coverage verdict                   | Meaning                                                                                     | Delivery                                      |
| ---------------------------------- | ------------------------------------------------------------------------------------------- | --------------------------------------------- |
| `complete`, issued current seal    | committed words cover measured speech and ledger finality was issued                        | normal delivery                               |
| `complete`, missing current seal   | coverage is measured; terminal receipt is absent or predates current occurrences            | degraded handoff                              |
| absent receipt                     | no coverage receipt is available for this capture                                           | degraded handoff if authenticated words exist |
| `incomplete`                       | authenticated measured speech is uncovered beyond 250 ms                                    | degraded handoff                              |
| `unavailable(not_observed)`        | no acoustic observer measured this take                                                     | degraded handoff                              |
| `unavailable(identity_mismatch)`   | the measurement names another session or capture epoch                                      | degraded handoff                              |
| `unavailable(invalid_measurement)` | some PCM reaching the observer was non-finite, so nothing it measured can be trusted        | degraded handoff                              |
| `unavailable(partial_observation)` | the observer's extent stops short of the capture, or committed/measured spans reach past it | degraded handoff                              |

The distinction matters for honesty, not for routing: an unavailable verdict is
**not** a claim that words were lost. Committed occurrences and the session WAV
are preserved exactly as for `incomplete`, `TerminalSealRefused` carries the
same authenticated payload, and recovery keeps the words. What changes is that
the take is no longer allowed to _certify_ itself. Empty text alone is not proof
of silence. Zero captured samples with empty text and no ledger facts may finish
without a seal. Nonzero captured audio with empty text may finish only with an
authenticated observed-silence receipt and no acoustic occurrences or recovery
debt. This is an explicit no-speech outcome, not a manufactured terminal seal.
Missing capture authority or observed silence paired with nonempty text produces
`CaptureStopFailure`, preserving saved audio but authorizing no text handoff.

The native projection now carries typed coverage status and unavailable reason
through `CsTranscriptProjectionEvent.seal_coverage`, including an absent ratio
when measurement is unavailable. Both `statusText` and the standing refusal
notice derive from the admitted projection: incomplete speech coverage differs
from unavailable measurement, and missing legacy evidence stays unverified.
Unavailable measurement does not assert lost words. The existing recovery slot
and capability-driven controls keep the exact committed bytes.

Admission is session-local: strictly later sequence, non-regressing revision
and epoch. A lifecycle terminal with the same document revision and a later
sequence remains valid and delivers before releasing the original capture.
Stale/replayed projections cannot replace current refusal paint or redeliver
acknowledged words. Retiring overlay paint does not retire the original
identity-addressed `ComposerPending` obligation; even a terminal older than the last retired document revision still
reaches the existing receiver. Repeated lifecycle cannot repaint or release twice, while
a matching repeated terminal or fresh offer may retry an unacknowledged
handover. Intentional equal words stay
intact. No automatic sending, microphone acquisition or refusal auto-hide is
introduced. Empty refusal retains Error phase and only its projected capabilities.

- Delivery remains `ComposerPending`, `SinkAccepted`, `Retained` or
  `Unattempted`. ComposerPending requires the original capture/thread receiver
  to acknowledge it. Retained can mean an intentional archive/canvas route;
  it is not automatically a failed transcription or an accepted receiver.

The destination helper now returns a result. A selected sink's error, Noop or
missing accessibility permission
produces `StopDeliveryFailure`, `end_reason=delivery_failed`, Error phase and
Retained disposition. This also keeps clean-stop sink errors visible. Empty
typed refusal stays an error with `end_reason=coverage_refused_empty`, Error
phase and no delivery attempt. Actual errors continue using the existing
`transcription_failed` warning allowlist, with accurate recovery messages.
Usable refusal emits `terminal_coverage_refused` without claiming total loss.
Audio remains under the existing producer/controller retention owners; this
handoff never removes or rearchives the original refused WAV.

The existing shared Stop slot still owns duplicate callers, terminal reset and
successor exclusion. The delivery claim now covers every route before side
effects. Tests inject only capture evidence and receiver results into the real
ledger/emitter/Bus, refusal decision, delivery resolver and terminal reset.
They author assertions for unsealed diagnostics, original target, pending
composer, sink success/error/Noop, empty/string refusal, replay and successor
identity. Existing shared-operation barrier tests remain separate evidence;
their generic hold test shortcut is not proof of an actual refused recorder
Stop. Refusal-bearing Stop through the full recorder/shared-task chain and real
receiver acknowledgment remain unverified. All tests are **UNRUN**.

**Native receiver boundary (rc-w3-native-coverage-reason).**
`from_bus_event` carries phase, typed delivery and typed coverage together.
`OverlayMode.coverageRefused` renders retained words with the existing recovery
notice. `terminal_coverage_refused` remains quality telemetry: the warning
allowlist is unchanged and only `transcription_failed` reaches `on_error`.
The success callback and Agent final-text hint remain gated on `formatted`.
Receiver admission still belongs to `ComposerDictation` / `AgentChatStore`;
`Stopped`, visible copy and a source test do not certify installed delivery.

Rust `transcript_projection.rs` carries the typed phase from lifecycle rows;
`cli_transcript_lane.rs` only produces Completed/TranscriptionFailed today.
Any future CLI use of the new reasons must replace that lane's binary phase
classification. `bin/codescribe.rs`, `scripts/bus-demux.py` and Swift consumers
need joined replay/receiver tests; generated bindings remain read-only until
the integrator reconciles the other admitted API changes.

BUILD/TEST/RUNTIME=NOT_ASSESSED. Security and hygiene checks alone do not close
W2. Astra admits this isolated checkpoint and restores the Rust/bridge/Swift
gates when the terminal consumer seams and generated bindings are reconciled.

## Telemetry

One INFO line per stop / To Agent / overlay Insert / defer:

```text
delivery_route: intent=overlay_insert route=clipboard_paste reason=explicit_insert target=Ghostty
delivery_route: intent=orient_dictation route=clipboard_paste reason=auto_paste target=Ghostty
delivery_route: intent=orient_dictation route=archive_only reason=auto_paste_disabled target=Ghostty
```

The stop path adds a `seal_refused` field to that line and follows it with
`stop-path delivery finished delivery=Pasted …` or a `warn` naming why the
paste was parked.

`reason=refuse_paste_into_self` is the smoking gun for "Codescribe owned focus,
so the transcript was parked instead of being pasted into an unknown internal
caret".

## Terminal and CLI consumers

An overlay Insert into a positively latched terminal/editor uses one borrowed
clipboard swap and one Cmd+V. The terminal emulator owns bracketed-paste
handling; Codescribe never types the transcript character by character.
Alternate-screen confirmation remains a product walk-around, especially for
vc-frame and zellij key-routing combinations.

The CLI reads the same committed Transcript Bus. `codescribe transcribe live`
owns the canonical Rust wake/projection path; `bus-demux.py` is a named-agent
routing adapter, not another transcript reducer. For an editable shell prompt,
`scripts/codescribe.zsh` inserts `codescribe transcribe last` literally through
ZLE without appending Enter. That explicit line-editor path complements UI
Insert and does not create a second text authority.

## What this cut does not do

- It does not pick the transcript (Apple / Whisper / final-pass). That is
  `adjudicate_recording_truth`.
- It does not lock the microphone. That is still a missing `RecordingSessionOwner`.
- It does not make the agent chain mandatory. `previous_response_id` stays
  best-effort until that throne is cut.

Stacked on `fix/engine-routing`.
