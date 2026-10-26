# Clean Transcript Bus

## Held delivery

Agent-channel messages (1–9 and broadcast channel 0) use
`interaction.agent_send_hold_sec` in `settings.json`: integer seconds, default
10, 0 disables the hold, adjustable from 0–30 in Settings → Agent.
`CODESCRIBE_AGENT_SEND_HOLD_SEC` takes precedence through the config loader.
Dictation and paste delivery are unchanged.

A held `codescribe.agent-user-message.v1` or channel `transcript_sealed` row
carries `hold_until`, an RFC 3339 UTC timestamp with microsecond precision, as
does the sealed voice row's `codescribe.transcript-evidence.v1` twin. The hold
starts at publication/seal time, not when a follower happens to read the bus.
Producers omit the field at 0. `cs-bus --send-text --hold-seconds N` applies
one deadline to every recipient's row; its `message_published` receipt and each
recipient entry include the same `hold_until`. Existing broadcast policy
receipts remain present.

Controls are append-only rows on the **same bus** as the message, schema
`codescribe.delivery-control.v1`, with `kind` equal to `revise`, `withdraw` or
`release`. `identity` names the original typed `message_id` or voice
`utterance_id`; that named field is also included when the original source row
is available. Controls carry `control_id`, `emitted_at` and the exact
`provider`, `provider_session_id`, `lease_id`, `bus`, `channel` and `audience`
owner. `revise` additionally carries replacement `text`.
Voice controls also preserve the capture `session_id` so repeated numeric
utterance labels cannot select a different take. The overlay submits that
capture session to the helper, which resolves its source `utterance_id` on the
same bus; ambiguous bare utterance labels are refused rather than guessed.
The current whole-channel snapshot lacks a producer-authenticated numeric
utterance ID; such controls remain capture-session-scoped. Propagating canonical
engine utterance IDs into that snapshot remains an integration/design decision,
not permission to relabel an observation request as an utterance.

```bash
cs-bus --revise ID --provider PROVIDER --session SESSION --bus BUS < corrected.txt
cs-bus --withdraw ID --provider PROVIDER --session SESSION --bus BUS
cs-bus --release ID --provider PROVIDER --session SESSION --bus BUS
```

Ownerless `--channel 0` variants fan out to the currently bound recipient
buses, using the same binding/lease validation as typed sending. A control
cannot edit a different provider/session/bus owner. Nothing is removed from
the source bus, and no control recalls text already delivered to a model.

The follower is the only delivery gate. A future deadline parks the terminal
envelope in the lease's persisted `held` map, outside `pending`. It emits no
follower message, output line, native wake or seal hook for that envelope.
On the next polling tick after expiry, or on `release`, it follows the ordinary
queue/publication/wake path exactly once as a fresh delivery, with the latest
text. Unacknowledged delivered messages retain the normal restart replay
contract. `revise` replaces held text and keeps its predecessors in `revisions`
on the held record; that history is never copied into the delivered envelope.
Voice evidence `apply_manual_edit` / `apply_user_revision` for the held
`utterance_id` follows the same revision rule.

`withdraw` removes the held envelope and records one text-free
`delivery_withdrawn` follower event, **not** an ACK marker. Durable withdrawal
receipts prevent resurrection during cursor replay; original messages and
controls remain source history. Controls for unknown, already delivered or
hold-disabled identities are ignored with one stderr line. Broadcast policy
`off` is evaluated before parking (one skip receipt, no held row); the moon
(`quiet`) holds normally and later lands without a wake.

Attach, status and `--read-pending` receipts expose only
`"held": [{"identity": "…", "hold_until": "…"}]` beside `broadcast`, never held
text. The overlay projects these source rows and controls into held bubbles;
the existing 500 ms poll derives the countdown, with Cancel/Edit and Send now
actions. Cancel/Edit withdraws the held message and returns its text to the
composer. Draft evidence is not a held message, and countdown display is not
delivery authority.

## Agent attachment and conversation history

Connecting an agent restores its live mailbox; it does not import chat history
into the model's context. For an established agent, connection includes locating
and reading the relevant retained conversation using the
[history procedure](../skills/codescribe/references/history.md).

History belongs to the exact provider/session/lease. The same resumed owner can
open its existing conversation; a new instance reads its predecessor's saved
conversation as context. Reusing a channel for a different agent never relabels
that channel's previous occupant. Archive metadata identifies an owner and bus
path, not proof that the text was loaded or read. An empty unread mailbox is not
an empty history. Reading history neither ACKs another lease nor makes old
requests fresh commands.

## Agent panel focus

A click on an agent row asks that agent's own follower to bring its vc-frame
pane, or its iTerm2 or Terminal.app tab, to the Founder's focus. The app never
drives the terminal; the follower was spawned from the agent's pane or tab and
inherits its coordinates. A Codex agent's thread in the Codex app is the one
destination the app opens itself, from the lease alone (see
[Agent panel focus in Frame or a terminal tab](#agent-panel-focus-in-frame-or-a-terminal-tab)).

At start the follower records them in its lease as `frame`: `session`,
`pane_id`, `socket_dir`, `host_bundle_id` (`__CFBundleIdentifier`),
and `host_pid` (from `ALACRITTY_SOCKET` / `ALACRITTY_LOG`). `frame` is `null`
for an agent outside vc-frame. A follower that predates this contract reports
no `frame` until it is restarted. The address comes from the owner's own pane
(`VC_FRAME_PANE_ID`, `VC_FRAME_SESSION_NAME`, `VC_FRAME_SOCKET_DIR`, with the
`ZELLIJ_*` names as fallback), the exported Vibecrafted contract.

Beside `frame` the lease records `terminal`, the iTerm2 or Terminal.app tab
hosting the agent: `host` (`iterm2` or `apple-terminal`), `host_bundle_id`
(`com.googlecode.iterm2` or `com.apple.Terminal`), `tty` (a `/dev/ttys…`
path, or `null`) and `session_id` (the UUID after the colon of iTerm2's
`ITERM_SESSION_ID` / `TERM_SESSION_ID`, which is AppleScript's `id of session`; `null` for Terminal.app). The host comes from `__CFBundleIdentifier`,
then `TERM_PROGRAM` (`iTerm.app`, `Apple_Terminal`); any other host, vc-frame's
own terminal or an editor's included, leaves `terminal` `null`. A non-Claude
follower takes it from the environment it inherited at attach and the tty of
the nearest process in its ancestry that has one. A Claude Code follower
resolves it from the session's processes with the same discovery as the pane
(session files, daemon roster, ancestors, attached clients, PTY coupling) and
refreshes it with the pane; a daemon worker's own tree is skipped because it
inherits whichever terminal started the daemon. The tty and the iTerm2
session id are local identifiers: they are written to the lease only and
never to a bus row.

A Codex follower also records `codex`, its thread in the Codex desktop app
(installed as ChatGPT.app): `thread_id` (the `provider_session_id`, the id
the lease and the channel binding already carry), `app_bundle_id`
(`com.openai.codex`), `rollout` and `remote`. `rollout` is true when the local
Codex store under `$CODEX_HOME` (default `~/.codex`) holds the thread's
rollout, `sessions/YYYY/MM/DD/rollout-*-<thread_id>.jsonl` or the same name
(also `.jsonl.zst`) in `archived_sessions/`; the walk visits the newest date
directories first and is bounded. A found rollout is re-checked by path on
every heartbeat, a missing one is searched again at most every 30 seconds, so
a thread whose rollout appears later becomes openable. `remote` is true when
a process in the follower's ancestry is a `codex … exec-server --remote …`,
the Codex Cloud exec-server. Every other provider writes `codex: null`. The
object adds no new identifier and holds no credential.

The program is not part of the address. Each click runs the public `vc-frame`
resolved on PATH at that moment (contract of the Vibecrafted maintainer,
2026-10-19). `VIBECRAFTED_VC_FRAME_BIN` pins Vibecrafted's internal engine and
is not a fallback; with no `vc-frame` on PATH the click answers `error`.

The request is a row on the agent's bus, schema
`codescribe.agent-panel-focus.v1`, `kind` `focus_agent_panel`, with
`request_id` (24 lowercase hex), `emitted_at`, the exact `provider`,
`provider_session_id`, `lease_id`, `bus` and `channel` of the agent, and
`source` (`overlay_click`). It carries no text, status or capture coordinates.
The owning follower acts once per `request_id`, with a known pane first: it
runs `vc-frame action focus-pane-id terminal_<pane_id>` (vc-frame switches to
the pane's tab first). Without a pane and with a known `terminal` it runs one
`/usr/bin/osascript` (5-second timeout, no stdin): in iTerm2 it selects the
window, tab and session whose `id` is `session_id`, or whose `tty` is `tty`
when there is no id, and activates iTerm2; in Terminal.app it selects the tab
whose `tty` matches, makes its window frontmost and activates Terminal. The
session id and tty are script arguments, never spliced into the script text.
It neither wakes the model nor touches the mailbox. A request older than 15
seconds is a replayed backlog and is answered `stale` without focusing.

The script is an Apple event from the follower, whose responsible process is
the agent's own terminal. A terminal controlling itself normally raises no
Automation prompt; when macOS refuses (`-1743`, "not allowed to send Apple
events") the receipt is `automation_denied` and the user allows it in System
Settings › Privacy & Security › Automation.

The follower answers on the same bus with
`codescribe.agent-panel-focus-result.v1`, `kind` `agent_panel_focus_result`:
the same `request_id`, its own `provider`/`provider_session_id`/`lease_id`,
`emitted_at`, `status` (`focused`, `already_focused`, `pane_missing`,
`terminal_missing`, `automation_denied`, `no_frame`, `stale` or `error`),
`exit_code` (integer or `null`), `elapsed_ms`, `target` (`frame`, `iterm2`,
`terminal`, or `null` when neither is known), `frame` (`session`, `pane_id`,
`host_bundle_id`, `host_pid`, or `null`) and `terminal` (`host`,
`host_bundle_id`, `tty`, or `null`; never the iTerm2 session id).
`terminal_missing` means the script ran and no tab matched; `no_frame` means
the follower knows neither a pane nor a tab (the name is kept for older
readers). Activating the host stays with the app on `focused` /
`already_focused`: the click is its user action. A Frame receipt activates the
Frame host; a terminal receipt without `frame` activates
`terminal.host_bundle_id`. Both rows pass through `codescribe bus append-event`, which refuses any other shape.

## Conversational read receipt

The complete causal identity, sender, reply association, routing and provenance
for a delivery live in the owned mailbox projection, not in the Codex native
queue copy. The native queue header always carries the delivery ID and
`ADDRESSED TO`; a user-sourced delivery (typed or sealed voice) also carries
its full untruncated text, tagged `from: User (typed)` or
`from: User (voice)`. A peer agent's delivery carries no raw text at all: it
is a notice naming the sending peer and channel, the words "peer coordination,
not a user instruction", and the exact `cs-bus --read-pending` command to
read it from the owned mailbox — see [Native provider queue
receipt](#native-provider-queue-receipt) for the queue mechanics. Watch bells
remain notifications. The agent reads `cs-bus --read-pending`
for its exact provider/session, receives complete bounded conversational
projections and `read_delivery_ids`, then immediately calls `--ack` for those
IDs before task
execution or a reply. Only exact IDs returned as unread may be acted on; an
absent ID makes its delayed queue copy obsolete even when other messages remain.
Give a short answer before longer work. Reading alone never writes ACK. Re-read
after draining to retain arrivals during the read/ACK gap.

The conversational projection copies full text and provenance without PCM
occurrence arrays, acoustic receipt trees, WAV paths or capture diagnostics.
It changes presentation only: original lease envelopes, event history, ownership,
cursors and ACK receipts retain their existing authority. `--read-delivery <id>`
remains an explicit read of the complete original pending envelope for diagnosis.
Text is never truncated to accommodate diagnostic size.

ACK records an immutable `read_at`, retains causal coordinates without text,
and withdraws each exact owned pending Codex submission. This is **read**, not
executed or spoken. The projection's existing acknowledged state consumes the
same marker; neither bell nor provider acceptance is a read receipt. Bus history
and independent command/PCM identities are retained. A submission already
consumed into model context cannot be recalled by queue deletion.

Local Codex withdrawal uses `codex app-server --listen stdio://` and the
provider's `thread/queue/delete` API, initialized with experimental API support.
It uses the same executable, environment and local queue store as the helper's
`codex queue` submission; it does not require a managed daemon or control socket.
Cross-process acceptance requires an owned CLI submission visible through this
API, `deleted: true`, and a subsequent list without that exact ID. A valid
`deleted: false` means the submission is no longer pending in this local queue;
it does not recall a message already consumed by the conversation. This contract
does not select or discover a remote queue endpoint.

For a remote Codex mailbox, the client-side `cs-bus --remote HOST --watch` owns
native queue submission and withdrawal. Remote attach selects follower wakeup
`off`; the existing SSH watch transports full conversational envelopes and
durable ACK control frames to the Mac where the Codex thread exists. No reverse
SSH, second follower, second mailbox or provider network endpoint is created.
Both producer and consumer validate provider/session/lease/delivery ownership
before exposing an envelope. Peer messages wake the monitor without gaining
Founder authority. Local receipts are namespaced by mailbox host, lease and
delivery. Reconnect admits message envelopes only against the current source
mailbox bus and pending state. Historical ACK controls instead validate the
original causal envelope and bus, so reattachment cannot replay an old request
while queue settlement can still recover. Only terminal delivery IDs are
tracked for withdrawal. Reconnect replays admitted envelopes and confirmed
ACKs; known queue attempts are not automatically resubmitted. ACK races use durable markers and
the same per-delivery locks as local delivery. The relay forwards ACK controls
again for failed withdrawals, including ACKs issued directly on the mailbox
host. Existing local ACK markers are reused; incoming controls schedule
withdrawal on the same worker rather than waiting on the SSH reader. Settled
controls cause no recurring marker writes or provider tasks; explicit ACK
retains synchronous settlement reporting. Policy transitions publish newly
wakeable pending terminal rows through the existing follower stream, respecting
source `wakeup=off`. Repeating an explicit policy command repairs an interrupted
event publication with the same delivery IDs. The follower and policy-release
writers serialize each complete NDJSON frame in the follower event sidecar,
including partial kernel writes. A remote retry requires a bounded source-side read with
current bus, pending and wakeup-policy admission before client submission:
the moon (`quiet`) suppresses all wakeups, and channel-zero off suppresses
broadcasts only; the two are independent.
Remote status exposes local monitor and queue state; explicit retries
execute on the client. A missing or interrupted relay leaves the remote mailbox
intact and requires the existing watch to be resumed.

The helper bounds total provider stdout to 64 KiB and RPC execution to ten
seconds, then closes its child
process after each withdrawal. Failures retain the ACK and owned queue receipt
for retry, with the concrete transport/RPC error; successful settlement clears
the previous error. Withdrawal never starts, resumes or steers a turn.

Codescribe publishes one private, append-only NDJSON stream. The Bus observes
session lifecycle plus occurrence-authenticated `TranscriptRevision` entries;
it does not own a transcript document and accepts no arbitrary product text.
Dictation, Agent, and Assistive share the same capture and ledger authority.
Mode changes only the downstream delivery consumer.

This Bus never opens a microphone, scrapes SwiftUI, re-transcribes audio,
folds raw engine events, or reconstructs text from overlay deltas. It can copy
the ledger's seal-coverage receipt and historical optional comparison
receipts; neither gives the Bus mutation authority. Under the
[2026-09-08 amendment](SEAL_COVERAGE_AMENDMENT_2026-09-08.md), current normal
stop repairs exact uncovered PCM from the owned archive, drains newly scheduled
occurrence formatter work, then publishes final coverage and the terminal seal.
It does not produce an automatic whole-document comparison.

## Path contract

Resolution order:

1. `CODESCRIBE_TRANSCRIPT_BUS_PATH`
2. `$XDG_STATE_HOME/codescribe/transcript-events.jsonl`
3. `$CODESCRIBE_DATA_DIR/transcript-events.jsonl`, with the normal default of
   `~/.codescribe/transcript-events.jsonl`

The parent directory is created when needed. On Unix the file is forced to
mode `0600`. Each attempted append includes a flush before publication returns.
No host, date, room, or control-plane path is embedded in Codescribe.

### Persistence loss and live publication

The file is an observability sink, not admission authority for live text.
Production `TranscriptBus::open` retains a functional Bus even when opening the
file fails. `open_at` remains the explicit fallible file-opening API; production
uses its result through the same `open_with_path` fallback exercised by the local
failure fixtures. Start, authenticated revision and end publication advance live
state independently of append success. Failed revision writes still return every
authenticated entry and replace the existing last-projection snapshot. Failed end
writes still return exactly one lifecycle terminal with the exact committed bytes,
occurrence receipts and controller-owned delivery disposition. A duplicate end
returns no projection, including after persistence recovers.

`sequence` orders in-process publication within one Bus session. It advances for
attempted lifecycle and evidence publication, including failed persistence. On the
successful path persisted rows carry these same numbers. A callback's sequence
does **not** prove a matching file row, receiver admission or durable storage.
Even successful `File::flush` is not an fsync/power-loss durability receipt.
`ComposerPending` remains pending; file success or failure never manufactures
`SinkAccepted`. The existing composer receipt/recovery path receives the same
terminal payload; the Bus adds no second delivery authority.

On the first open, serialization, append or flush failure, persistence is disabled
for that Bus's lifetime. The existing tracing diagnostic records the session,
filename, error and `persistence=disabled_for_session`. Live projection continues.
There is no retry queue, payload journal, second reducer or fallback transcript
file: retention is the existing single last-projection snapshot, replaced by the
next committed projection. Process exit can lose unpersisted text. The UI has no
typed persistence indicator in the current bridge contract; failure is discoverable
in logs, and the visible document is live reducer truth, not a saved-file claim.
A dedicated UI persistence warning would require an explicit bridge/status and
Swift consumer extension; neither a display label nor delivery state encodes it.

An append may leave a prefix; a failed flush may leave a complete row. The Bus
never retries either and never reuses its sequence. A later session attempts a new
open with fresh lifecycle and no prior document. If the existing nonempty file
does not end in a newline, opening it fails explicitly and the new live Bus remains
usable without appending to that incomplete row. It neither truncates evidence nor
adds a newline that would pretend the partial row was valid. After explicit
external repair/rotation, a subsequent session can persist normally. A complete
row left by uncertain flush needs no replay; a later session may append after it.
This is not cross-process append locking or automatic damaged-log repair.

External tailers can therefore have an incomplete prefix of a session, an
unterminated final row, or no rows for a live session. They cannot reconstruct
missing bytes or infer successful delivery/idle state from that absence. Existing
install guards may conservatively refuse a stale unmatched start; this Bus cut
does not change installation or repair an external consumer's lifecycle policy.

## Event families

App `codescribe.transcript.v1` rows carry lifecycle only. `publish_started` emits
one empty `session_started` event for the controller-owned session, and
`publish_ended` emits one empty `session_ended` event when the controller
leaves that session (every path back to Idle, including zero-seal takes and
stop-timeout recovery). Neither can publish document text. The terminal row
does carry the already-resolved projection phase and action availability so a
file tailer can combine it with the last authenticated render without inventing
UI policy.

CLI file rows use the same schema with `source=cli_file_verdict`. Draft rows
retain per-segment `text` and supply an engine-assembled `rendered_text` snapshot;
seals supply the exact final document. Readers copy snapshots without joining
segments. Legacy CLI seals already carry the full document in `text`.
`session_ended` preserves the preceding document. CLI projections retain their
source and never claim occurrence or acoustic ledger receipts.

One microphone: the live app take is the most recently started app session
that has no later `session_ended` (or legacy `transcript_sealed`) for that
same `session_id`. Historical `session_started` rows without terminals are
abandoned takes — crash residue or buses written before the controller
always published an end — not a live recording.
`scripts/install-if-idle.sh` keys on that current pair, plus any unpaired
`source=cli_file_verdict` session (the CLI does not hold the install flock),
and on the agent-turn lease (`~/.codescribe/agent-turn.lock`, held shared by
the app for the whole turn). A merely running app does not refuse install
(Founder, 2026-09-08); the process-lifetime runtime lock only stops a _new_
app start while an idle-time install is copying the bundle. Never tear down
the app mid-take or mid-turn; never refuse install forever because an old
session lacked an end line.

`session_ended` carries one typed `end_reason` (`TranscriptSessionEndReason`):
`completed` for a take whose serialized stop and transcript processing succeeded,
`start_superseded` when a key-up or reschedule invalidated a hold start after
`session_started` and before the take became an active recording, and
`start_failed` when the recorder could not be started after the session was
announced. Lines written before 2026-10-24 may also carry `chord_prefix`, a
value no longer produced: a key that records is never the channel modifier
(`docs/HOTKEYS_CONTRACT.md`), so a take can no longer be a chord's prefix.
App refusal, stop timeout, and forced recovery use
`transcription_failed`; returning the recorder to Idle does not imply transcript
success. CLI file sessions use `transcription_failed` when decoding or
stream output fails after publishing a draft; the partial text is never sealed
as a completed document. The controller has exactly one terminal publisher
(`end_transcript_bus`); the delayed hold start unwinds every pre-active exit
through `unwind_hold_start`, so no started session is left without its
terminal publication. Persistence failure can omit its file line as described
above. The first terminal publication wins; later calls are no-ops.

A product recording can only begin after **acoustic admission**: the
controller resolves the input device without opening it, requires a measured
`EnergyCalibration` profile for that device from the immutable settings
snapshot (`energy-calibration.json` beside `settings.json`, see
`core/config/energy_calibration.rs`) and an armed Silero seal lane
(`audio.seal_lane_armed`, default `true`). The optional power-user
`CODESCRIBE_SILERO_FUSION` env value overrides that field when present in
either direction. A refused start writes nothing to the Bus and opens no
microphone. It reaches the overlay as a typed
`codescribe.presentation-status.v1` projection with kind
`admission_refused`, one actionable `admission_*` code, and Rust-owned display
copy. Guided calibration publishes `calibration_succeeded` (including the new
profile version) or `calibration_failed` through the same IPC/listener lane.
These passive status cards are not Bus rows and carry no occurrence, reducer,
or acoustic receipt fields. A card is shown even when the transcription overlay
preference is off; it then leaves on the ordinary terminal auto-hide countdown,
which the keep-visible pin does not suspend while that preference is off.

`codescribe.transcript-evidence.v1` is the committed projection family. Every
line is created only by `TranscriptBus::publish_revision(revision, ledger)` and
contains:

- Bus `sequence` and `emitted_at` metadata;
- controller `session_id` and `mode`;
- reducer revision, action, document index, complete `rendered_text`, and the
  entry label;
- exact occurrence identity: session, capture epoch, sample start, sample end;
- the matching acoustic serial plus word-evidence, layer-decision, seal, and
  manual-edit receipts copied from the ledger-owned reducer entry;
- optional per-entry `presentation_receipt` for new **and retained** Light+
  shapes, separate from acoustic decisions and human-edit receipts. It carries
  receipt/provenance/session, source and resulting revision, occurrence PCM
  coordinates, occurrence seal ID, source label, exact left context, its SHA-256,
  and shaped bytes. No field claims word-level timing for the shaped text.
- optional additive `seal_coverage` evidence: measured speech/covered sample
  counts, uncovered PCM ranges, the 250 ms threshold, and the availability
  contract below;
- optional additive `comparison`: SHA-256, character count, and rendered text
  for the pre-repair Apple-lane document and the whole-session local Whisper
  pass (historical/diagnostic; absent on the current normal stop path).
  These fields remain inside `codescribe.transcript-evidence.v1`; older
  readers may ignore them and Rust decoding defaults them to absent.
- the complete canvas contract: `phase`, `can_paste`, `can_insert`,
  `can_copy`, `can_retranscribe`, `can_format`, and `terminal`.
- additive `lifecycle_terminal` and typed `delivery` on evidence projections.
  Revision rows use `false` and `unattempted`. The in-process `session_ended`
  projection uses `true` and the controller's disposition (`unattempted`,
  `composer_pending`, `sink_accepted`, or `retained`). Its text-free persisted
  lifecycle row remains `codescribe.transcript.v1` and does not carry these two
  fields. Reading that row is not proof of composer admission.

### Shared durable revision encoding

Live callbacks still return a complete evidence projection for each occurrence,
with the same rendered document, availability, uncertainty spans and receipts.
Persistence encodes the selected projections of one publication as one NDJSON
row. Its top level is the first selected evidence projection, so document-history
and document-snapshot readers still find the full `rendered_text` in the evidence
family. A manual revision with no occurrence entries retains its standalone row.

An occurrence-bearing persisted row adds
`persistence_encoding: "shared-revision.v1"` and `occurrence_rows`. The array
holds each subsequent selected projection in publication order, with its original
`sequence`, `emitted_at`, `occurrence_session_id`, `capture_epoch`, PCM bounds,
`document_index`, `label` and complete `acoustic_receipts`. All remaining fields
come from the top-level projection. Expanding a row means copying those shared
fields and replacing only the listed occurrence fields; text is never joined or
used as identity. The first occurrence's receipts remain at the top level and are
not repeated in the array. This preserves Word evidence, decisions, seals,
presentation pins and manual receipts while storing the full document, coverage,
comparison and consultation payloads once per persisted publication.

`bus-demux.py` admits the complete encoded row before handling any occurrence,
rejects unknown encoding versions or malformed occurrence members, and advances
its durable byte cursor only after handling the row's envelopes. Unencoded
evidence rows remain readable. Snapshot readers that select one
projection per reducer revision can consume the top-level projection directly;
receipt inventories must also read `occurrence_rows`.

An addressed channel session is one evolving spoken message. The channel capture
session selects its identity; its PCM entries select evidence within that message.
The follower copies the full `rendered_text` once per reducer revision/action and
keeps every occurrence's original coordinates, label and acoustic receipts in an
additive `occurrences` inventory. `message_id` hashes `channel-message` plus the
capture session. Draft delivery phases hash `channel-message-revision`, session,
revision and action. The closing phase hashes `channel-message-seal` plus session,
independent of occurrence count or text. Distinct captures with identical words
remain distinct messages.

Only the channel's close or its own `session_ended` releases the final envelope.
A ledger terminal observation supplies certification but does not close capture.
Without that observation, the final envelope retains `coverage: refused`.
The conversation preview uses this same capture identity and copies the full
render instead of rendering an entry label as a separate chat message. Receipt
IDs for all physical entries stay observable; the ledger and reducer are unchanged.
ACK markers retain occurrence coordinates without labels or acoustic payloads.
The existing follower lease retains its last observed unclosed channel snapshot
alongside the committed bus cursor. Acknowledging a preview does not erase that
capture's final-delivery obligation. Restart restores these snapshots and pending
envelopes; successful close delivery clears the unclosed snapshot together with
the cursor commit. This is transport recovery of reducer bytes, not a new document
authority or another configuration store.

A shared logical revision below 512 KiB is one physical NDJSON row. Larger
revisions use consecutive `codescribe.bus-chunk.v1` transport rows with 32 KiB
base64 blocks, ordinal/total counts, the complete byte length, common observation
metadata, and the SHA-256 of the original JSON. All parts are one leased append
batch. No physical row limit is raised. Readers verify ordering, length, checksum
and metadata before expanding every occurrence; a durable cursor remains before
an unfinished batch. Unknown encoding, broken linkage, incomplete batches or
reconstruction above the current 256 MiB decoder budget refuse progress rather
than grant delivery or idle. Stored bytes are preserved even beyond that decoder
budget; support for such documents needs separate admission. This limit is not a
retention policy. Capture callbacks remain the complete original projections.

### Daily storage generations

The active appendable journal remains `transcript-events.jsonl` at the configured
data root. `transcript-events.jsonl.generations.json` is the sole ordered storage
receipt; `transcript-events.jsonl.generation.lock` serializes all cooperating
appenders and rollover. The first append on a new local calendar day pins the
closed inode under `events/YYYY_MMDD/<generation-id>.jsonl`, prepares a fresh
inode, durably records the pending swap, and atomically replaces the hot path.
Logical offsets and original stream identity span the entire linked chain.
Session end, channel close, seal, delivery and ACK still require their own rows;
midnight never mints any of them. Same-process sessions and reopened descriptors
refresh under the same lease. Pending swaps recover only matching original
identities; incomplete trailing rows refuse rollover. Existing archive names
are never implicitly overwritten.

Device numbers (`st_dev`) are physical mount coordinates, not durable volume
identity. On macOS the receipt pins the volume UUID. At startup, a reader or an
appender may recover a uniform device-number change under the same generation
lease, after checking volume UUID, every inode, archive length and recorded
archive SHA-256. The ordered chain is validated again before an atomic receipt
replacement. Only segment device coordinates change: stream ID, original logical
stream device/inode, offsets and all journal bytes remain intact. An active file
may have legitimately grown since its recorded checkpoint; shrinking is refused.

Receipts written before volume UUID pinning need independent reboot evidence:
the receipt predates the current boot, the original stream inode retains its
recorded birthtime, every linked file moved from the same old device to the same
current volume, and immutable bytes lacking a historical digest were not changed
after boot. The active inode must have a pre-boot birth and content modification
time; a later permission change may update ctime without invalidating those
bytes. Growth since the manifest checkpoint remains valid if its content mtime
predates boot. If compression replaced the original stream inode, the first
closed generation must have a recorded compressed flag and historical digest;
that digest is verified, and every linked file must have a pre-boot birth and
content mtime, with every closed generation also retaining a pre-boot ctime.
All recorded digests, physical inodes, lengths and the uniform device transition
remain mandatory. Actual post-boot content writes without a previously pinned
UUID still refuse automatic admission. A successful admission pins the UUID for
future restarts. Missing proof,
another volume, replaced inodes, digest mismatch, ambiguous pending rollover or
superseded-copy cleanup refuse recovery and preserve the original receipt and
bytes. Recovery never deletes the manifest or certifies a session as ended.

App startup applies this same leased recovery to the main journal and existing
managed regular `channel-*.jsonl` files under the authoritative agent bridge
home's `buses` directory. It does not follow channel/receipt symlinks, create
receipts for unmanaged files, or rewrite channel lifecycle events. A refused
channel reports its path and remains unavailable rather than looking idle.

An existing nonempty journal without a generation receipt is **undated**.
Admission pins all its original bytes under `events/undated/`; it neither scans
nor compresses that mixed-day source, and never invents a day from mtime. A
fragmented tail remains on its original authority. This source change does not
inspect or migrate the Founder's historical file.

Dated immutable generations are compressed off writer/capture/UI locks by the
maintenance owner using direct `/usr/bin/ditto` structured arguments
`--noclone --rsrc --extattr --hfsCompression`, with `DITTONORSRC` removed for that
child. Before publishing a compressed path it verifies `UF_COMPRESSED`, logical
byte length and SHA-256 equality, and unchanged source identity. These are
ordinary JSONL files: normal open/read/seek uses transparent filesystem
compression, with no container or manual decompression. Successful process exit
or an APFS clone alone does not prove compression. After durable publication a
recorded superseded-copy receipt allows safe removal of the ordinary daily copy;
interruption retains recoverable original bytes. Unsupported filesystems or
platforms retain an ordinary archive labelled `compressed: false`; failures
retain the source and back off for one minute. Empty queues spawn no archive
worker, and one worker per journal coalesces actual pending work.

ACK folds, transcript projection/history, CLI publication/live following,
Agent followers, orphan-tail discovery and the native delivery reader use the
same generation receipt and ordinary reads. Archive compression does not change
logical cursors. The installation reader keeps all existing recording, CLI,
channel, unknown-row and oversized physical-row checks; unverified storage never
certifies idle. Native observation rebuilding advances in 64 MiB passes across
dated generations, retaining the existing 64 MiB cold-tail treatment of the
undated prefix. It emits no delivery commands. History and orphan-tail lookups
retain their existing 64 MiB windows over the linked stream. The optional age
rewriter refuses managed generations; daily archiving does not expire evidence,
Dictionary correction/proposed stores or independently retained audio.

### Explicit historical migration preparation

This CLI is opt-in and **does not admit a replacement journal**:

```text
codescribe bus prepare-migration --source /explicit/source.jsonl --out /new/private/staging
codescribe bus prepare-migration --source /explicit/source.jsonl --out /new/private/staging --stage
codescribe bus prepare-migration --source /explicit/source.jsonl --out /new/private/staging --stage --compress
codescribe bus prepare-migration --source /explicit/source.jsonl --out /existing/completed/staging --stage --compress --resume
```

Without `--stage`, only source metadata is planned; no output is written and no
payload is scanned. Staging streams the pinned source length in 64 KiB blocks,
uses at most eight output descriptors, and keeps at most 1 MiB for record
classification. Known-schema JSON records with valid source `emitted_at` are
partitioned by that timestamp's **local** `YYYY_MMDD` day, preserving exact bytes
and order within each day. Invalid, unknown-schema, undated, oversized and
unterminated records are streamed intact to `events/unknown/`. The inventory is
per calendar bucket, capped at 4096 buckets, never per historical row. Exceeding
the preparation budget fails with source and owned partial staging preserved.

`preparation.json` is atomically published only after all staged logical byte
counts/checksums and source device/inode/length/mtime/ctime match. Diagnostics and
receipts contain identities/counts/checksums rather than private payloads.
Compression uses the same verified platform primitive; unsupported compression
is honestly recorded as ordinary preserved output. The original source and
ordinary staging copies remain intact as rollback. `--resume` re-verifies a
**completed** preparation, including all checksums and unchanged source; it is
idempotent reuse, not an interrupted-prefix checkpoint. An incomplete staging
directory refuses resume and remains evidence; a fresh explicit output is needed.
No preparation holds the append lease for a full scan or blocks capture locks.
Concurrent source mutation invalidates the preparation instead of admitting a
stale replacement.

`--apply` deliberately refuses: proven idle writer/runtime ownership and a
reviewed protocol for original cross-day ordering are still required. Calendar
partitions alone cannot reconstruct the global order of interleaved dated and
unknown rows; the intact source remains that authority. These outputs are not
installed in the live generation chain and therefore do not change history or
ACK state. Root must assess adversarial preparation, compression CPU/allocated
bytes and ordinary read/seek behavior before deliberate admission. This worker
has not executed any migration or compression against real or synthetic data.

Removing duplicate durable document snapshots and bounding the active generation
can reduce new growth. Complete archived history still grows over time; neither
daily generations nor transparent compression impose a total disk-use cap.
No claim is made that the previous 20 GB was reclaimed, that compression has a
particular ratio, or that reads necessarily become slower.

The projection contract is one snapshot, not a bag of Swift inputs:

| Field                               | Source of truth                                                                                                                                                                                                                                                             |
| ----------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `reducer_revision`, `rendered_text` | Exact committed reducer revision                                                                                                                                                                                                                                            |
| `phase`                             | `listening` for open book revisions, `finalizing` after a terminal ledger seal, then `formatted`, `coverage_refused` or `no_speech` from `session_ended` plus the last committed render; a terminal user revision remains `formatted`; failed/superseded starts are `error` |
| `can_paste`                         | The delivery throne selects `ClipboardPaste`, a latched target exists, and the take has ended                                                                                                                                                                               |
| `can_insert`                        | The delivery throne selects `ClipboardPaste` or `DeferredInsert`, and the take has ended                                                                                                                                                                                    |
| `can_copy`                          | The committed render is non-empty                                                                                                                                                                                                                                           |
| `can_retranscribe`                  | The session WAV exists and the take has ended                                                                                                                                                                                                                               |
| `can_format`                        | The take has ended and the committed render is non-empty                                                                                                                                                                                                                    |
| `terminal`                          | A terminal document revision or the controller's unique `session_ended` transition; a user/formatter revision can precede lifecycle end                                                                                                                                     |
| `lifecycle_terminal`                | Only the controller's unique `session_ended` projection; document revisions never release capture ownership or consume the delivery obligation                                                                                                                              |

`resolve_delivery_route(OverlayInsert, ...)` remains the only destination
decision. The projection layer queries its result; it does not create another
paste policy. The controller snapshots the target before overlay focus can
replace it, and checks the session-owned WAV only after the stop path has had a
chance to retain it.

An explicit edit, retranscription or version move carries its typed
`document_revision_receipt` on the resulting projection. A direct document
operation uses the reducer's authenticated publication capability: its
`capture_receipt_id` starts with `direct-publication-`, and its acoustic source
occurrence and seal lists remain empty. This is document provenance, never
an acoustic seal. Operation identifiers keep the provenance first, for example
`user-edit-direct-…`. The command acknowledgement and the projected receipt
name the same operation. Receivers finish a pending edit or version move only
after the matching document projection arrives; a command acknowledgement
alone does not repaint or complete it. Such a revision has
`lifecycle_terminal=false` and cannot release capture a second time.

The Bus refuses the **whole revision** before any row or sequence mutation if
the reducer's publication validator rejects it. Validation binds the exact ordered
entries, action, rendered bytes and other snapshot fields to the private reducer
capability, then checks session, current ledger labels, seal references, and full
current/retained shaping receipts with their source revisions and left contexts.
Missing, fabricated, stale or altered evidence cannot authorize shaped bytes.
An older/equal reducer revision cannot republish rows. The Bus does not admit an
occurrence, choose a label, infer identity, perform text-tail matching, mint a
seal or render another document. `seal_coverage` is emitted before terminal finality. A terminal
`LedgerSeal` reducer action marks the writer sealed only when the latest
coverage is **complete**. No arbitrary string can close committed Bus truth.

### Seal coverage availability contract

`seal_coverage.status` is one of `complete`, `incomplete`, `unavailable`.
Only `complete` may certify terminal truth; `incomplete` and every
`unavailable` reason block the seal in `AcousticLedger::seal_terminal` and in
`TranscriptReducer::apply_ledger_seal` alike.

| Field                | Values                                                                                      | Absent when                  |
| -------------------- | ------------------------------------------------------------------------------------------- | ---------------------------- |
| `status`             | `complete` · `incomplete` · `unavailable`                                                   | never                        |
| `unavailable_reason` | `not_observed` · `identity_mismatch` · `invalid_measurement` · `partial_observation`        | `status != "unavailable"`    |
| `speech_producer`    | `capture_energy` · `silero_boundaries`                                                      | never (empty on old records) |
| `availability`       | `observed` · `not_observed` · `identity_mismatch` · `invalid_measurement` · `discontinuous` | never (empty on old records) |
| `observed_samples`   | contiguous PCM extent the observer measured                                                 | `status == "unavailable"`    |
| `coverage_ratio`     | covered fraction of measured speech; a real `1.0` for measured silence                      | `status == "unavailable"`    |

The absent-ratio rule is the load-bearing one: a take nobody measured has no
covered fraction. `coverage_ratio` is never `NaN` and never a synthetic `1.0`,
because rendering one made absence of evidence look like a perfect take. When
the status is `unavailable`, `speech_samples`, `covered_samples` and
`max_uncovered_samples` are all `0` and carry no meaning.

A reader branches on `status` and, for `unavailable`, on `unavailable_reason`.
Never on the ratio, never on display copy. `availability` is the observer's
own state and is diagnostic: it explains _which_ observer failed and how, while
`unavailable_reason` is the ledger's verdict. They can differ — a
`discontinuous` observer produces a `partial_observation` verdict.

Availability semantics, as the producers report them:

- `observed` — the observer ingested a contiguous extent and every sample of it
  was finite. An empty range set here is **measured silence**, and it seals.
- `not_observed` — no PCM reached that observer for the bound identity.
- `identity_mismatch` — a measurement exists but names another session or
  capture epoch; nothing is relabelled onto the caller's identity.
- `invalid_measurement` — PCM arrived and some of it was non-finite. Non-finite
  input is substituted with zero before measurement, which makes an unmeasurable
  region indistinguishable from a silent one, so the observer refuses the whole
  take rather than certifying the part it could still read. Position does not
  matter: invalid audio before, after or between valid audio refuses alike.
- `discontinuous` — captured audio never reached the observer, either because a
  chunk skipped ahead or because the extent stopped short of what the take
  produced.

**Native coverage projection (rc-w3-native-coverage-reason).**
`CsTranscriptProjectionEvent.seal_coverage` copies the existing receipt through
`from_bus_event`, including sample counts/ranges, observer diagnostics and absent
ratio. Status and unavailable reason are Swift-visible enums. Missing legacy
coverage stays absent; unfamiliar wire tokens become `Unknown`, never Complete.
The native bridge decodes protocol tokens, not logs or display strings, and
creates no new evidence or transcript authority.

`OverlayState` derives both status and standing refusal copy from the admitted
projection. Incomplete measured speech and unavailable measurement have distinct
copy; each known unavailable reason explains why measurement could not certify
this take without asserting that words were lost. Missing coverage is unverified.
Text bytes, phase and capability bits still come from the same projection.
An empty Error projection gains neither words nor actions from the diagnostic.

Admission compares sequence, reducer revision and capture epoch **within the
same session**. Sequence must increase; revision and epoch cannot regress.
Multiple entry rows and the lifecycle clone may share a revision, so equal
revision with later sequence remains admissible. Each revision republishes its
ordered occurrence entries with the same full render; when a document spans
epochs, early rows below the admitted epoch do not paint, while the final row at
the current epoch carries that same document. This is presentation admission,
not a new occurrence reducer. The Bus still owns publication order and refuses
old/equal revisions at its writer boundary; lifecycle publication increments
sequence and clones the last document revision/epoch once.

Retired sessions are excluded from paint before ordering admission. Their
identity-addressed `ComposerPending` terminal still reaches its original receiver,
even when a newer document revision arrived before that late lifecycle. Delivery
uses the existing receiver admission receipt; retirement cannot cancel the debt. Exact sequence replay cannot repaint or release capture. A repeated
lifecycle cannot repaint or release capture twice; a matching repeated terminal
or a later fresh offer may retry a handover the receiver has not acknowledged. A new session starts its own order, clears its
notice, and preserves existing identity-owned recovery. Later document revisions
of the current session remain admissible and carry their own notice verdict.
No refusal arms auto-send, a success callback, auto-hide or microphone capture.
Installed/live acceptance remains a separate integrator obligation.

### Presentation provenance and serialization

The additive field remains inside `codescribe.transcript-evidence.v1`.
`ProjectedAcousticReceipt.presentation_receipt` is absent for legitimate plain
records and old records; deserialization preserves `None`. It never synthesizes
a receipt or treats absence as permission to shape. A present
`ProjectedPresentationReceipt` requires every field (including context bytes,
digest, source revision and source seal); malformed/incomplete objects refuse
decoding. Unknown fields inside that proof also refuse decoding. Older consumers
may ignore the additive outer field; they consequently have no Light+ proof.

Decoded observer JSON is not mutation authority. Only `TranscriptReducer` can
mint the private, non-serialized `TranscriptRevision` publication capability;
`publish_revision` accepts that type plus the live ledger, never decoded Bus
rows. Neither serialized receipt IDs nor the in-process digest constitute a
cryptographic signature authenticating an externally supplied log. Existing
trusted controller IPC transports the owner-produced JSON to the bridge; external
readers are observers of its provenance, not new admission authorities.

`CsProjectedPresentationReceipt` carries every field losslessly via
`CsProjectedAcousticReceipt.presentation_receipt`, with no new outer event.
The four non-generated Swift receipt constructors in ComposerDeliveryJoinTests,
OverlayRefusalLayoutHangTests and OverlayStateTests are adapted. Generated Swift
must be regenerated by W3 `make app-bindings`; checked-in generated code is not
updated by this W2 source checkpoint. The CLI projection reader remains a text
snapshot consumer and does not expose a full receipt audit surface.

Occurrence and terminal seal scopes are ledger-minted and have distinct IDs.
A one-occurrence terminal is still terminal. `EngineEvent::LedgerSeal` remains
an in-process, serde-skipped carrier, so no permissive old-JSON scope default
exists. `SessionFinalised` cannot fabricate a ledger seal. Presentation revisions
stay `lifecycle_terminal=false`, `delivery=unattempted`; only the controller's
unique lifecycle end carries destination disposition.

All new tests are UNRUN under W2. This schema description is source evidence,
not successful binding generation, installation or runtime delivery evidence.

### Terminal document revisions

The overlay canvas is read-only in every phase and displays the exact engine
projection, including empty text. Status messages and unfamiliar chrome phases
never replace or reject its document. A user-revision request sends
`session_id + source_revision + rendered_text` across FFI; it does not paint
Swift state. `TranscriptReducer::apply_user_revision` accepts only the
exact current terminal revision, and `AcousticLedger` appends a
`ManualDocumentRevisionReceipt` with `provenance=user-edit`, the source
occurrence/seal set, and the replacement bytes. A whole-document edit does not
invent new word-to-PCM alignment or rewrite any source acoustic receipt.

Rust then emits a new terminal `apply_manual_edit` evidence projection. The
projection carries the ledger's `user-edit-*` receipt in each source occurrence
row and is the only event that replaces the formatted canvas and delivery
buffer. It may follow `session_ended` because microphone lifecycle is already
closed. Replay accepts that terminal revision only for the just-ended session;
once a newer session is active, an older edit cannot displace it.

The Format dock command enters the same ledger-before-reducer commit corridor.
Rust reads the selected terminal document under `session_id + source_revision`
CAS, runs `format_text_with_status_for_policy`, and accepts an `Applied` result
as a derived presentation over unchanged acoustic text. Acceptance mints a
`navigation` document receipt, advances the CAS revision, and publishes the
formatter presentation over that new revision. A second result naming the old
revision is stale. `Failed`, `Skipped`, and `AiNoop` results add no accepted
version; diagnostic evidence may remain.

### Linear transcript versions

The reducer owns ordered accepted steps and one selected cursor. The first
explicit operation anchors the existing document once. Each accepted edit,
retranscription or formatting result appends one step, including an identical
successful retranscription or formatter output. Three retranscriptions and one
format therefore offer four Undo steps and four Redo steps. Navigation selects
saved bytes without rerunning recognition or formatting and does not append an
attempt. A new accepted operation after Undo discards the forward branch;
a refused or stale operation leaves it intact. Swift projects these steps and
blocks navigation while another document operation or an unsaved edit is pending.

The retained take mirrors these steps and cursor into its existing transcript
archive revision chain. Raw transcript and audio stay unchanged. Archived takes
use that same chain for navigation; selecting a take from History opens its
selected version on the overlay canvas with the normal actions. Retranscription
requires retained audio; formatting operates on text. The Previous take menu
continues to mean a retained superseded draft, not transcript versions.

Archive writes use the current chain head as CAS. A conflicting external write
or I/O failure preserves the existing archive and reports an unsaved-history
reason, including when projection delivery finishes before the operation ACK.
Live acceptance alone is not proof that a version was saved. A diverged live
mirror stops writing rather than merging concurrent archive edits.

Controller-authenticated context captures enter the same presentation reducer
as `RecordContextMarker` actions (`record_context_marker` on the Bus). The
reducer anchors each marker at the captured character position and renders it
into every later complete `rendered_text` revision, preserving capture order at
equal positions. Swift receives that finished projection verbatim; it does not
insert, pad, retain, or replay marker text.

### Receiver lifecycle and presentation guarantees

The overlay receiver keeps three things apart: the capture that is pending, the
previous take's authoritative document, and an unsent local draft. A new capture
is admitted on the controller's own `on_recording_preparing` /
`on_recording_started` callback while nothing is recording — never on a Bus
`session_started` row, because the consumer contract below forbids reading that
row as permission to mutate product state.

At that admission the receiver holds a monotonic, presentation-local capture
generation. It fences asynchronous UI work only: the terminal auto-hide wake, the
orphaned-"starting" watchdog, and the agent-handoff fade completion each capture
the generation they were armed under and refuse to act once it moved. The
generation cannot mint a session, an occurrence, a seal, a delivery
acknowledgement or acoustic evidence; those remain Rust-owned, and a UI counter
that named any of them would be exactly the forged authority this boundary
forbids.

A pending capture paints no previous speech and inherits no action capabilities:
phase, terminal flag, reducer revision and the five action bits are reset, and
the previous projection leaves the paint path. It is retired, not destroyed.

Superseded work has exactly one owner. Each superseded take is retained as a
single identity-associated unit — Rust's session id, the last projected
document, its reducer revision and the uncommitted draft that differed from it —
never as separate string and projection slots that a later capture can move
independently. A take with no observed projection has no identity and is
therefore not retained, rather than retained under one this receiver minted.
Delivery recovery still belongs to the composer store and the existing
retained-delivery documents, and no second transcript history store exists.

Retention is never implicitly evicted. No capture boundary, late event or
watchdog may drop an unsaved edit; only an explicit user decision resolves one.

Capacity is asymmetric, and the asymmetry is stated rather than dressed up as a
bound. Clean documents are genuinely capped at one, because each remains
authoritative in the ledger and reproducible from it. Unsaved edits have no
count limit. From this presentation fence the only two ways to impose one would
be discarding a user's edit, which is the silent loss the owner exists to
prevent, or refusing microphone admission, which is not this layer's decision to
make. So the store is bounded by the user's own unresolved decisions and not by
a constant, and the reported count is disclosure, not enforcement. That residual
capacity boundary is open and belongs to an owner outside this fence; it is not
closed by a second history store, and no receiver counter should be read as
closing it.

Recovery is reachable and explicit. The sole overlay action surface projects two
commands whenever retained work exists — recover and discard — and they are the
only commands on that rail sourced from local presentation state rather than the
reducer, because the bytes they protect are an edit Rust never saw. They are
offered independently of the phase table, so even a terminal outcome that
projects nothing but Close cannot make an unsaved edit unreachable, and
during a live capture the rail shows those labelled
commands only: the previous words never return to the canvas as current speech.
Recovery hands the retained bytes to the pasteboard, and consumes the retained
item only on a confirmed write. A refused write keeps the exact item — identity
and draft included — and surfaces the failure, because the alternative is
destroying the only copy of an edit at the precise moment the copy did not
happen. Recovery commits nothing to the reducer, mints no session, forges no
seal, submits no delivery and takes no focus. A scheduled draft commit for the superseded take is cancelled rather than
sent, so no revision request crosses FFI for a closed session while a new
capture is live.

### Refused coverage at the receiver (rc-w2-refusal-ui, W2 source checkpoint)

`coverage_refused` is a terminal presentation outcome of its own, parsed from
the wire like every other phase. The existing phase name covers refused terminal
finality, including a missing issued seal with complete measured coverage. It
does not reclassify complete coverage as incomplete. The optional coverage
payload is preserved unchanged; absent coverage stays absent. Retained-word
handoff requires the ledger refusal's exact session, epoch, rendered text and
optional coverage to match this already-published unsealed document. Bus remains
an observer: it neither mints a seal nor reconstructs missing projections.

Before it existed as an `OverlayMode` case the
receiver's `OverlayMode(rawValue:) ?? mode` fallback kept the previous phase, so
a settled refused take finished its life painted as `finalizing`. That is the
concrete defect this section closes; the fallback itself is unchanged and still
retains chrome for any phase the producer invents next.

The receiver states three things at once and may not collapse them:

- The words are the reducer's committed bytes and stay on the canvas verbatim.
- No seal was recorded. The success callback and the Agent final-text hint
  remain gated on `formatted` alone, so a refused take cannot fire the seam
  that arms auto-send, and no auto-send policy changes here.
- The destination is reported separately from both. A terminal ends capture;
  it does not acknowledge a receiver.

A standing, accessible explanation is painted from state rather than scheduled
as a toast, and the panel refuses an Assistive tray tick for this phase exactly
as it already did for `formatted` and `no_speech` — a refused take is the one
outcome whose only recovery handle lives on that panel. The explanation mirrors
the producer: it is present while the last terminal projection carried
`coverage_refused` and gone otherwise, so it cannot outlive the fact. The next
capture clears the notice and any persisting chip, and clears neither the
retained delivery documents nor the superseded takes, which are keyed by their
own session identities.

The `rc-w2-refusal-visibility` correction protects both auto-hide arming and
deadline evaluation while the current mode is `coverage_refused`. Hover exit,
drag and resize cannot rearm it, and a wake armed before refusal cannot close
the recovery panel or reach Agent delivery. The existing capture-generation
guard rejects a predecessor's wake before it touches a successor's timer.
Explicit human Close still reaches the normal close callback without changing
the refused verdict or deleting the retained document. A legitimate successor
capture owns its own presentation; its ordinary successful terminal still gets
the five-second countdown. The notice continues to mirror the producer's phase;
this receiver does not certify a later `formatted` revision as a valid seal.

Stored bytes and accessible controls are separate claims. Keeping a document in
`retainedComposerDocuments` proves storage in this receiver, not a reachable UI.
The correction keeps the current refused panel and its capability-driven
controls available until human dismissal or a successor transition. Recovery
after that transition depends on the existing receiver/superseded-take controls;
no new history or recovery store is introduced. Installed accessibility and
original-thread delivery still require joined runtime verification.

The rail projects the producer's capability bits on `coverage_refused` and on
`error` instead of a fixed `[close]`. A delivery failure keeps its words and its
retained audio, so Copy, Insert and Retranscribe are exactly the recovery the
user needs at that moment, and the previous fixed list hid them behind the word
"error". Format also follows the producer's `can_format` on a terminal document
in `formatted`, `coverage_refused` or `error`. The request creates an explicit
document revision; it does not certify an acoustic seal or erase diagnostics.
A false capability stays disabled, and an active capture, dirty draft or pending
document operation still prevents the request. Direct decoder completion alone
does not enable actions: the capture lifecycle must have ended first.

Delivery is unchanged and deliberately not phase-aware. Refused documents take
the same `ComposerPending` route, and only the receiver's typed receipt is
admission: `admitted`/`parked` consume the session's delivery slot, `retained`
keeps the bytes stored, `empty` claims nothing. A retained handover now shows
a persisting notice rather than a chip that fades after the toast window, since
a handover that came back is standing state. An empty typed refusal arrives as
`error` with no text and no bits; nothing is invented for it.

BOUNDARY — `end_reason` does not cross the projection contract, so Swift cannot
distinguish `coverage_refused_empty` from an ordinary transcription failure and
does not guess: both render the existing error card. Distinguishing them needs a
producer-side field, not a receiver heuristic. A user revision of refused words
may currently return `formatted` from the producer; its seal provenance remains
a producer boundary, not something this timer correction repairs. All tests
here are UNRUN under W2. The visibility tests use the existing injected clock
and deadline evaluator, including a saved pre-refusal deadline; no new test
uses wall-clock sleeps. BUILD/TEST/RUNTIME=NOT_ASSESSED; no W2 closure is claimed.

Retiring a session also fences its late events, on both receiver paths. A
retired projection may still complete its own addressed delivery and capture
release, but it cannot repaint the successor's canvas, change its chrome,
finalize it, arm its countdown or close it. The sibling presentation-status path
is fenced by the same identity: a status for a known retired session leaves the
current capture, text, mode and countdown untouched and paints no card, while a
status addressed to the current capture still presents and still releases it.

Status identity has an explicit disposition, read off the producer rather than
guessed. `PresentationStatusProjection` carries `session_id: Option<String>` and
a typed `kind`. A non-retired session id — including one this receiver has never
observed — is addressed to the current capture, because the lifecycle callbacks
carry no session id at all and the sole producer of `admission_refused` emits it
from the start path of the take the user just asked for; a refusal with no
session id is that same capture verdict and still lands. The two calibration
outcomes carry no session by construction: they are Settings-owned microphone
results, so while a capture is in flight such a status paints its card but may
not end a take it cannot name. Ending it would be minting capture identity for
an event that has none.

Duplicate `preparing` / `started` beats for an open capture are idempotent: they
do not re-fence the take, erase already admitted text, or restart the session
clock. A `preparing` that repeats for a capture which already proved it is alive
— recorder confirmed, audio or speech measured, final pass begun, or text
admitted — additionally may not un-prove it. Regressing the warmup state there
re-armed the orphaned-"starting" watchdog against a live take, and because the
capture and its generation were unchanged the generation fence passed and the
wake aborted a capture the user was still speaking into. Warmup is the only
state that watchdog may dismiss, so a genuine first warmup timeout — including
one whose `preparing` was itself duplicated before any audio arrived — still
recovers.

Visibility follows the current capture route plus the explicit overlay
preference. An enabled Dictation take stays visible through measured silence; an
explicit overlay-off runs headless; a genuine Agent/Assistive route stays hidden;
and showing the panel never takes key or main focus. There is no always-show
shortcut and no disabled animation.

One seam is not closable inside the receiver. `CsTrayStatusPayload`
(`bridge/src/tray_status.rs`) carries `kind`, `tone`, `indicator_mode`,
`assistive` and a monotonic tray `generation`, but no
session or capture identity. `TrayStatusStore` already refuses non-monotonic
ticks, so ordering is sound; what the payload cannot express is whether a current
`assistive` reading belongs to the live capture or to a different route. The
documented mid-hold `Fn` → `Fn+Shift` upgrade is delivered on exactly that tick,
so the receiver honours it and does not guess. Binding a tray tick to a capture
requires capture identity on the producer side; until then this is a stated
boundary, not an inferred hide cause. The lifecycle callbacks are identity-less
for the same reason: `on_recording_preparing` / `on_recording_started` carry no
session id, so a projection for a session the receiver has never observed cannot
be classified as predecessor or successor by presentation alone.

These guarantees are source contracts authored under W2. The Swift tests that
state them are UNRUN, and no binding generation, install or runtime reproduction
is claimed here.

## Authority boundary

```text
OccurrenceIdentity + AcousticLedger receipts
  → TranscriptReducer.document_by_occurrence
  + controller-authenticated context markers
  + terminal user revision receipt (source session + reducer revision)
  → TranscriptRevision
  → TranscriptBus::publish_revision
  → CsTranscriptProjectionEvent
  → OverlayState.applyTranscriptProjection

session_ended + last committed Bus render + delivery/audio snapshot
  → the same CsTranscriptProjectionEvent (`terminal=true`)

controller admission / calibration outcome
  → PresentationStatusProjection (not a Transcript Bus row)
  → CsPresentationStatusEvent
  → OverlayState.applyPresentationStatus
```

`EngineEvent::Preview` is ephemeral overlay paint. Raw `UtteranceFinal`,
`Correction`, `ReplaceRange`, and `InsertAnnotation` events are observation or
diagnostics. `Stats` and `SessionFinalised` are lifecycle. None can enter Bus
truth, delivery, history, clipboard, final controller text, or a terminal seal.
Lane and transport errors remain `Warning` / presentation-status / log
evidence. The session archive accepts a typed `Committed`, `NoSpeech`, or
`Unavailable` outcome: only `Committed` writes copyable user text;
`Unavailable` writes no transcript artifact, and `NoSpeech` has the fixed
history title `(no speech)`. Diagnostic strings are never history titles or
"Copy last transcript" candidates.

There is no Bus draft API, reducer draft storage, arbitrary-text
`publish_sealed` API, or raw-event `DeltaSinkAdapter`. The overlay's editor
state is ephemeral and cannot reach delivery. Consumers must observe the
authenticated evidence projection rather than reduce engine text themselves.

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

`codescribe transcribe live` uses the shared Rust projection reader. The
default stdout is the human canvas view: an extending revision appends only
its suffix to the open line, a non-extending reducer rewrite closes the line
and marks `⟲ rev N` with the full new render, and a terminal seal closes the
take as a permanent `⏺ sealed` block carrying the session, revision, and PCM
sample range. `--json` switches to the machine contract:
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

## File CLI and Finder

`codescribe transcribe --no-bus recording.wav second.m4a` decodes files in
order through the process-owned Whisper singleton. One failed file does not
prevent later files from running; the batch exits nonzero if any file fails.
Use `--no-bus` for archive batches that should not replace Copy-last.

When the bus is on (the default), retention sniffs twelve bytes: `RIFF....WAVE`
keeps the source bytes, otherwise `load_audio_file` writes a PCM-16 WAV at the
decoded rate. SHA-256 of those retained WAV bytes is recorded in
`sessions/.index/<sha256hex>` as the first `sessions/<session_id>.wav` name; a
hit hard-links that inode to the new session name (copy on EXDEV/EPERM/EEXIST)
so demux identity stays `sessions/<session_id>.wav`. `--no-bus` retains nothing,
which is why the Finder Quick Action is unchanged.

`--stream` prints and flushes newly admitted segments after each decode window,
before starting the next window. These are file-verdict drafts, not live
microphone observations. Overlapping windows use the same timestamp assembly
as non-streaming transcription. The final text is not printed a second time;
if final lexicon processing changes it, a `⟲ final:` line explicitly replaces
the draft. Without `--stream`, stdout contains only the final transcript.
Warnings and file/provenance headers go to stderr. Streaming does not remove
decode-window latency or make decoder tokens into committed transcript text.

Run `scripts/install-finder-quick-action.sh` to install **Transcribe with
Codescribe** in Finder's Quick Actions menu. It uses the installed CLI with
`--no-bus`, writes a sibling `.txt`, and copies successful results to the
clipboard. Existing output files are preserved and reported as failures.
Failed decodes leave no partial `.txt`, and other selected files continue.

## Native chat recipient recognition

Exact names and supported Polish suffixes remain case-insensitive and take
precedence over approximate matches. Without an exact address, the native
reader recognizes a unique registered name with one substitution, insertion,
deletion or adjacent transposition in the opening word (optionally after
`hej`, `cześć`, `hello` or `hey`). Approximation is restricted to names of
4–32 characters; it does not search every ordinary word in the transcript.
The envelope marks this result with `routing_match: "fuzzy"`.

Recipient discovery includes persisted offline identities on the same bus,
not just fresh heartbeats. A competing offline name must not send its messages
to someone else. Names are discovered once per input batch. Incomplete or
unreadable discovery disables approximate matching, leaving exact matching.
Multiple approximate candidates produce `kind: "routing_ambiguity"`, the
candidate list, and `state_change_allowed: false` for each candidate receiver.
The source text and reducer evidence remain unchanged; this is destination
recognition, not an ASR correction or proof that the words were spoken correctly.

## Optional audience field on sealed rows

Sealed transcript rows may carry an optional top-level `audience` string.
When `audience` equals a follower's bound name, that follower receives the
row without requiring its name to appear in the text; the emitted envelope
marks `routing_match: "audience"`. When `audience` is `"*"` the row is a
broadcast and every follower receives it with the same marker. An `audience`
naming someone else does not suppress the existing name gate: rows still reach
a follower if the transcript text addresses them by assignment, exact match,
or fuzzy discovery. Rows without `audience` behave exactly as before.

The digit that produced the row is configured in
`vc.agent-audience-binding.v1.json` under the agent-bridge home
(`CODESCRIBE_AGENT_BRIDGE_HOME`, otherwise `~/.codescribe/agent-bridge`).
Each digit `1`–`9` names one agent session (`audience`, `provider`,
`provider_session_id`). Digit `0` is the broadcast and does not need a row
in that file. A missing or malformed file refuses the channel instead of
opening the microphone.

### Broadcast policy

Each provider/session/leased bus has one runtime policy receipt under
`runtime/broadcast-policy/<key>.json` in the agent-bridge home. The key is the
first 24 hex characters of SHA-256 over NUL-separated lowercase provider,
provider session ID and resolved bus path, matching the playback mute key.
The receipt schema is `codescribe.broadcast-policy.v1`:

```json
{
  "schema": "codescribe.broadcast-policy.v1",
  "provider": "codex",
  "provider_session_id": "SESSION_ID",
  "lease_id": "<32 lowercase hex>",
  "bus": "<resolved leased bus path>",
  "policy": "off",
  "quiet": true,
  "emitted_at": "<utc>"
}
```

The receipt carries two independent axes (Maciej, 2026-10-23: "księżyc niech
routuje tym, czy może go do ciebie napisać, a broadcast niech routuje tym,
czy jest ten agent dostępny na kanale 0"):

- `policy` — channel-0 membership, `on` or `off`. `off`: do not queue a new
  broadcast; append an idempotent `broadcast_skipped` row with delivery
  identity, `origin_channel: "0"` and `policy: "off"` to the existing follower
  events stream. A skip is not an ACK marker and contains no transcript text.
  There is nothing to acknowledge. Off stays broadcast-only: a direct or peer
  delivery to an `off` lease still queues, so leaving channel 0 never cuts the
  agent off from addressed words.
- `quiet` — the moon ("tryb księżyc", do not disturb), `true` or `false`.
  Lit, every delivery from every channel — broadcasts, the Founder's directs
  and peer-to-peer alike — queues in the existing pending mailbox without
  follower output, native queue submission, seal hook or waiter completion.
  On the next natural turn, `--read-pending` includes them and the agent ACKs
  only what it read. The moon does not need channel 0: an agent in a meeting
  lights it and keeps its membership, or sits off channel 0 with the moon lit.

Every combination is legal. Readers that want the one word a broadcast meets
get `off` (not a member), `quiet` (member, moon lit) or `on`; a direct meets
`quiet` or `on` from the moon alone. A receipt written before the split
spelled the lit moon as `"policy": "quiet"` without a `quiet` field; it still
reads as on channel 0 with the moon lit.

Missing files mean `on`. Invalid, foreign or unreadable receipts log a warning
and also mean `on`: unlike playback mute's fail-closed silence, policy fails
open so damaged state never silently disconnects an agent. Typed Founder,
spoken channel-zero and peer broadcasts all use the same policy gate; direct
channel/name deliveries pass through only quiet, never off. Previously queued
messages are retained when the policy changes; already submitted provider
wakeups cannot be recalled by changing this preference. Clearing the moon
delivers what queued under it: the Claude/OMP waiter's unread poll surfaces
the rows by itself, and for a codex lease the policy write resubmits the
pending seals to the native queue once.

`cs-bus --broadcast on|off --quiet on|off --provider P --session S [--bus B]`
validates the lease before atomically writing the receipt; each flag moves its
own axis and leaves the other as the receipt had it, and the app always sends
both. `--broadcast on|quiet|off` alone keeps its pre-split one-knob meaning
(`quiet` = member with the moon lit; `on` and `off` clear the moon) for callers
that know only that flag. The omitted bus is resolved from that lease.
Followers read the receipt per delivery without restart or reattachment.
Attach, pending reads and status include `broadcast.policy` (the word a
broadcast meets), `broadcast.membership` and `broadcast.quiet` next to
`playback`. Typed and peer publisher receipts report each recipient's
`broadcast` policy, but still publish every admitted copy, including `off`;
the follower is the delivery authority. Sender snapshots are informational,
not a promise against a policy switch while the publication is in flight.

The overlay's channel-zero disclosure checks agents whose policy is `on`;
unchecking selects `quiet`. The header counts `on` over bound agents. Both
`quiet` and CLI-only `off` show a slashed antenna in the agent's own row.
Swift only paints snapshots of the policy receipts; it writes no preference
store of its own.

## Native chat receiver acknowledgment

For a provider/session-scoped `scripts/bus-demux.py` reader, stdout is transport,
not acknowledgment. Original addressed envelopes are stored in order in the
existing lease's `pending` array before emission or advancement of the bus
cursor. Restart replays unacknowledged envelopes with unchanged delivery IDs,
text and draft/seal permissions. An unscoped diagnostic reader has no mailbox.

After the receiving conversation durably accepts an envelope, acknowledge it
using the same helper generation, bus, bridge home, provider and session:

```sh
python3 scripts/bus-demux.py --provider codex --session SESSION_ID --ack DELIVERY_ID
```

Pass `--bus` and `--bridge-home` as well when the reader used explicit overrides.
The acknowledgment command does not attach a second follower. It refuses
unknown delivery IDs and mismatched sessions/buses. Repeating a valid receipt
is harmless. Receipt files under `acknowledgments/<lease_id>/` retain lease,
provider/session, selected bus and the original envelope's causal coordinates
and frozen recipient identities. They omit transcript text and WAV paths.
They prevent an acknowledged delivery from reentering the mailbox and let an
explicit reply validate its original owner after the pending mailbox clears.
The follower clears acknowledged pending text on its next loop or reattachment.
Earlier ID-only receipts still prove receipt, but cannot authorize a causal
reply once their original pending envelope has been cleared. Lease directories are private and files
are mode 0600; unacknowledged transcript text persists across process exits.

Pending storage is capped at 256 envelopes or 8 MiB of serialized envelope
content. At capacity, a continuous follower retains its lock and heartbeat,
pauses bus consumption, and resumes automatically after acknowledgment frees
space. A one-shot read exits 4 without advancing past the undelivered event;
acknowledge received items, then resume that same lease. Do not delete
recovery files or create a new provider session to clear this condition.

An acknowledgment proves receipt, not execution of an external side effect.
The conversation must retain delivery identity for its own execution discipline.
These mechanics do not supply provider wakeup, and upgrading the source helper
does not upgrade an already running follower. A generation without acknowledgment
support must not be reported as using this protocol.

## Agent receipt on the bus

The app watches `acknowledgments/<lease_id>/`. For a marker whose delivery is a
seal — still pending on the lease as `kind: seal`, remembered from an earlier
scan, or present on a bus row as `transcript_sealed` / `kind: seal` — it appends
one row:

```json
{
  "schema": "codescribe.agent-ack.v1",
  "kind": "agent_ack",
  "channel": "2",
  "delivery_id": "<24 lowercase hex>",
  "agent": "<audience>",
  "emitted_at": "<utc>"
}
```

The channel and `agent` come from `vc.agent-audience-binding.v1.json`: the
lease's `provider_session_id` selects the digit. A draft or revised marker does
not emit. The same `delivery_id` is appended once. Restart memory is
`runtime/agent-ack-cursor.json` under the agent-bridge home; existing bus rows
are the duplicate fence if that file is removed. Rows already on the bus are
not rewritten.

An open channel session also appends `codescribe.channel-session.v1` (`kind: channel_session`). `state: open` with `loud: true` means the microphone is
live. `state: sealed` with `reason: silence` means
`CODESCRIBE_CHANNEL_AUTOSEAL_SECS` elapsed after the capture's measured
Silero speech-end edge, or the session measured no speech edge at all for
30 s after it opened (time while an agent reply plays into it does not
count). The words so far are delivered and the session ends as
a hang-up does: its capture is released and nothing reopens it, so the next
message is the next Fn+digit (one press, one message; Founder decision
2026-10-26, which replaces the reopen half of voice seal `cc6c8248`). Text
arrivals do not start or reset these clocks. While speech is measured
active, or the speech-start/end pair after heard voice is incomplete, there
is no automatic silence deadline; manual hang-up remains available. `0`
disables both. `state: sealed` with `reason: hangup` means Fn+digit was
pressed again and nothing reopens the session. `state: sealed` with
`reason: orphan` means the process that opened the session ended without
sealing it (quit or crash); the next controller start writes it for each
named capture with an `open` but no matching non-open channel-session row,
reading only the last 64 MiB of each bus (shared, and every dedicated bus the
binding names). A replacement capture can open on the same channel while its
predecessor's decoder tail drains. That open never settles the predecessor;
only its own close, session-ended event, or orphan recovery does. Each channel
session gets at most one `sealed` row, on the bus that carried its `open` row,
after its capture closed and whether or not the ledger issued a terminal seal. The row names `opened_at`, `provider`,
`provider_session_id`, and `utterance_silence_sec`, repeated from the `open`
row; followers pair the two rows on them. `ChannelHudState` carries
the same open fact, including provider and session, for the overlay. The
overlay paint itself is a separate cut.

While in-process agent speech is playing, channel PCM is not offered to the
channel feed. External speech reports its actual outcome in a distinct
`agent_reply_playback` event. Reply text itself is published before synthesis;
its initial `spoken: false` is not evidence of a failed or completed playback.
The helper admits playback only after the existing capture interlock proves
idle, and stops its own player when a take starts. An open channel does not
block a reply by itself: the app publishes its measured speech phase in
`runtime/channel-speech-<digit>.json`. A `fresh` session (no speech since it
opened, e.g. a channel opened to answer a reply) admits any reply after 1 s.
A `quiet` session (speech, then silence, inside the same take) admits only
the reply of the agent bound to that channel: a pause in a message to one
agent is not room for another, which waits for that take to seal. Quiet is
measured again from the end of the previous reply. While a reply plays, the
speaker announces it in `runtime/agent-playback-now.json` and the app drops
all channel PCM (half-duplex); a key take, a channel opened during the reply
or measured speech stops it.

## Durable replies and exact speech controls

`cs-say TEXT --provider P --session S --reply-to DELIVERY_ID` first validates
that the delivery's pending envelope or acknowledged ownership receipt belongs
to this exact provider/session/lease and selected bus. It carries that
utterance's source coordinates and frozen recipients onto one
`codescribe.agent-reply.v1` / `agent_reply` event. Omitting `--reply-to` writes
`association: "unsolicited"` and `delivery_id: null`; no newest-question guess
is made. Explicit unknown or foreign delivery IDs are refused before publication.
A broadcast has concrete admitted recipients, so a later attachment cannot
acquire an earlier utterance's receipt obligation. The follower matches every
channel event's frozen provider/session/lease/bus before name routing. Equal
text in distinct document occurrences is never collapsed by string equality.

The helper resolves a manifest-owned `runtime/bin/codescribe` first, then the
existing `codescribe` on PATH and the known `~/.cargo/bin/codescribe` and
`~/.local/bin/codescribe` install paths. Bundled ownership requires its recorded
size and SHA-256 digest. It invokes `codescribe bus append-event --bus ABSOLUTE_PATH` with one JSON event on stdin. The Rust generation owner performs the private, chunked
journal append and durability barrier. No helper opens the bus for writing.
The publisher returns logical `offset` and `length`, plus `stream_dev`,
`stream_inode` and `stream_id`. A refused publication or invalid receipt never
starts speech. Text survives synthesis, credential, playback and capture failures.

Each reply has one 24-lowercase-hex `reply_id`. Speech has separate
`codescribe.agent-reply-playback.v1` / `agent_reply_playback` receipts naming that
same ID, the exact provider/session/lease and a 24-lowercase-hex
`playback_ticket`. States are `waiting`, `playing`, `spoken`, `failed`,
`refused` or `stopped`; `spoken` is true only for the completed spoken state.
`tts_error` and `reason` describe failures without credentials. ACK, provider
queue acceptance, reply text and speech completion remain separate facts.

`cs-say` still attempts speech immediately after text publication. In the app,
Play is explicit for one reply; viewing a tab never synthesizes or replays.
The installed helper's control interface is:

```bash
cs-bus --play-reply REPLY_ID --playback-ticket TICKET --provider P --session S --bus PATH
cs-bus --stop-reply REPLY_ID --playback-ticket TICKET --provider P --session S --bus PATH
```

Play reads one complete canonical source event, bounded to 32 MiB of storage
bytes, through the generation and chunk readers. A private derived receipt at
`runtime/reply-sources/<reply_id>.json` stores only source coordinates, bus and
owner; it contains no reply text and is not a transcript archive. Stream
replacement, missing source, incomplete chunks and foreign ownership refuse
playback. Existing `voices.json` profiles and the synthesis lane stay authoritative.

The helper's xAI speech is sent in a bare language code, never `auto`:
`CS_SAY_LANGUAGE`, `pl` when unset, cut to its first part (`pl-PL` and
`pl_PL` are `pl`) and accepted only as `pl` or `en`; any other value speaks
`pl` with one warning. With `auto`, xAI takes a sentence that opens with a
short Polish word (W, U, A, I) for English and spells it ("W nocy" read as
"double-u nocy"), and a long reply goes out one sentence per request; xAI
also reads `pl-PL` or `polish` like `auto`. The speech cache key includes the
language, so recordings made with `auto` are never replayed. The helper does
not read the app's settings; the app sends its configured speech language
the same way (`docs/STT_CONTRACT.md`, `tts_language`).

A voice gets words for listening, not the characters written for the screen
(`spoken_text`, applied once in `speak_published_reply` for `--say`, Play and
automatic playback, on both vendors). The published reply, its receipts and
the overlay keep the written text; only the synthesis request and the speech
cache key use the spoken one. The rules are fixed, not a model:

| Written                                                                             | Spoken                                                          |
| ----------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `hania → as:` opening a message                                                     | `Hania:`                                                        |
| `0.17.0 → 0.18.0`, `2/4`, `basia/2`, `12:51–12:53`                                  | `0.17.0 na 0.18.0`, `2 z 4`, `basia, kanał 2`, `12:51 do 12:53` |
| a path or file (`~/x/raport-20261026.md`, `core/llm/speech.rs`, `bus-demux.py:120`) | its last name (`raport`, `speech`, `bus demux`)                 |
| `runtime/receipts`, `sent_to`, `--takeover`                                         | `runtime receipts`, `sent to`, `takeover`                       |
| a commit or lease hash (7–64 hex with a digit), `+g…` build suffix, URL             | nothing                                                         |
| `11:53:17Z`                                                                         | local time without seconds (`12:53` in winter in Poland)        |
| backticks, `*`, list marks, fence lines, `∪`, `§`, `<name>`                         | dropped or said as a word                                       |

A line break becomes a sentence end. A text the rules leave unchanged is sent
byte for byte, so plain sentences keep their cached recordings; a text with
nothing left to say is spoken as written. Words follow the speech language
(`na`/`to`, `z`/`of`, `kanał`/`channel`). Agents still write to the founder
in plain words: the rules are a safety net for text that was written to be
read, such as a channel message played with Play.

A fresh ticket is required for each deliberate Play. Ticket reuse is refused.
The existing stable `runtime/playback.lock` serializes all speakers, including
channel 0. The microphone guard always starts from the canonical global
capture bus and discovers the dedicated channel buses; a reply's publication
path never substitutes for the global capture journal. The existing lifecycle
cursor enforces capture interlocks.
Lifecycle scans have a separate 500 ms slice budget from the playback polling
cadence. Faster polling does not shorten the safety scan; a scan that exhausts
its budget still refuses idle and stops playback conservatively. The original
cursor retains its processed prefix for the next scan.
Stop writes an owner-checked request for that exact reply/ticket and requires
its live ticket lock. The Python owner checks the request during lock/capture
wait and playback, and terminates only its own `afplay` child. A stop request
receipt is not completion; the terminal playback event supplies that fact.
The app's built-in chat player is a separate owner and cannot stop this player.

### Overlay conversation navigation

The header receipt mark opens the agent monitor in the overlay canvas. It is
a notification entry point, not a channel picker popover. The monitor separates
viewing a conversation from opening or hanging up a capture channel. My dictation
returns to the existing transcript canvas; selecting a conversation expands the
same overlay without changing capture, submitting text, acknowledging delivery,
or starting playback.

Conversation messages are displayed newest first. Only the view reverses the
observer's chronological projection; occurrence identities, reply associations
and the retained history stay unchanged.

When the controller opens a channel, the overlay follows the conversation with
that channel's provider and provider session. A historical owner on the same
digit cannot claim the new view. If the observer has not supplied that owner yet,
the monitor remains visible until the matching snapshot arrives. Closing that
channel or deliberately choosing a different view cancels this deferred
selection. Routine roster polling does not override manual review. Simultaneous
new recipients open the existing aggregate conversation, rather than choosing an
arbitrary agent. Header and conversation navigation never request speech.

The delivery observer's restart cache is the directory
`data.noindex/overlay-delivery-cursor/` under the data root (see "High-churn
data and Spotlight" below). Each bus is one cache unit (its cursor plus
its conversation projection) in its own owner-only JSON file, named by a digest
of the bus path; the file also records that path. Live snapshots consume bus
evidence and verified receipt metadata immediately; persistence is a separate
checkpoint. The first changed snapshot saves once, and later changes across all
buses share one checkpoint after 30 seconds since the last successful
checkpoint, or when one bus has advanced by at least 8 MiB. The byte threshold
triggers a checkpoint after a read; it does not lower the existing 64 MiB
per-read budget. Continuous activity cannot postpone the time deadline.
A checkpoint encodes and writes only the units whose bus changed since their
last successful save, each atomically (temporary file, fsync, rename);
unchanged units keep their files, and unchanged polls do not write at all.

Each unit pairs its projection with a cursor before any unfinished row or
storage transaction. Restart replays the suffix after that cursor. A failed
save keeps the previous file of every unit it did not replace and retries the
dirty state on the next poll. The existing 128 MiB cache budget, counted
across all unit files, evicts whole units, largest projection first; an
evicted unit, or the unit of a bus no longer in use once more than 16 are
cached, loses its file and returns to the bounded cold-tail path. The earlier
single file `overlay-delivery-cursor.v1.json` is never read; the startup move
brings it into `data.noindex/` and the first successful save deletes it. This
cache never advances the agent mailbox cursor, acknowledges delivery, or
establishes transcript truth. The overlay's reply read marks live beside it in
`data.noindex/overlay-read-state.v1.json`.

### High-churn data and Spotlight

Spotlight re-imports a file on every write unless it sits in a folder named
`*.noindex`, also one nested inside an indexed folder. Data that is rewritten
constantly and has no path-identity role therefore lives in
`<data root>/data.noindex/` (`~/.codescribe/data.noindex/`, or
`$CODESCRIBE_DATA_DIR/data.noindex/`). `Config::churn_dir()` in
`core/config/loader.rs` is its only resolver; Swift receives it as
`CodescribeConfig.churnDir()`. It holds:

- `logs/`: `codescribe.log` (Rust tracing; the app rolls it at 200 MB and
  keeps `.1` to `.4`) and `diagnostic.log` (Swift);
- `overlay-delivery-cursor/` and the earlier `overlay-delivery-cursor.v1.json`;
- `overlay-read-state.v1.json`.

Everything whose path is persisted, compared or shared by path with another
process stays where it was: this bus and its `events/` archives, channel buses,
leases, bindings, archives, `sessions/` (bus envelopes carry
`sessions/<session_id>.wav`, retention receipts name `sessions/` and `takes/`
paths), `takes/`, `transcriptions/`, and every `agent-bridge/runtime/` file that
`bus-demux` or the plugins read or write (followers, wake ledgers, reply
playback, playback mutes, broadcast policy, channel speech, the ack cursor).

The app moves entries left by an earlier build once per start, inside the
logging initialization and before the log sink opens
(`core/config/churn.rs`). The CLI moves nothing and creates no log folder: it
only appends to `logs/codescribe.log` when the app's folder exists.
`bus-demux` starts the CLI for every bus publication, and a CLI that created
the new folder first would leave both locations present for good. Each entry
is one same-volume rename that refuses an
existing destination: nothing is copied, deleted or replaced. An entry that
exists at both locations, or on another volume, is left untouched and logged as
an error on every start until it is resolved by hand. The moved entries are not
read or written by `bus-demux`, leases or the bus, so agents writing the bus
while the app starts are not affected.

## C11 evidence boundary

`484095ce` was the last executable-code cut before documentation successor
`d57196ab`. C11 is the next structural executable cut; its actual commit hash is
recorded only in the durable C11 report. Compiler, tests, runtime, app, install,
and release behavior are `NOT_ASSESSED` under the C11 embargo.

## Attachments bound to a live channel take

The overlay stages files against the displayed capture evidence, using
`cs-bus --stage-capture-attachment --capture-session ID --capture-epoch EPOCH --capture-source-sequence SEQUENCE --bus ABS --attach-file PATH` (repeat the
last option for more files). Sequence zero is valid. A channel digit, current
focus or latest text is never a substitute for the exact session and epoch.

The Rust append owner validates that the capture remains open and the source
sequence names an exact retained evidence row. Continued speech may advance the
latest sequence, revision or occurrence while a paste is in flight; that does
not retarget or invalidate the selected row. A changed epoch, frozen audience
or recipient set, and closed capture evidence still refuse admission. Sealing
one ledger segment does not close an otherwise open channel capture.
It copies the selected source occurrence
coordinates and frozen audience/recipients, then acquires every recipient bus's
normal append lease in sorted order. It validates the same source on every bus
before writing `codescribe.capture-attachment.v1` pending receipts. Only after
all pending writes are durable does it append the exact
`codescribe.capture-attachment-commit.v1` marker to the selected primary bus.
Readers ignore pending pointers without this commit. A partial write therefore
cannot authorize a partial broadcast; an idempotent stage ID permits an
open-target retry. Closed or changed targets refuse and retain the UI's files.

The follower preserves committed pointers across revisions/restart and adds
structured `attachments` plus readable pointer lines to the same sealed voice
envelope. It never emits a separate typed message or resolves new recipients.
The overlay projects the pointers on the same capture bubble; secondary history
resolves the named primary commit through its existing generation reader. The
transcript reducer's document is unchanged. Files pasted before evidence exists
wait without an error. The read-only opening receipt and frozen recipients bind
that wait to a capture already open at paste time. Its first matching evidence
automatically drains the files through the same append owner. A successor take
cannot inherit the files. Retry retains an already-bound exact segment; it does
not choose the latest occurrence or a new audience. The follower retains source
coordinates across revisions and restart, so continued speech cannot drop a
committed image from the final voice envelope. This segment association does
not yet supply the exact audio sample corresponding to the paste gesture.

## Overlay draft dictation and Reply

The conversation composer's microphone uses the existing shared controller's
single-turn composer capture and its admitted handle. Explicit Stop addresses
only that handle. Delivered text joins the original conversation's editable
draft, retaining its files and edits after navigation. It never auto-sends.
Terminal projections and identified terminal status release that take; late
projections still retain their original destination.

Bubble Reply selects a bounded `{message_id, author, text}` reference. Clearing
it leaves the draft intact; selecting the quote navigates to the retained
parent. `--send-text --user-reply-to JSON` publishes it as `user_reply_to` on the
actual user message and its queue envelope. It is separate from the agent's
`reply_to` delivery receipt and carries no capture authority.

## Written messages from agent conversations

`codescribe.agent-user-message.v1` / `agent_user_message` carries a random
24-hex `message_id`, `source_event_id` equal to that ID, `source: typed`, exact
UTF-8 text (at most 64 KiB), timestamp, channel and one frozen recipient with
provider/session/32-hex lease/bus. It contains no PCM coordinates, WAV, seal or
coverage claim. Five submissions of the same text remain five messages.

`cs-bus --send-text --channel N --provider P --session S --lease L --bus ABS`
reads the text on stdin. It holds the canonical binding's shared lock through
publication, refuses a rebound owner or an absent listener, and uses
`codescribe bus append-event` for the existing private generation journal.
The follower produces a `kind: message` envelope and uses its ordinary mailbox,
ACK and native queue path. Agent replies retain the exact delivery association;
written input never passes through acoustic finality or microphone ownership.
Publication uncertainty is reported without automatic replay.

The channel-zero composer uses `cs-bus --send-text --channel 0`. Under the
same binding lock, it validates every bound recipient before publication and
writes one private copy per recipient. Copies share a message ID and timestamp;
`origin_channel: "0"` records the broadcast while `channel` retains the exact
recipient channel for the publisher. The follower presents channel zero and the
overlay merges recipient receipts into one question. An empty roster or invalid
recipient refuses the send and retains the draft. Publication across journals is
not atomic: a write failure can leave earlier recipients delivered, so errors
must not trigger automatic retry. No audio capture or agent identity is invented.

Conversational deliveries expose `addressed_to` from the original source address.
For channel zero it names the publisher's frozen list when the copy carries one:
`0 (sent to: as, hania)`, with the same list under `sent_to` as `{name, channel}`
entries in mailbox reads, MCP reads and watch notifications. Every channel-zero
publisher writes that list into every copy: a channel take as `recipients` beside
`audience: "*"`, a peer `--send --to 0` and a typed `--send-text --channel 0` as
`target.recipients`. The list is the admitted set at sending (owners with channel
zero `off` are not on it; an owner on it can still skip the copy when its policy
changed before delivery), so it says who the message was sent to, not who read
it. The copy's own `recipients` and `peer_to` stay its delivery key, never the
address. A stamped empty list reads `0 (sent to: nobody)` with `sent_to: []`:
every bound agent had channel zero off, and the copy reached this mailbox only
for its own skip record. A channel-zero copy without the stamp reads
`0 (all audience)` and has no `sent_to`: its reach is unknown, and no reader
derives one from bindings or other agents' policies. `sent_to` carries the
publisher's names verbatim, like `recipients` and `target`; only the prose
label is sanitized and bounded (nine names, each cut to the queue-metadata
alphabet). Codex's native queue includes the same explicit `ADDRESSED TO:`
line. The queue header's channel identifies the receiving
mailbox, not the original address. A private message keeps its target
channel/name. A named utterance without a channel keeps its named audience; a
missing address is reported as unknown, never inferred from mentions in the
text. This projection changes neither routing nor delivery identity,
recipients, ACK state or the original message text.

## Receipts

Agents can read what the overlay already shows: who a message was delivered
to, who read it, who skipped it, whether it was withdrawn, and whether the
user heard it. Every such fact is one private file under
`runtime/receipts/<message_id>/`, written once by the process that produces
the fact and never rewritten, replaced or removed by any other process:

| File                           | Writer                                                                                                              | Content                                                                                                                                                                                |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `<delivery_id>.delivered.json` | the follower, when the copy enters `pending` (the backfill, for history)                                            | `name`, `channel`, `lease_id`, `provider`, `provider_session_id`, `bus`, `wake: on\|quiet` (`null` only from the backfill), `at`, `sent_to` when the copy carries the publisher's list |
| `<delivery_id>.read.json`      | `cs-bus --ack`                                                                                                      | the same identity fields and `read_at`                                                                                                                                                 |
| `<delivery_id>.skipped.json`   | the follower, instead of `delivered`, when channel zero is `off`                                                    | the same identity fields, `policy: off`, `at`                                                                                                                                          |
| `withdrawn.<delivery_id>.json` | the follower, on a `withdraw` control of a held copy                                                                | `control_id`, `at`                                                                                                                                                                     |
| `playback.<ticket>.json`       | the reply player, at the ticket's first terminal state                                                              | `state`, `reason`, `spoken`, `provider`, `provider_session_id`, `at`                                                                                                                   |
| `sent.json`                    | the helper publisher of a channel-zero text (typed `--send-text --channel 0`, peer `--send --to 0`), at publication | `sent_to` as admitted at sending, `bus` (the sender's bus for a peer text, null for typed text), `at`                                                                                  |

Every file carries `schema: codescribe.message-receipt.v1`, `kind` and
`message_id`; none carries message text or WAV paths. The identity fields
(`provider`, `provider_session_id`, `lease_id`) name the mailbox that
produced the fact; they stay in the private files and are not part of the
`--receipts` projection. The key is `delivery_id`, not the lease: after a channel handover the next session
acknowledges the delivery the previous lease received, and that is one
recipient in two files, not two recipients. Only copies of a message leave
receipts: typed, peer and channel-take copies carry a 24-hex `message_id`;
a plain dictation seal has none and leaves no receipt. Drafts leave none.

The state of a delivery is its farthest existing file: `delivered → read`;
`skipped` stands alone and is final (turning channel zero back on does not
deliver the skipped copy); `withdrawn` is a separate fact about a held copy
that never reached `pending`. A follower restart re-offering the same copy,
or a second `--ack`, finds the first file and changes nothing: `read` never
falls back to `delivered`. `--ack` writes the `read` file before the ACK
marker: the marker ends the delivery's pending life and a repeated `--ack`
is filtered by it, so a crash between the two leaves the half that the next
`--ack` completes, never a marker without its `read`. `wake` records the broadcast policy as it stood
at delivery (`on`, or `quiet` under the moon or the legacy quiet membership),
the same answer the follower gave the native wake; whether the copy wakes
now is read from the current policy, not from this file. The moon is not a
message state.

`spoken` is a fact per message, taken from the playback files, not from the
`spoken` field of the `agent_reply` row (which stays `false` after a later
Play) and not from the conversation that happened to play it: a playback of a
channel-0 text lands in the player's own conversation. One ticket is one
file; a later terminal publish of the same ticket (the failure path after a
refusal) changes nothing. The ACK marker under `acknowledgments/<lease_id>/`
and the skip marker under `broadcast-skipped/<lease_id>/` now also carry
`message_id`; their shape is otherwise unchanged and the app keeps reading
them.

`receipt_state(root, message_id)` composes the directory into one state:
`{message_id, sent_to, recipients: [{delivery_id, name, channel, state: delivered|read|skipped|no_receipt, wake, delivered_at, at}], denominator, withdrawn: [{delivery_id, control_id, at}], spoken: {state, reason, spoken, at, playback_ticket} | null}`. Agents appear by name and channel only; no provider,
session or lease leaves the files through this projection.

The helper owns the contract of the denominator: the formula below and
the model directories that pin it. `sent_to` comes from `sent.json` when
the publisher wrote one, so it does not depend on any reader having been
alive; without that file (messages before this helper generation, and
channel takes, whose list the app stamps into `recipients` outside the
helper) it is the stamp the recipients' files carried, or null when none
did. Every member of `sent_to` without any file is listed as
`{name, channel, state: no_receipt}`: the copy was sent and no reader has
recorded it yet, or the reader predates this generation. `denominator` is
the number of agents on `sent_to` plus every agent delivered to or read
outside that list, minus every agent whose copy was skipped (one agent by
name and channel, however many deliveries reached it); `no_receipt` counts,
`skipped` is shown but never counted. The list is frozen at publication
and a skip is recorded at delivery, so a member of `sent_to` can turn out
skipped: the skip wins, the entry stays `skipped`, and the count does not
wait for a read that cannot come. The overlay and agents read
`denominator` and the states; neither derives the count a second time from
the roster or from policies. `sent.json` proves nothing about the caller:
it does not open `--receipts` authorization, so an agent on the list with
no file of its own cannot read the receipts of that message, as before.
`spoken` is the completed playback when one exists, else the latest
terminal one. Files whose name and content disagree, foreign schemas and
foreign message ids are ignored, never repaired.

**One contract, two implementations.** This is a deliberate exception to
the one-owner rule: the overlay cannot call the helper, so the same
formula is computed twice, once by `receipt_state` in the helper and once
by the overlay's composer (`OverlayReceiptState.compose`, the one state
machine its projection keeps). Both are pinned to the same model
directories under `scripts/tests/fixtures/receipts/<case>/`, each with its
exact `expected.json`, and both test suites read those cases. Every new
state or rule goes to the model directories first, then to both
implementations; a change that passes in one implementation but not the
other is not done. The overlay computes only the formula from the files:
no roster, no policies, no reach of its own.

```bash
cs-bus --receipts MESSAGE_ID --provider P --session S [--bus PATH] [--bridge-home DIR]
```

prints that state as one JSON line (`kind: receipts`) and exits 0. The caller
must hold a copy of the message or be its author: a receipt file for this
lease (or provider/session), a pending or held envelope with that
`message_id` in this lease's mailbox, this lease's name and bound channel on
the frozen `sent_to` list, or ownership of `runtime/reply-sources/<message_id>.json`.
Anyone else gets `{"kind": "receipts_refused", "message_id", "reason"}` and
exit 3, the same answer whether or not the message exists. `--receipts` never
reads the bus or prints message text; names in its JSON are the agents' raw
names, like `sent_to`.

Receipts exist from the helper generation that writes them: followers started
earlier write `delivered` only after they reattach, and messages acknowledged
before that have no `read` file unless the backfill below gave them one.
Like `acknowledgments/`, `broadcast-skipped/`, `reply-playback/` and
`reply-sources/`, `runtime/receipts/` has no retention sweeper; all marker
directories share that limitation.

### Backfill

The overlay reads `runtime/receipts/` only and never falls back to the
older markers, so history from before receipts keeps its read count by
being rewritten into the same files, once:

```bash
cs-bus --backfill-receipts [--dry-run] [--bridge-home DIR]
```

takes no `--provider/--session` (it works on the whole bridge home, not on
one mailbox), combines with no other command, and prints one JSON line
`{"kind": "receipts_backfill", "written": {"delivered", "read", "skipped", "playback", "sent"}, "existing", "unmapped", "mapped_by_bus", "failed", "dry_run"}`, exit 0,
or exit 3 when `failed > 0` (a receipt that could not be written, for
example under an unwritable `runtime/receipts/`; a refused run never looks
like "nothing to do"). It is the
operator's one-time step after installing this helper; it is not run by
`make install-bus` or by a follower. Sources are read and never changed,
moved or removed; nothing is written outside `runtime/receipts/`:

| Source                                                                                                                                                             | Receipt                                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| a terminal row with a `message_id` in a reader's journal `runtime/followers/<lease_id>.events.jsonl` (that copy entered that mailbox)                              | `<delivery_id>.delivered.json` with `wake: null` and `at` = the copy's `emitted_at`                                                       |
| a copy still in `pending` of `leases/<lease_id>.json` (delivered, never acknowledged), read after the journals                                                     | the same `delivered` file; when the journal already named the delivery, write-once keeps the journal's file                               |
| `acknowledgments/<lease_id>/<delivery_id>.json` (`message_id` on the marker, or inside its `envelope` for markers older than that field)                           | `<delivery_id>.read.json` with the marker's `read_at`, its mtime when the marker has none                                                 |
| `broadcast-skipped/<lease_id>/<delivery_id>.json` (`message_id` on the marker; a marker older than that field is tied to its copy through the bus it names, below) | `<delivery_id>.skipped.json` (`policy: off`)                                                                                              |
| `runtime/reply-playback/<reply_id>.<ticket>.json` in a terminal state (`spoken`, `failed`, `refused`, `stopped`)                                                   | `playback.<ticket>.json`; a ticket still `waiting` or `playing` is left for the player, `.lock` and `.stop.json` siblings are not markers |
| the frozen `target.recipients` stamp on a journal row or an ACK envelope                                                                                           | `sent.json` with the copy's `emitted_at`; `bus: null`, the publisher's lane is not reconstructed                                          |

`wake: null` is the one value that means "history from before receipts":
the delivery is a fact (the row is in the reader's own journal), its wake
was never recorded and is not reconstructed. The runtime always writes
`on` or `quiet`; `receipt_state` passes `null` through as that value and
nothing else. The mailbox is the second source because of a gap in the
first: a copy delivered under the quiet policy (the moon) leaves no row in
the journal (the follower publishes a row only under `on`, while the
runtime of this generation writes `delivered` with `wake: quiet` for it),
so an unread quiet delivery is found only in `pending`; one that was
acknowledged is found through its ACK marker. In both sources `at` is the
copy's own `emitted_at`, the time it was published, not the moment it
entered the mailbox. A copy in neither (and without an ACK marker) gets no
`delivered` file and stays `no_receipt`. Only the copies the runtime
receipts are sources: a terminal kind with a `message_id`; a plain
dictation seal is passed over and a draft is not a delivery. Name and
channel come from `leases/<lease_id>.json`
and the copy's frozen `recipients` entry, as `receipt_recipient` resolves
them at runtime; the names are the agents' raw names.

A skip marker written before markers carried `message_id` names only its
`delivery_id`, its lease and the `bus` its reader refused to read from.
The backfill reads that bus (its whole generation chain) once per lease
with the reader's default chain (the normalizer, admission by the frozen
`recipients`, `consider` under the lease's name, without `--hear-all` or
fuzzy name routing, so a copy that reached a reader only through those
leaves its marker unmapped) and computes each copy's delivery id with
`delivery_identity`, the one function `SessionLease.enrich` stamps it
with. A marker whose delivery id is exactly one copy's takes that copy's
`message_id` and gets the same `skipped` file a marker with the field
gets; `mapped_by_bus` counts these. Text and time are never compared.

Nothing is guessed. A source that cannot be tied to a `message_id` (the
oldest ACK shape without an envelope, an envelope without `message_id`, a
dictation seal behind an ACK marker, a temporary `.tmp` file, an unreadable
JSON or lease file, a `pending` entry or `message` row without a usable
`message_id`, `delivery_id` or `emitted_at`, an incomplete last journal
row, a playback file of another schema, a skip marker without `message_id`
whose bus is gone, whose generation chain is over 256 MiB, or which
holds no copy, or more than one message, under that delivery id) is counted as
`unmapped` and left alone; `unmapped` is information, not an error, and
the exit code stays 0. Idempotence is the write-once rule itself, not a
register of what was done: a second run finds every file, writes nothing,
and counts them as `existing`; `--dry-run` reports the same counts without
creating a file or directory, so it cannot detect a write that would fail.
The command never prints message text.

## Open-channel co-audience

A nonzero Live agent-channel journal may carry `co_audience` on its emitted
rows. This is a sorted list of the other Live channels admitted by the same
local `RecordingController`, excluding the row's own channel. Membership comes
from the controller's canonical open-channel registry. It is a read-only
projection, not a recipient list or proof that anyone heard a whole utterance.
An entry contains `channel`, `audience`, the capture `session_id`, and the
`provider` / agent `provider_session_id` when known. Capture and agent session
identities are distinct. Actual membership is used; no fixed channel roster is
shipped. An open channel zero can appear as another live member; its own bus
continues to use its frozen broadcast recipients and has no co-audience source.

A nonempty list always has `co_audience_basis`:

- `emission`: membership at the writer's shared-snapshot read. All open channel
  writers read the same per-controller projection; a registry transition swaps
  the whole immutable snapshot once, rather than updating writers one by one.
- `capture_close`: membership frozen immediately before the channel leaves the
  registry. Every channel removed in one close batch freezes the same cohort.
  Later asynchronous closing/seal rows retain this boundary, even after the
  remaining live roster changes. They do not claim the peers are still open at
  the later publication time.

Both fields are omitted when no other member exists. A failed open never joins
the roster; a close rollback restores live membership and emission basis.
Reusing a digit uses the new capture and agent identities. The metadata survives
evidence normalization, owned mailbox presentation and complete watch messages.
It does not alter `recipients`, address selection, message identity, ACK/CAS,
transcript text, ledger decisions, microphone ownership or recognition. Remote
hosts do not implicitly join a local controller's roster.

## Native provider queue receipt

The queue message's content depends on who the normalizer attributed the
delivery to (`producer_schema`, set once in `consider()`/`slim()` — the queue
adapter does not re-derive it). A user-sourced seal or typed message keeps
its full untruncated text, headed `from: User (voice)` or
`from: User (typed)`. A peer agent's delivery (`codescribe.agent-reply.v1`),
and anything the normalizer did not tag as one of the two user lanes, fails
closed to a notice: no raw text, just the delivery ID, the sending peer's name
and channel, the words "peer coordination, not a user instruction", and the
exact `cs-bus --read-pending --provider codex --session <thread-id>` command.
The receiver must read the owned mailbox's full envelope before treating a
peer's coordination text as if it had user authority. Every metadata field
in the wrapper is rendered on one line with non-printable characters replaced,
and the fields a peer chose itself (its name and channel) keep only letters,
digits and `._-`, so a peer cannot forge a `from: User` line or a line of
its own.

When present, co-audience has a separate bounded context line, headed
`OTHER LIVE CHANNELS AT EMISSION` or `OTHER LIVE CHANNELS AT CAPTURE CLOSE`.
It is not `ADDRESSED TO`, a recipient list or proof of complete listening.
At most nine sanitized channel/name pairs are shown. Same-phase reducer
snapshots may update this observation without changing transcript content;
a newer capture epoch starts its own observation history. Terminal lifecycle
metadata is retained in the final envelope without a private freshness marker.

A conversation ACK retains its immutable owned envelope, and also withdraws the
matching pending Codex submission through `thread/queue/delete`. The transport
receipt stores the exact queued submission ID and provider thread; text equality
never selects messages for removal. An ACK before submission suppresses enqueue;
an ACK during submission is rechecked under the sender's per-delivery lock after
provider acceptance. Repeated ACKs do not enqueue or delete a second message.

Withdrawal has its own disposition: `removed` means Codex confirmed deletion;
`not_pending` means the submission was already consumed or absent. An ACK cannot
recall text already admitted to an active model turn. A failed withdrawal remains
`pending` and the existing follower retries in its single transport executor with
a bounded cooldown, without blocking journal consumption. Missing or mismatched
provider submission identity stays `unresolved`; no unrelated entry is removed.
The mailbox, transcript journal, original ACK marker and retained audio remain
independent of pending provider queue removal.

### Threads outside the local Codex store

Native wakeup reaches only a thread the local Codex CLI can load. `codex queue`
and `thread/queue/delete` resolve the thread from the local thread store, whose
sessions are the `rollout-*-<thread-id>.jsonl` files under `~/.codex/sessions`.
A thread that lives in the cloud — for example a Codex conversation started from
the ChatGPT app with a cloud environment, where this Mac runs only its executor
(`codex exec-server --remote https://…`) — has no local rollout. Every
submission is refused with `invalid thread-store request: no rollout found for thread id …` and the transport receipt records `disposition: rejected`.
Pointing `codex queue --remote unix://` at the shared local app-server daemon
does not help: that daemon does not hold the thread either.

Such an agent still reads, acknowledges, sends and speaks normally; only waking
it after its turn has ended is unavailable. Keep its turn open with one
`cs-bus --watch --until-event` for as long as it must hear the bus, or let an
orchestrator that is woken through its own channel resume it. To see where a
thread lives, follow the process tree of its `cs-bus --watch`: a
`codex exec-server --remote` parent means a cloud thread.

## Unexpected listener loss and recovery

A listening session has four layers. Each is observed by a different component,
and only some can be recovered by the helper:

| Layer                                                                    | Observed by                                               | Recovered by                                            |
| ------------------------------------------------------------------------ | --------------------------------------------------------- | ------------------------------------------------------- |
| Follower process (lease owner, mailbox writer)                           | the session's `--watch`                                   | the same `--watch`, through owned recovery below        |
| Watch process                                                            | the provider's monitor (exit notification) and `--status` | the agent only: it restarts `--watch` under its monitor |
| Provider notification window (for example a bounded exec/monitor window) | the provider only                                         | the agent, by renewing the window                       |
| Provider conversation                                                    | the provider only                                         | not recoverable by the helper                           |

The helper cannot see a provider's notification window or conversation. A
window ending while the watch keeps running is not a disconnect, and the helper
does not claim to notify through an expired window.

Managed followers retain the resolved workspace in their lease wakeup
configuration as well as the PID record. Losing the PID record therefore does
not change the configuration or force a verified live follower to restart on
reattachment. Recovery carries that recorded workspace through both the follower
launch and its confirmed configuration, using the lease when the PID record is
missing. Legacy records without an explicit workspace do not invent one.
A changed workspace still requires the existing drained handover.

A watch started with `--provider/--session` checks its follower every two
seconds. A follower counts as lost only when two consecutive checks find no
live follower process, the lease lock is free, and the channel binding still
names this exact provider session. Quiet time, a stale heartbeat and a renewed
watch are not losses. A held lease lock means a handover is in progress.

Recovery runs under the canonical binding lock, in the same order as attach,
detach and archive. It starts one follower with the recorded name, channel, bus,
wakeup, hook and explicit workspace through the same launch and readiness path as `--attach`. The
replacement acquires the same lease and resumes its cursor and unread mailbox.
Recovery writes no acknowledgment, resubmits no accepted queue copy and replays
no transcript. Each lost follower incarnation (pid plus start time) is handled
at most once. Detach, archive and takeover remove or replace the binding, so
nothing they ended is revived. A session that does not own the binding cannot
recover it. Forks need their own session ID.

The watch reports recovery on its own stdout as a single
`codescribe.agent-bridge.lifecycle-notice.v1` line (`notice: "Codescribe listener lifecycle"`). It is independent of the lost follower. Events are
`follower_recovered`, `recovery_failed`, `recovery_suspended`, `unrecoverable`
and `listener_ended`, after which the watch exits. A notice has no
`delivery_id` and no transcript text. It is never in the mailbox and is never
acknowledged.

Two consecutive losses without a newly queued message suspend automatic
recovery (Founder decision, 2026-10-10). A suspended session stays visible as
`listener.recovery_suspended` in `--status`. For Codex, the suspension notice is
also submitted once through the native queue, so a later turn learns of it after
the watch has ended. A newly queued terminal message or an explicit `--attach`
resets the streak. Bookkeeping lives in `runtime/followers/<lease>.lifecycle.json`;
the watch registers in `<lease>.watch.json` for `--status`.

## Disconnected agent archive

The overlay drawer offers **Remove from list and move to archive** only for a
closed channel whose controller roster explicitly reports a dead follower.
Unknown liveness and live readers do not offer that action. The click freezes
the provider, provider session, lease and channel, rather than selecting by name.

The managed helper's `--archive-agent CHANNEL --provider P --session S --lease L --bus PATH` holds the canonical binding lock and the exact lease's exclusive
lock through publication. A replaced owner, living PID, busy lease or unreadable
mailbox refuses the operation. It never signals a follower, acknowledges a
delivery, advances a cursor or removes transcript/audio/mailbox history.

The helper writes `archives/L-CHANNEL.json` (`codescribe.agent-archive.v1`) and
then releases only that channel through the binding file's existing writer.
The receipt includes the frozen owner, bus, name, time and `released: true`.
The overlay reader ignores archive metadata while that exact owner remains
bound, including a failed binding write or deliberate reattachment. Otherwise
the conversation remains in saved conversations; an empty history also retains
an archive entry. Reusing the channel exposes the new owner independently.

## Agent panel focus in Frame or a terminal tab

Each agent row resolves one destination for its focus button
(`overlay-focus-panel-CHANNEL`, unchanged): the Frame pane when the roster
reports `frame_detected` and a compatible `vc-frame` is installed (below),
otherwise the iTerm2 or Terminal.app tab when it reports `terminal_detected`
(the lease's `terminal.host_bundle_id` is non-empty), otherwise a Codex
agent's thread in the Codex app when it reports `codex_thread_url` with
`codex_thread_local` true. A Frame or terminal button shows only for a live
follower; its help reads **Focus agent panel in Frame** or **Show the agent's
terminal tab**, with the `terminal` symbol for a tab. `vc-frame` capability
gates only the Frame destination.

The Codex thread is the one destination the app reaches itself. The roster
reads the lease's `codex` object without the heartbeat filter (the newest
lease of the provider session wins; an id outside 8–128 letters, digits, `-`
and `_` is dropped) and builds `codex://threads/<thread_id>` in one place,
`ChannelHudState.codex_thread_url`. A click opens that URL with
`NSWorkspace`: the Codex app activates and shows the thread, without
AppleScript and without an Automation permission. No
`codescribe.agent-panel-focus.v1` row is written and no follower acts. This
is a deliberate exception to "the follower acts": opening a URL is not
driving a terminal, and Codex app followers are usually dead between turns,
so the button stays enabled while the row is dimmed as **Disconnected**. Its
help reads **Show the agent's Codex thread**; the row answers at once with
**Codex thread opened** or **Could not open the agent's Codex thread**.

The link resolves against the Codex app's local thread store. A Codex Cloud
thread has no local rollout (`codex_thread_local` false) and its link ends in
the app's own "no rollout found for thread id" error, so with no Frame pane
or terminal tab its button is shown greyed with **Codex Cloud thread: open it
in ChatGPT** instead of failing on click.

The overlay drawer starts fail-closed and renders **Focus agent panel in Frame**
only after a public `vc-frame` command on `PATH`
(or on the user's non-interactive `zsh` `PATH` when a `launchd`-spawned GUI
process inherits only `/usr/bin:/bin:/usr/sbin:/sbin` and `$HOME/.zshenv` or
`$HOME/.zprofile` is present) is verified to support
`vc-frame action focus-pane-id <PANE_ID>`. Capability detection probes
`vc-frame action focus-pane-id --help` with session and socket variables
stripped. Positive results are memoized by the executable and wrapper-target
file stamps so older compatible `vc-frame` releases and current releases are
both accepted without version-string equality, focus changes, or session side
effects, while negative results are never cached so repairing a missing wrapper
dependency restores availability on the next poll without restarting Codescribe.
Literal `~/.local/bin` fallbacks and `VIBECRAFTED_VC_FRAME_BIN` are never used.
When a compatible `vc-frame` is absent or unsupported, the Frame destination is
excluded and only a known terminal tab can still offer the button; when a
destination exists and a channel's follower is disconnected, the button
remains rendered and disabled.
