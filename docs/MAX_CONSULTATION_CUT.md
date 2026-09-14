# Max consultation — implementation contract

Status: structural implementation in progress; not assembled or verified.
Founder request: 2026-09-14, Roman conversation.
Baseline: b91e9a947941371d0d51ba6cee1e3e68e7331e8c.
Runtime: isolated Fleet Worktree `cut/roman-max-consultation`.

## W0 execution registration

Roman owns this single sequential cut and its later integration review. Phase
W1 starts with the clean pinned code baseline above; this plan is the only
pre-existing untracked file and belongs to Roman. Enforcement grade B:
operational, not a claim of hook enforcement. Recovery branch is
`cut/roman-max-consultation`; parent integration remains explicit, not automatic.

Closed source domain: app Agent provider/tool/runtime wiring, controller
formatting admission, core Agent/formatter execution and streaming request
handoff, bridge/UI consultation controls, and their direct tests/prompts/docs.
No concurrent writer is assigned that domain. Sol's historical integration
investigation is read-only and disjoint.

During structural W1/W2, do not run `cargo build`, `cargo check`, `cargo test`,
`cargo clippy`, `cargo fmt`, `rustfmt`, `swiftc`, `swift-format`, `xcodebuild`,
`make check`, `make verify`, `make test-swift`, `make app-bindings`, `make app`,
`make install-if-idle`, application probes or dependency installation in this
worktree. The Max executor/identity/tool handoff is not assembled, so partial
gates would assess a different system. All applicable gates return after an
exact-SHA W2 structural-close receipt; security checks skipped by any checkpoint
must be restored. No gate suppression has been performed yet.

Allowed evidence: Git history/diffs and `git diff --check`, Loctree maps,
bounded source/schema reads, and non-executing security/hygiene inspection.
Author tests now; execute them only after integration closure. Maximum three
mechanical repair attempts after the first compile; architectural disagreement
reopens structural work rather than weakening tests. Final verification includes
full `make check`, `make verify`, `make test-swift`, generated bindings and the
idle-safe installed-app handoff, followed by real two-turn consultation proof.

## Required behavior

Max is an instruction-following augmentator, not an unconditional expansion
writer. It can produce the requested result, consult context and use existing
Agent tools. Preparing a shell command does not authorize executing it.
Correction and Smart remain text-only formatting policies. This restriction
does not disable the independently selected Agent chat product.

Successive utterances in the same consultation retain the user's instructions,
corrections and previous answers. A new consultation must not inherit another
consultation's history. Changing provider/settings must not silently erase local
history or substitute the Agent chat's selected provider for the formatting lane.

The Founder request to include an original message applied to one particular
take. There is no global original-plus-expanded paste requirement.

## Observed source boundaries

1. `app/controller/mod.rs::apply_formatter_revision_from_overlay` formats an
   accepted terminal document, then uses the existing revision CAS.
2. `app/controller/mod.rs::format_composer_turn_once` formats the terminal
   composer turn and admits the result through the same presentation owner.
3. `core/pipeline/streaming/apple_live_session.rs` drains `formatter_rx` during
   capture and calls `format_text_with_status_for_policy` per occurrence.
   Its `FormatterCompletion` retains exact PCM occurrence identity. This path
   must participate too; an app-only replacement would leave daily hands-free
   Max behaving differently.
4. All three currently use the one-shot formatter in
   `core/llm/ai_formatting.rs`. It receives neither consultation identity nor
   an Agent tool executor. The baseline Max prompt forbade answering
   instructions and required 1.5–3x expansion; that prompt is now replaced in
   this unassembled worktree, not in the installed app.
5. `core/agent/session.rs::AgentSession` already owns history, provider chaining,
   tool round-trips and permission decisions. Reuse it; do not create another
   model/tool loop.
6. `app/controller/helpers.rs::AgentRuntimeState` demonstrates durable thread
   recovery and serialized turns, but its shared slot follows Agent UI thread
   selection. Do not attach Max blindly to that slot.
7. `app/agent/mod.rs::create_provider_for_lane` now requires an explicit lane
   and rejects tool-capable formatting outside Max. Existing chat consumers
   select Assistive. Max must retain the immutable formatting lane's
   provider/model/auth identity; its execution path is not connected yet.
8. `app/agent/tools/clipboard.rs` already exposes `read_clipboard` and
   `write_clipboard`. Reuse registration and permission policy; do not use an
   unrelated eager clipboard snapshot as if it were a tool result.

## Structural implementation obligations

- Pass explicit consultation/turn identity and a host-provided execution
  capability into the shared formatting path; the core must not import macOS
  tools from the app crate or read a second mutable settings authority.
- Route Max to the existing AgentSession loop; retain Correction/Smart requests
  without tools. A missing Max executor is an explicit unavailable result, not
  a fabricated claim of tool execution.
- Serialize Max turns in spoken order before invoking a provider. Ordered
  result delivery alone does not serialize tool side effects.
- Retain user and assistant roles separately. Tool output and captured context
  are data, never new user authority. Keep existing mutation approval checks.
- Preserve occurrence identity, revision admission and delivery routing. An
  answer may alter the text label but must not invent acoustic occurrences.
- Expose a deliberate consultation reset/new-conversation action and durable
  identity; do not reset merely at capture end, pointer movement or UI focus.
- Preserve pending/failed/cancelled turn state without replaying successful
  side effects automatically. Stream to existing presentation surfaces without
  treating tentative output as accepted delivery.

## Required acceptance cases

- Max receives a request to prepare `git add` from clipboard: it calls the
  clipboard tool, uses exactly returned paths and never calls a process or Git
  mutation tool merely to prepare the command.
- A following correction references the prior answer and changes only the
  requested aspect; both turns use the same consultation identity/history.
- Two distinct consultations do not exchange messages, images or tool results.
- Correction, Smart and Off cannot invoke Max tools, even when transcript text
  asks them to or contains a tool-shaped payload.
- Overlay formatting, terminal composer formatting and live occurrence
  formatting exercise the same Max execution contract.
- Settings/provider changes retain history while honoring the new sealed lane;
  corrupt persisted history is an explicit error, not a silent empty session.
- Tool refusal, cancelled capture, stale revision and provider failure neither
  falsely claim successful execution nor replace a successor document.
- Real installed proof includes a clipboard read and two connected spoken
  turns, with tool receipts and the actual pasted output inspected.

## Structural checkpoints and remaining assembly

`e07ab4bf88f01429a6295f897093a92d99820f68` adds explicit provider-lane
admission and authored provider-selection tests. It does not run Max turns.

The next source step adds `AgentSession::replace_provider`: the existing session
retains messages, local identity, tool registry and approval handler while the
incoming provider's response chain and session response id are cleared. An
authored test checks that the next request contains the prior conversation and
the new correction. It does not prove persistence, clipboard execution or UI
integration. No production caller uses this operation yet.

The live formatter uses `FuturesOrdered` jobs per occurrence. Ordered completion
does not prevent concurrent provider requests or tool effects. Assembly must
therefore introduce explicit turn admission before Max execution, not merely
replace the function inside each existing job with an Agent call. Occurrence
identity remains acoustic; consultation identity must not be inferred from it.

The next structural step introduces `core/agent/consultation.rs`: one retained
FIFO owner drives the existing AgentSession and drains its bounded UI channel.
Dropping a reply receiver does not abort accepted work. Duplicate turn ids are
refused in-process; a failed turn stops further execution pending recovery.
Completed history goes through ThreadDeliveryGateway with an explicit
MaxConsultation origin, and Done is emitted only after that receipt. Provider
replacement happens in queue order. Non-Max admission is rejected.

Authored (not executed) tests cover two queued turns with history, dropping the
first reply receiver, completed history in the canonical store, duplicate-id
refusal, failed-turn blocking and Off/Correction/Smart exclusion. These use a
synthetic provider, not real clipboard or tool execution.

The following step moves pending/completed turn identities into a journal owned
by ThreadStore, alongside (not duplicating) message history. A kernel file lock
permits one live owner per consultation. The pending marker is written and synced
before provider execution. Completion is written only after the canonical
history receipt. Reopen refuses unresolved pending state or malformed JSON;
completed ids cannot be replayed after reopen. A crash between history delivery
and journal completion conservatively requires recovery. Thread atomic writes
now sync data before rename and the parent directory after rename, so the
completion journal cannot intentionally outrun an unsynced thread save.

Authored journal tests simulate owner drop/reopen, unfinished-turn refusal,
completed-id replay refusal, concurrent ownership, malformed state and path
escape. They have not run and are not actual process-kill/power-loss evidence.

History restore now occurs inside consultation start after acquiring its lease.
A pre-populated session is rejected, as are missing completed history and a
thread identity/mode mismatch. `ThreadMessage::try_to_message` validates roles
and content shape before using the existing projection; unknown roles, malformed
tool results and omitted image data cannot silently become user text. Authored
tests cover those refusals and valid Max history restore; still not executed.
Image asset existence needs further validation. Tool-call/result pairing is now
checked at consultation restore: each assistant invocation must have exactly one
following user-role result before conversation resumes. Duplicate invocation ids,
orphan or repeated results, missing results, wrong roles and nested tool control
blocks are refused. Result payloads cannot be mixed with new instruction text.
Both registered providers currently produce this same role/block shape.
An authored gateway test persists valid multi-tool and failed-tool exchanges plus
malformed variants, then checks admission and byte-for-byte history preservation.
It has not run; this is structural context validation, not live tool proof.

`app/agent/max_consultation.rs` now constructs the real formatting provider and
AgentSession using the host-supplied permission-configured registry/approval
broker. Each admitted turn supplies its sealed model, prompt and provider;
server-side chains reset while local history remains. There is no cached
last-enqueued generation that could drift after a duplicate turn is refused.
The built-in Max prompt now follows instructions, distinguishes command
preparation from execution, and treats tool outputs as data. The authored local
HTTP fixture now exercises this host through the queue and durable answer,
not just its provider factory. It has not run; it uses an empty test registry.

The shared policy formatter now accepts an explicit FormattingConsultation
capability. Max routes to that capability before the text-only length/repetition
and retry filters; the app Max host implements it. Missing capability returns
Failed rather than running the one-shot prompt and pretending to be an Agent.
Off/Correction/Smart cannot invoke the capability. An authored core test checks
short correction admission, unchanged legitimate answers and those policy
boundaries. The controller now retains one Max host across captures and supplies
it to explicit overlay formatting and terminal composer formatting. Turn ids
come from session id plus source revision, not transcript text. Both routes
still pass their answer through existing presentation revision admission.
Chat, voice Agent and this Max host now use one `configured_registry` factory
with the same persisted permissions, grant merge and decision-time hot reload.

The retained owner now acquires the existing agent-turn install lease before
writing its pending marker or invoking a provider. It holds that lease through
history/journal settlement. A lock refusal returns without admitting effects;
there is no automatic retry. The controller supplies the canonical lease path,
while authored tests supply a temp path. An authored test holds the installer
lock and checks zero provider calls, then explicit resubmission after release.
This is still unexecuted; runtime install exclusion is not yet proven.

The controller now gets its selected id from the same ThreadStore gateway.
Selection is atomically synced under an exclusive selection lock; only first
use mints an id. Reopening the store retains it, including when that
consultation has unresolved work. Corrupt selection/path identity is refused
without overwriting it. Authored tests cover store reopen, unresolved work and
corrupt selection preservation; no installed restart has been exercised.
Explicit selection/reset UI is not wired. Approval requests without a host broker
remain refused. Its event callback currently reports errors only; streaming Max
UI is not connected. Live occurrence formatting still passes None, deliberately
not executing one tool turn per acoustic fragment. This remains unassembled and
must not be installed as a completed Max cut.

Outstanding: explicit consultation selection/reset UI and recovery actions; durable
queued-but-not-started instructions; explicit
reconciliation of unresolved turns (never implicit replay); cancellation;
live turn admission; permission UI and streaming presentation/delivery wiring.
Two controller call sites are connected in source, but the full product path is
not verified. The journal protects before-effects admission, not all acknowledged
in-memory queue entries; do not claim complete crash recovery yet.

The owner now exposes `close_if_idle` for the future explicit reset path.
Admission and close share a lock and a pending-work count. Close refuses queued
or executing work, closes every cloned admission handle, and acknowledges only
after releasing the session and journal lease. Pending-work guards also release
their count if queue admission fails or the owner drops queued work. Closing a
failed owner leaves its unresolved journal marker intact. Extended authored
tests cover active refusal, stale handles, lease release and unresolved-state
preservation. No UI reset is connected and these tests remain unexecuted.

The explicit new-consultation backend now runs through controller and the
CodescribeHotkeys bridge. It refuses non-idle capture/processing, awaits owner
close, then compares the expected persisted selection under its selection lock.
Another live owner or a stale selection refuses reset. A new id is persisted
without altering the previous thread or journal, including unresolved markers.
An authored test checks owner refusal, preserved journal bytes and stale-reset
refusal. The bridge API is authored but Swift bindings are not regenerated under
W1, and no user-facing button invokes it yet. Cancellation during owner close
leaves the controller slot empty so a later attempt reopens durable selection
instead of retaining a closed handle.

Live investigation: `schedule_formatter_after_terminal_label` submits exactly
one PCM occurrence when its last earlier observer returns. This is not the end
of a logical instruction. `EpochGate::Sleep` closes an Apple engine epoch after
Silero silence and flushes Layer 1 coalescing, but does not wait for those
refinements to finish. It is therefore not a ready-to-execute semantic verdict.
Live admission must retain the member occurrences and wait for their observation
frontiers; it must not substitute the first occurrence or a text-derived id.
The current presentation formatter revision API is terminal-document-only, so
the live result also needs an explicit admitted revision boundary.

This investigation exposed an independent shared Agent bug: EOF or a dirty
terminal without a following Error could reach Done and tool execution.
AgentSession now requires a clean provider terminal before committing answer
history or executing gathered tools. Any dirty terminal remains rejecting even
if another clean terminal follows; provider and session chain ids are cleared
on rejection. An authored test covers absent/dirty/mixed/clean terminals with
an execution counter and a clean positive control. It has not run.

The Creator panel now exposes New consultation only for enabled Max. Its
existing SettingsViewModel awaits the SettingsEngine/Hotkeys backend, rejects
duplicate requests while pending, and reports success only after acknowledgement.
Errors remain visible without claiming reset. UI does not own consultation
identity or delete history. Authored Swift tests cover pending re-entry, backend
refusal and non-Max exclusion; they have not run. Swift binding generation and
rendered interaction proof remain deferred to W2/W3. The earlier missing-button
notes above describe prior checkpoints, not the current source state.

The existing chat ApprovalBroker now lives in core/agent/approval.rs and the
bridge imports it; its old implementation was removed rather than copied as
a second broker. This makes the same permission suspension mechanism available
to controller-owned Max. Max still has no host approval/event UI connected.
During relocation, source inspection found that duplicate pending keys replaced
old requests and their guards could remove a successor; dropping an unpolled
future also left its card registered. Duplicate registration now refuses without
replacement, guards are captured before polling, and each registration has a
token so stale cleanup cannot evict another request. Three authored tests cover
these cases; existing bridge exact-key/cancel tests remain consumers of the
relocated broker. None have run.

Delivery inspection found that the existing app broadcast discards lagged events
and events received without a Swift listener. It cannot be the sole memory of
an approval card. ApprovalBroker now exposes an exact-thread pending snapshot
from its existing pending map, retaining the original request preview. Chat's
bridge exposes that snapshot and shares one request projection with callbacks.
An authored test covers thread isolation, repeated reads, rejection and future
drop. Reading a snapshot never approves or replays work. Max's host/UI wiring
must use notifications plus this authoritative state, not notification-only
approval delivery. Swift consumption and generated bindings remain outstanding;
this does not yet prove recovery in the app.

The controller now owns an ApprovalBroker instance for its retained Max session
and passes its handler into AgentSession. Hotkeys exposes pending Max cards and
exact-key resolution through the same FFI request projection. Snapshot reads do
not create a controller/session or recorder. Resolution additionally requires
the selected consultation id, so a card from another conversation is refused.
Capture reset still refuses queued/executing work, including approval waits.
The Creator settings panel now consumes the snapshot on appearance and explicit
refresh, reusing ToolApprovalCard for Deny/Allow once/Always allow. The existing
SettingsViewModel serializes refresh/verdict actions, preserves backend identity,
reloads after verdict and disables stale cards after read failure. Authored Swift
tests cover exact forwarding, removing settled cards, read failure and stale-key
refusal; none have run. This is a manual recovery surface, not the required
immediate live notification/overlay path. An unanswered request remains bounded
by AgentSession's existing approval timeout and denies execution; do not install
this intermediate state. Automatic notification/recovery, executable integration
tests and generated bindings are still owed after W2.

Automatic permission display is now wired in source. ApprovalBroker emits a
coalescing watch invalidation on pending-state changes, not token broadcast
events. A late subscriber is initially marked changed; listener registration
also requests a fresh snapshot. The controller-created forwarder terminates
when its broker closes. Swift's app-lifetime listener refreshes a Max permission
projection and passively reveals the existing Agent window when cards or a
read error exist, without focusing the composer or changing chat selection.
The same cards render in a separately labelled Max area and settings recovery.
Notifications arriving during a snapshot/verdict request schedule another read
rather than disappearing behind the busy guard. Authored tests cover late and
coalesced notifications, re-entrant snapshot invalidation and separating Max
refresh from chat summon. These tests remain unrun; first generated-binding,
compile and real permission-window proof are still outstanding.

## Live assembly decision record (after dacb32089)

This is Roman's source-derived implementation design, not a new Founder
decision and not a W2 closure receipt. The live path is still absent.

### Evidence changing the next implementation step

- apple_stream_transcription_session receives SessionConfig with the capture
  identity, immutable settings and shared AcousticLedger, but no host Max
  capability. Passing a callback only to controller terminal formatting cannot
  reach this function.
- schedule_formatter_after_terminal_label acquires a permit for exactly one
  occurrence before the last earlier observer returns. Its FormatterRequest
  and FormatterCompletion both carry one occurrence. Replacing None with Max
  in that call would execute one tool conversation per fragment.
- EpochDecision::Sleep finishes Apple, seals its open partial and flushes
  Layer 1 coalescing. It does not wait for Whisper completion. Sleep is a
  candidate acoustic boundary, not permission to execute.
- core/conversation/turns.rs::TurnManager uses wall-clock speech/silence
  thresholds. It has no occurrence membership, ledger receipts or semantic
  completeness input. Instantiating it beside the existing EpochGate would
  add another timing owner without resolving instruction readiness.
- PresentationEmitter::terminal_revision_source and apply_formatter_revision
  are a terminal-document corridor. record_manual_document_revision expressly
  does not distribute generated words back across PCM labels. Using either as
  a live per-occurrence formatter would misrepresent the provenance.

### Required assembly order

1. Carry the selected host execution capability through the existing recorder
   session configuration. It is optional for non-Max takes and sealed once per
   capture; the core must not instantiate app tools or reload settings.
2. At an existing acoustic boundary, retain the capture sample interval and
   its member occurrences. Preserve five distinct equal labels as five PCM
   members. This is instruction grouping, never new acoustic identity.
3. Wait for every member's real observation frontier and seal receipt, including
   Whisper and required text recovery. Uncovered speech, missing receipts,
   dropped refinement or a crossing occurrence prevents readiness; a timeout
   must not convert these into success.
4. Distinguish acoustic readiness from a complete instruction. No inspected
   component currently supplies a semantic-completeness verdict. The concrete
   semantic admission mechanism remains an open architectural obligation;
   punctuation, elapsed silence and engine closure alone cannot close it.
5. Admit one ready instruction with an explicit capture/boundary identity to
   the retained FIFO. Queue ownership precedes provider/tool execution.
   Resuming speech while readiness is pending must extend or invalidate that
   candidate before effects, without replaying an already admitted turn.
6. Render tentative Agent events with consultation AND turn identity, separate
   from committed ASR text. A result is a revision of the admitted instruction
   group, not a label assigned to its first occurrence. The existing reducer
   needs a scoped admission operation that preserves later spoken content.
   Stale or cancelled destinations must not overwrite the successor document.
7. On stop, settle pending candidates and acknowledged turns explicitly. Neither
   abandon acknowledged instructions nor secretly rerun the entire take through
   a second Max turn. Persistence and cancellation obligations still apply.

### Falsifiers required before W2 closure

Author cross-component cases for: five identical physical words in one
instruction; one instruction split across several Apple results; silence while
Whisper is pending; resumed speech before readiness; queue pressure; a dropped
UI consumer after tool execution; a tool result arriving after the next
instruction; capture cancellation; and stop while a permission is outstanding.
Each must assert tool execution count and identity, retained conversation
history, ledger membership and final delivered document together. Isolated
queue tests cannot prove these properties. Real-audio proof remains W4.

The next write must address capability transport and grouped admission, not
change the existing per-occurrence Max call to execute tools prematurely.
Any required change outside the closed W1 source domain needs an explicit
domain update before editing; this record does not silently expand it.

The selected Max executor now travels through StreamingRecorder into
SessionConfig for live takes. Both hold and toggle bind the controller's same
retained host; explicit raw and assistive routing do not bind it. The recorder
additionally excludes disabled formatting, non-Max policies and SingleTurn.
Rebinding session authority clears the old handle; callback cleanup also clears
it. Offline constructors carry None. Apple receives but does not yet invoke
the handle: grouped/semantic admission and scoped result delivery are still
missing, so wiring the old per-occurrence call remains forbidden.
An authored transport test checks the full intent/enabled/policy matrix,
Arc identity and missing capability without opening audio or running an Agent.
It has not run. Max reset now refuses a scheduled unfinished Hold start, because
changing selection between scheduling and capture would otherwise strand the
captured handle on the previous conversation. A Max initialization failure keeps
capture available without a tool executor and logs the refusal; no successful
Agent execution is claimed from that path.

SealedConsultationInput now reads known qualified/committed members of an
explicit capture interval from the existing ledger. It retains each occurrence
and its real seal id, ordered labels and the capture interval; repeated equal
labels remain distinct members. Missing qualification, open observation frontier,
text-recovery debt, absent seal or absent label returns not-yet-available.
Crossing/overlapping members and invalid intervals are refused instead of clipped.
An authored synthetic five-Iwo test checks exact member/receipt preservation;
another covers pending Whisper, clipped intervals and foreign capture identity.
Neither test has run and neither is physical-audio proof.
This reader is not yet a production boundary consumer. Since ac1fa9982 it
requires the existing ledger's authenticated coverage of the candidate interval;
it still does not certify semantic completeness or grant execution permission.
The worker-to-host handoff remains open. The capture-local input queue added in
51b8e3b30 retains pending boundaries across failed readiness reads, and 5744f0277
adds measured-silence advancement. Neither is wired to production yet; neither
is a durable queued-instruction journal.

## W1 structural re-entry: scoped answer presentation

Roman's implementation decision at source baseline 5744f0277. This explicitly
extends the closed source domain before further writes; it is not a new Founder
requirement, W2 closure, or permission to execute gates.

Additional domain: the existing AcousticLedger's presentation-receipt API,
TranscriptReducer/PresentationEmitter scoped group revision admission,
TranscriptRevision publication authentication, and direct TranscriptBus/
projection consumers and tests. Acoustic qualification, PCM identity, VAD,
seal predicates and observation ownership are NOT included in this extension.

Evidence: authenticated_revision_occurrences rejects a nonterminal document.
record_manual_document_revision binds the complete document to its full source
set. IncrementalShapingReceipt explicitly binds one occurrence and Light+ left
context. Assigning an arbitrary Agent answer to either receipt would assert a
different provenance than the computation actually has. An untracked UI text
override would also bypass authenticates_publication and the single reducer.

Required group corridor:

1. Preserve the admitted consultation id, turn id, capture identity, ordered
   source member identities, source labels and their seal receipts. No generated
   word receives fabricated PCM alignment.
2. Authenticate a group presentation receipt in the existing ledger, then let
   the existing reducer mint the document revision. This is a presentation
   receipt, never an observation, new occurrence or terminal seal.
3. Store group presentation once for the exact contiguous source member set.
   Subsequent spoken entries remain outside that replacement. The immutable
   revision must carry enough group evidence for publication authentication;
   a generated answer must not appear only in an unaudited rendered_text field.
4. Admit a late answer only if its exact member set/labels/seals still matches.
   A global revision mismatch caused solely by new suffix speech must not lose
   an already completed tool answer. Conflicting group edits, cancellation or a
   different destination must refuse replacement without retrying tool effects.
5. Light+ shapes cannot concurrently own presentation of the same source group.
   Whole-document user edits remain explicit and invalidate older pending group
   replacement rights. Do not erase persisted Agent history on display refusal.
6. Tentative Agent events stay separately identified and never publish committed
   transcript revisions before final answer/history settlement.

Required falsifiers: answer after a new open suffix, answer after a later sealed
group, missing/relabelled member, overlapping group, duplicated answer, changed
session/epoch/turn, explicit document edit, tampered revision payload and Bus
publication. Assert exact member preservation and untouched suffix as well as
the displayed answer. Keep the existing real clipboard/two-turn delivery proof.

This re-entry removes a concrete assembly obstacle; it does not assert that the
new corridor exists. Next implementation must connect this evidence path before
allowing live Max tool execution. Current installed behavior is unchanged.

These checkpoints are structural W1 work. Checkpoint hooks are bypassed in full:
trailing-whitespace, end-of-file-fixer, check-merge-conflict, mixed-line-ending,
detect-private-key (security), cargo-check, cargo-fmt, prettier and
commit-msg-provenance. All remain verification obligations. Only source review
and `git diff --check` have been performed for this step. No executable gate,
installation or integration is claimed; those require W2 structural closure
against the exact assembled SHA first.

## Group execution handoff after 3762d829a

The host FormattingAgent capability now has synchronous grouped FIFO admission,
separate from waiting for its answer. Max uses the same request preparation and
retained ConsultationRuntime as terminal requests. The group key uses capture
session/epoch/sample interval, not recognized text or document revision.
Admission checks exact source text and group key before enqueueing. The pending
handle retains the immutable input, and completion checks both the turn key and
the selected history id before exposing the answer with its durable receipt.
Text-only implementations explicitly refuse this operation; they cannot turn a
String result into group completion evidence.

Authored, unexecuted tests cover two ordered groups, preserved equal-word PCM
members, durable two-turn history, mismatched input/turn refusal, duplicate-key
refusal without another provider call, and foreign reply/history rejection.
These are synthetic provider/ledger fixtures, not clipboard, audio or tool proof.
No production capture caller invokes this new operation yet. Boundary collection,
semantic admission, queue acknowledgement, grouped presentation and stop/cancel
settlement still need connection. Accepted but not started entries are still
in memory; this checkpoint does not claim durable queue recovery. Overlapping
candidate intervals must be excluded by the capture queue before admission.

W1 remains open. Only source review and git diff --check are used; all executable
gates, generated bindings, installation and integration remain deferred. A local
checkpoint bypasses the same complete hook entrypoints listed above, including
detect-private-key security checking; it does not certify security or correctness.

The public emitter group corridor now requires ConsultationGroupAnswer from
the retained FIFO instead of caller-authored text/member parameters. It obtains
consultation/turn identity from that correlated durable result; the reducer still
owns current-member validation and revision numbers. A closed delivery worker
refuses before mutating ledger presentation. This does not make publication and
downstream delivery transactional if a worker fails after that admission check.

The existing local HTTP Agent-lane fixture now includes a grouped Max turn after
the first conversation turn, projects its sealed sources, adds later open speech,
and applies the actual completed answer through emitter, Bus and delivery buffer.
It asserts history count, source/group identity, untouched suffix, duplicate
presentation refusal and closed-worker refusal without consuming the result.
The HTTP request count remains exact. This cross-component test is authored but
unexecuted under W1; its acoustic evidence is synthetic and its tool registry
empty. It is not real microphone/clipboard proof. Production capture admission,
semantic completion, durable pending input and stop/cancel settlement remain open.

Capture queue acknowledgement now takes PendingConsultationGroup, not a sealed
ledger reading. Production code can obtain that handle only from successful
retained-owner group admission. The capture queue still checks exact session,
epoch and current interval; stale or foreign handles do not advance it. A caller
must retain an accepted handle even if capture acknowledgement is refused, since
that refusal cannot revoke already accepted effects.

The authored retained-runtime group test now advances the actual capture queue
with both accepted handles, checks that refused source/turn admission leaves its
front unchanged, and rejects repeated acknowledgement before awaiting completed
history. The isolated capture-order unit test explicitly uses constructed test
handles and does not claim runtime admission proof. Neither test has run. This
typed handoff closes the accidental read-as-ack API, not durable queue recovery
or production microphone wiring; W1 and the full verification debt remain open.

## Durable admission falsifier at a210cea07

Source inspection confirms a lost-input window: ConsultationRuntime::enqueue
increments an in-memory count and sends through mpsc, while run_owner calls
ConsultationJournal::begin only after dequeuing and obtaining its install lease.
AdmissionState stores completed ids and one pending id, not waiting input.
Thus successful queue acceptance currently cannot prove restart survival.

An authored test now blocks the first provider call, enqueues a second instruction,
and immediately reads the existing consultation journal. It requires a queued
entry with turn_id and canonical user Message input before that second provider
call starts. After completion it requires removal from queued state while the
completed id remains. This is an unexecuted falsifier: inspected current source
does not implement the asserted queued field. It is not a green regression test.

Next structural implementation must persist acceptance in this same journal
before returning the admission handle. Completed messages remain owned by
ThreadStore; queued input is unfinished work, not a second conversation history.
Reserve channel capacity before durable acceptance, serialize admission order,
and preserve journal ownership across that write without stale runtime handles
keeping its kernel lease alive after close. Installer refusal before effects
must explicitly settle unstarted admission, while an uncertain write or executed
turn remains recovery-required. Reopen must never silently replay uncertain tools.
Group source receipts and multimodal input must also survive; do not store auth
credentials or replacement-provider objects as recovery payloads. This schema
and lifecycle work remains unfinished; no W2 closure is asserted.

### Durable admission source implementation after 4678c6043

Admission now reserves an mpsc slot, holds the shared admission ordering lock,
and syncs QueuedInstruction through the existing ConsultationJournal before
sending and returning a receiver. Queued records retain canonical user Message
content (including image bytes), provider name, request knobs and grouped source
receipts. They contain no provider object or extracted authentication material.
The in-flight record stays queued while pending names its potentially executed
turn; completion removes that record only after canonical history delivery.

The execution owner alone holds a strong journal reference between operations;
runtime handles retain a weak reference, so closed/stale clones do not hold its
kernel lease. Admission and execution share the same journal lock. Pending-count
cleanup uses an atomic counter so a closed mpsc receiver cannot synchronously
re-enter the admission mutex while dropping a reserved send. Queue/channel order
is serialized across durable acceptance. Duplicate ids now refuse synchronously.

Reopen refuses any retained queued or pending work without changing the file or
silently replaying effects. Installer refusal before begin explicitly removes
only the unstarted front; failure to persist that removal blocks further work.
Any uncertain journal write prevents later begin/accept/discard in this owner.
The runtime's already-unsettled path retains queued inputs for recovery rather
than executing or deleting them. Recovery UI/explicit reconciliation is still
missing, and these records alone do not authorize automatic replay.

Authored tests now cover persisted multimodal waiting input/model, grouped member
receipts, retained source on owner drop, FIFO begin, refusal to discard a started
turn, explicit resubmission after pre-execution refusal, and uncertain-write
non-overwrite. Existing duplicate tests now assert the earlier synchronous refusal
while retaining provider-call counts. None have executed under W1. The previous
falsifier has an implementation to assess, not a passing receipt. Real process
crash, disk failure, performance of synchronous fsync, full gates, production
capture wiring and installed proof remain outstanding.

The live recovery gate now belongs to the shared journal rather than a separate
run_owner-local unsettled flag. Both admission and execution consult it. A
provider/tool/history failure marks that gate before the failing reply is sent;
later input is refused before acknowledgement or a journal rewrite. Already
accepted entries remain retained and receive explicit recovery-required results.
Uncertain writes use this same gate, preserving the first failure reason.
Persisted pending/queued records still govern reopen; the live reason is not a
new history or permission source. The extended unexecuted failure test asserts
two retained entries, the pending first id, synchronous third-input refusal and
byte-identical persisted state after refusal. Closing still releases ownership
without clearing those records. Recovery actions and full verification remain open.

ThreadDeliveryGateway now exposes read-only inspect_consultation. It reads one
atomically published journal snapshot without creating consultation directories,
acquiring/releasing the execution lease, selecting a new conversation or invoking
a provider. The projection contains retained user Message inputs, provider names
and the pending turn id; request prompts, provider objects and group metadata are
not promoted into instructions or UI action authority. Pending can mean active
or interrupted execution: this read does not claim liveness or authorize replay.

Inspection refuses malformed JSON, path escape, repeated/conflicting identities,
non-user roles and tool-control blocks in retained source input. Missing state
returns None, not a newly created empty consultation. Authored tests inspect
while another owner holds the lease and after its drop, compare unchanged file
bytes, preserve recovery refusal and cover malformed/control-shaped inputs.
These tests are unexecuted under W1. No bridge/UI consumes this read yet; it is
the existing store's inspection boundary for that next connection, not a finished
recovery surface or a new history owner.

The next checkpoint connects selected-consultation inspection to CodescribeHotkeys
without constructing RecordingController or Max. Selection inspection reads the
existing atomic selection file and journal; first use returns None instead of
minting an identity. A selected identity with no journal returns an empty snapshot
for that identity without creating a journal. Corrupt selection remains an error.
The returned identity is the one read even if selection changes concurrently.
The bridge projects source text blocks, attachment count and provider name, not
image bytes, request options or permission to replay potentially executed work.
ThreadStore initialization can create its root/blobs directories; consultation
inspection itself creates neither selection nor journal directories or files.

Authored, unexecuted tests cover missing selection, active and interrupted work,
unchanged selection/journal bytes, corrupt and escaping selection, and bridge
projection of exact source text with attachment count. Swift consumption,
generated bindings, recovery decisions and real crash proof remain outstanding.

## W1 structural re-entry: agent installation discoverability

Founder message on 2026-09-14 explicitly requires that the agent skill be
available from configuration without a cloned repository. This adds the existing
AgentBridgeInstaller, Creator settings entry and direct tests to this worktree's
closed domain. No remote installation, configuration edit or follower restart
on Dragon is authorized by this structural step. The same compile embargo holds.

Read-only host comparison found local app 1050/b91e9a947 versus Dragon
1041/fed519f7d (both version 0.15.1). Dragon's installed Codex/Claude skill says
0.4.0 while its app payload already says 0.5.0. Dragon has no bridge receipt or
managed marker in the inspected Codex skill folder; its live Leon process uses
../../codescribe/scripts/bus-demux.py, not the installed runtime helper.
These are installation drift receipts, not proof of an ASR-quality cause.
Persisted settings also differ (local Max/Luna/Smart final pass; Dragon
Correction/Terra/Off final pass, explicit EarPods input). No effective runtime
snapshot or matched-audio comparison has yet established causality.

The existing installer already sources its payload from the app bundle. Creator
will expose that installer without silently installing on visit or overwriting
unowned skill folders. Explicit adoption with recoverable backup remains a
separate unfinished part of this requirement, as does follower wakeup proof.

Creator now has per-client Install/Update buttons and passive status refresh.
SettingsViewModel injects the existing AgentBridgeInstalling service. A click
re-reads installed clients and unions the requested client, so updating Codex
does not implicitly deselect Claude installed since the last UI refresh.
Success explains skill reload and /codescribe without claiming a listener was
attached. Failure displays the installer error and never reports success.
Authored tests use an isolated real bundle payload and temporary home through
the settings model: passive inspection leaves home absent, explicit installation
preserves the other client, repeated update retains both, and an unowned skill
remains byte-identical with visible error and no receipt. Tests are unexecuted.
The SwiftUI skill guided reuse of existing model/service ownership; no alternate
installer or settings authority was added. This is source-only, not installed
or rendered proof. Manual-copy adoption and live notification delivery remain
unfinished, and the full history census goal is still open.

Manual-copy adoption is now authored through the existing installer transaction.
Normal install/update still refuses an unowned folder. Creator exposes a separate
confirmation after an installation error. Confirming permits one client's manual
folder with a regular SKILL.md and no managed marker; redirected directories and
managed folders refuse. Other managed clients remain selected. The transaction
retains the renamed original after success and records its path in the receipt;
subsequent managed updates preserve both the backup and its receipt reference.
The UI shows the backup path without claiming that voice delivery was verified.

Authored, unexecuted tests cover original content plus extra files surviving
adoption and later update, the other client remaining installed, refusal of
ordinary overwrite and repeated manual adoption, receipt-write failure restoring
the original directory, and symlink refusal. Existing rollback is best-effort,
not a crash-recovery protocol: process death between renames, concurrent external
folder mutation and rollback I/O failure still require verification/hardening.
No real home folder was adopted, no remote state changed, and no app was built
or installed in this step. W2 gates and actual Creator interaction remain owed.

The shared installer now acquires a non-blocking kernel flock on the persistent
installation.lock before reading prior receipt ownership or staging/replacing
payloads. This serializes participating processes using the same bridge root;
the lock is not unlinked and the descriptor closes on success/error or process
death. Symlink/non-regular lock targets refuse. Manual adoption unions the prior
receipt's clients after acquiring ownership rather than relying only on the
earlier UI snapshot. This is writer exclusion, not interrupted-transaction recovery.

Authored unexecuted tests hold the same kernel lock to require refusal before
payload/receipt creation, check release after both failed and successful install,
preserve the lock inode's path, and refuse a symlinked lock without changing its
target. The installation crash/rollback obligations above remain open. No gates,
home-directory installation or Dragon mutation ran under this checkpoint.

Rollback now returns explicit recovery failures rather than discarding removal
and restoration errors. A failed stage move records its prior rename in the same
rollback sequence. Failed replacement removal preserves the original backup and
does not attempt to restore onto the remaining destination. Failed restoration,
including a missing backup, reports the exact backup and destination paths.
The outer installation error states rollback is incomplete instead of presenting
the original failure as if restoration had succeeded.

An authored, unexecuted test injects removal and restoration failures through a
test-only FileManager subclass, uses a final receipt-write failure to enter
rollback, and asserts original bytes remain in the named backup plus both paths
are surfaced. This is controlled fault-injection source, not a filesystem-crash
receipt. Cleanup of generated stages remains best-effort; durable recovery of a
process-killed transaction and full executable verification are still outstanding.

## W1 structural re-entry: diagnostic receipt

The newly read Founder history and current Dragon comparison require useful
Copy debug info. Scope includes App.swift's existing callback, a pure report
renderer, the configuration bridge's canonical settings-path accessor and direct
tests. Configuration reporting must not masquerade as capture execution proof.
Use the existing controller's last-serving verdict; do not invent another runtime
owner. Do not include credentials, raw endpoint URLs or transcript content.
Generated bindings and executable tests remain deferred until W2 closure.

The callback now uses AppBuildInfo and a pure codescribeDebugInfo renderer.
It shows commit/build time, configured ASR/engine/input and both LLM lanes, the
canonical UserSettings settings path (via the bridge), app-data/notes paths,
and the separate existing last-serving verdict. Missing execution evidence is
explicitly not-yet-observed, never inferred from useLocalStt. It warns that the
newly loaded configuration is not proof of the active capture snapshot.
Raw STT URLs and transcript templates are not included. Local paths and labels
remain visible with a review-before-sharing notice.

Authored unexecuted Swift tests set Apple configuration and a Whisper serving
verdict, verify both remain distinct, check build/path/lane fields, and ensure
credential-bearing endpoint fixtures and template content are absent. Missing
serving data stays explicitly unknown. The existing settings loader still runs
on report generation; failure-safe diagnostics for a broken loader and a receipt
of the exact active capture generation are not yet implemented. New bridge
bindings, full gates and actual copied report inspection remain owed.
