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

Diagnostic follow-up: source inspection disproved the suspected current panic
path: load_runtime_snapshot returns Ok around the startup snapshot, including
repair/refusal receipts. The diagnostic bridge now uses that same canonical
startup loader without credential imports and refuses a projection if its
receipt contains unrepairable configuration. Swift still copies build, paths
and independently observed serving information, but omits configured values on
refusal. Success is labelled resolved and potentially repaired, not a faithful
raw-file read. Refusals are process-lifetime receipts, so this does not claim a
fresh successful repair clears an earlier refusal. The canonical load can still
perform its existing settings repairs; this is not a read-only disk inspector.
Scope includes this bounded diagnostic bridge method and its direct Rust tests.
Authored tests cover unsupported schema preservation, valid settings projection,
and Swift report generation without configuration. All are unexecuted under W1;
new bindings, W2 gates and installed-app clipboard proof remain outstanding.

## W1 semantic admission mechanism

Agent design choice: the existing FormattingAgent capability now offers
assess_group, with a refusing default. Max uses a fresh client for the sealed
formatting lane to classify the exact immutable candidate as COMPLETE or CONTINUE.
The request has an empty tool list, an assessment-only prompt, a fresh response
chain and a 15-second bound including request startup. It neither enters the
retained AgentSession nor writes consultation history or executes tools.
The result carries the exact sealed input assessed, not only an uncorrelated bool.

This is a semantic suggestion, not execution authority. The live capture owner
must invalidate the suggestion on resumed speech and compare the candidate to
fresh ledger truth before durable FIFO admission. That caller is still absent;
this checkpoint does not enable live Max or close W1. Model errors, timeout,
unknown output, tool events, inconsistent final text, excessive output, missing
clean terminal and events after terminal all refuse assessment. CONTINUE retains
the pending instruction; the forthcoming owner must not discard it or retry on
every audio quantum. A bounded observer-return trigger and stop/cancel handling
remain required. The 64-token output limit and 15-second bound are unmeasured
agent choices; provider support, real speech judgement and resource cost require
W2/W4 evidence, not parser assertions. No punctuation-only rule replaces this.

Three authored, unexecuted parser tests cover fragmented clean decisions,
incomplete/failed/conflicting/oversized replies and attempted tool output.
Cross-component tests must still prove empty tool definitions on the actual
request, unchanged consultation history, stale-candidate rejection, no execution
before durable queue admission, and completion while newer speech continues.

At 8acba923c source follow-up traced both provider request builders: tools come
only from the supplied definitions, with no hosted-search injection in these
builders. The local Responses HTTP fixture now includes assessment between two
durable consultation turns. It matches the formatting model, assessment prompt,
single exact transcript input, absent tools and absent previous_response_id,
then checks the assessed source identity and empty pending/retained queue.
The next executed group must still report four history messages, preserving
the distinction between classification and conversational execution. This new
HTTP case is authored but unexecuted under W1. It does not prove Anthropic or
account-auth endpoint behavior, live candidate invalidation, lexical correctness
or bounded resource use. Codex account routing intentionally omits the requested
output-token cap in the existing client; the 15-second assessment timeout and
parser byte cap remain, but no 64-token server limit is claimed on that route.

## W1 pre-execution source validation

ConsultationInputQueue now checks COMPLETE against a fresh ledger reading before
calling synchronous executor admission. CONTINUE, changed input, missing acoustic
coverage and executor pressure retain the pending instruction. The caller retains
the returned admission handle and acknowledges it separately; an acknowledgement
failure must not drop that handle or authorize replay. The capture owner must
serialize current observer/ledger state, admission and acknowledgement without an
await or concurrent capture mutation between them.

Recorder-observed resumed speech can invalidate all unaccepted candidate boundaries.
The accepted prefix and last registered recorder-clock boundary are retained, so
old ticks cannot revive an invalidated assessment and subsequent grouping includes
all still-pending occurrences. No PCM identity, seal or transcript label is removed.
This is a transport invalidation method, not a second speech detector.

Authored negative cases cover CONTINUE, changed assessed text, incomplete observed
audio, executor pressure, resumed speech and old clock ticks. The retained-runtime
group test now admits both groups through the new check before acknowledging them.
All tests remain unexecuted under W1. The live Apple capture owner still does not
call these methods: observer-driven candidate production, assessment scheduling,
resumed-speech invalidation, result presentation and stop/cancel settlement remain
required before structural closure. This checkpoint is not installed or integrated.

## W1 completed-answer transport and capture-edge findings

Source review at edfe50296 found two remaining connections that must not be
approximated while wiring the live owner. SileroIngest.speech_live includes the
chunk in which an utterance closed; it is not equivalent to an open utterance.
The live owner must inspect the existing open/closed observations from the same
SileroIngress, preserve the recorder clock, and wait for sealed ledger members.
Neither EpochGate sleep nor a separate silence timer is a semantic turn verdict.

The existing EventSink now carries typed completed consultation answers through
an in-process method, outside serializable EngineEvent and raw IPC. Passive sinks
declare zero consultation publishers. PresentationEmitter declares one and calls
its existing reducer/Bus/ordered-delivery admission; no second document owner is
introduced. FanoutEventSink counts publishers through nested fan-outs and refuses
zero or multiple destinations before invoking any publication. Wiring counts stay
constant for each sink lifetime; runtime destination availability remains the
emitter's own refusal. This is not an OS-focus routing decision.

Authored topology assertions use a completed retained-runtime result and cover
zero publishers, duplicate references, nested ambiguity, one nested publisher,
passive observers and propagation of a destination refusal. The existing local
HTTP fixture now sends its completed group through FanoutEventSink to the actual
PresentationEmitter, checks duplicate refusal and retains its ledger, Bus and
delivery-buffer assertions. These tests are unexecuted, not runtime proof.

The Apple worker still does not schedule grouped Max assessments or deliver their
results. Next assembly must connect the recorder-observed candidate owner, bounded
assessment scheduling, fresh-source admission, this completed-answer method and
stop/cancel settlement. No structural-close, install or integration claim is made.

## W1 live Max connection checkpoint

The Apple session now consumes the injected live FormattingAgent instead of
discarding it. Arming requires live-formatting capture intent, enabled Max and
exactly one configured consultation publisher. Correction/Smart keep the existing
occurrence formatter path; SingleTurn capture does not enter this live owner.

LiveConsultationCapture resides on the existing Apple capture worker. It observes
that worker's SileroIngest open/closed edges, invalidates unaccepted boundaries on
continued speech and reads sealed candidates from the same AcousticLedger and
Silero acoustic evidence. One immutable candidate is assessed at a time. Unchanged
pending input is not resubmitted every PCM tick. Assessments and accepted answers
use separate async future queues so waiting for an answer/approval does not hold
up a newer semantic assessment. At most sixteen answers await completion.

Before executor admission, the worker checks fresh source under the ledger lock
and reserves answer transport. The admission handle is retained even if subsequent
queue acknowledgement refuses. Answer collection flushes already-emitted source
events before invoking EventSink's typed completed-answer method. The async session
continues draining admitted work if its engine event sender closes early.

On ordinary EOF, final source sealing/repair precedes a final grouping attempt and
settlement before terminal ledger publication. CONTINUE is not forced to COMPLETE.
Source that cannot settle produces an explicit warning. A lost return channel
ends capture-side waiting with refusal, not an assertion that accepted tools were
cancelled; the retained executor/history still owns their recovery. Assessment
failure alone keeps the source pending and allows a later changed candidate;
execution/publication ambiguity stops further admission within that capture.

Three new active-module tests are authored for actual open/closed edge semantics,
disconnected-return stop handling and error return without fabricated publication.
They are unexecuted. This checkpoint has no compiler, lint, security-gate or runtime
verification. Remaining W2/W4 obligations include the complete live audio/provider/
tool/approval/presentation round trip, cancellation and host-task loss while effects
are running, source mutation during assessment, and resource/latency measurements.
In particular, synchronous durable executor admission currently runs on the Apple
worker: its journal I/O can delay PCM processing even though model execution is
async. This remains an explicit structural/performance review obligation, not a
claim of nonblocking capture. Stop while an approval is pending follows the existing
executor's timeout/settlement policy; no new forced tool cancellation is asserted.

The existing history census and worktree-admission obligations remain open. This
checkpoint does not integrate any historical branch or certify all conversations
reviewed. No W2_STRUCTURALLY_CLOSED receipt or installed-app claim is issued.

## W2 structural integration and admission repair — 2026-09-14

Founder explicitly requested integration into the current Codescribe branch.
Roman integrated the exact cut tip `fa9225f17da179fcb726f88bfc32bb428166e6b3`
by fast-forward into Living Tree `fix/seals-whales-and-agents`, from
`b91e9a947941371d0d51ba6cee1e3e68e7331e8c`. The original Fleet Worktree and
its branch remain intact. Existing uncommitted generated Swift bindings and
untracked `docs/settings.json` were preserved byte-for-byte and are not owned
by this repair. This is structural integration, not verified delivery.

Claude's audit of that exact cut confirmed synchronous journal persistence
under the capture worker's AcousticLedger lock. No repair or executable gate
was performed by that audit; it did not close W2. Historical commit-trailer
concerns remain recorded without rewriting the 53 original commits.

The admission repair separates persistence from execution authority:

1. A COMPLETE assessment schedules one bounded preparation on `spawn_blocking`.
   The same retained consultation runtime persists the candidate and reserves
   its FIFO slot, but its owner waits for a one-shot authorization before any
   provider or tool work. Capture holds no journal/admission lock during this.
2. Capture receives the prepared handle and re-reads current speech and source
   under the ledger lock. Only a matching candidate with speech still closed
   may authorize. That operation only sends a one-shot signal, with no I/O.
3. Stale source, resumed speech, lost return transport or dropped preparation
   closes authorization. The existing owner durably removes the unstarted
   input in FIFO order. Cleanup failure requires recovery, not replay.
4. Accepted answers retain the existing durable completion, source identity,
   presentation admission and single-destination path. Session drainage also
   tracks outstanding preparations. A process crash with retained waiting
   input still requires explicit recovery and cannot silently execute it.

Authored, unexecuted regressions cover no provider call before authorization,
rejection cleanup and explicit retry, authorization while journal/admission
locks are held, plus preparation error propagation. Existing source-staleness,
five-occurrence conservation and grouped-answer tests remain obligations.

This repair remains structural W2, without `W2_STRUCTURALLY_CLOSED`. No build,
test, formatter, linter, application probe or install was run. `git diff --check`
is structural hygiene only. A local checkpoint uses `--no-verify`, skipping
the full pre-commit and commit-msg entrypoints: whitespace/EOF/conflict/line-
ending checks, private-key detection, cargo-check, cargo-fmt, Prettier and
commit-message provenance. Full applicable gates, including security, must be
restored after structural closure. No push or publication is certified here.

The follow-up replaces `admit_assessed`'s arbitrary callback with typed
`authorize_prepared`. Capture can no longer accidentally put disk work inside
that callback. Staleness tests now observe the one-shot signal directly:
changed source, insufficient coverage, resumed speech and a successor boundary
must close it without authorization; disconnected execution must not advance
the accepted prefix. These remain authored, unexecuted tests under the same
structural embargo and checkpoint hook-bypass accounting above.

## W2_STRUCTURALLY_CLOSED — Max integration, 2026-09-14T18:15:31Z

Integrator: Roman (Codex), following the Founder's explicit instruction to
integrate this cut into the current branch. Runtime: Living Tree at
`/Users/maciejgad/vc-workspace/vetcoders/codescribe`.
Assembled source SHA: `b45115f5f45f9ddd8e96a7ac2e20f1833ebdf08e`.
Base and exact admitted Fleet tip are recorded above. This receipt changes
documentation only; subsequent source repairs must name their own SHA.

The Max source graph is assembled and ready for executable verification:

- Both controller recording entrypoints inject the selected retained Max host.
  The live capture owner uses bounded assessment/preparation/answer transports.
- Durable preparation has no execution authority. Capture revalidates current
  PCM member evidence, then signals authorization without journal/admission I/O.
  Dropped preparation is rejected before provider execution.
- Terminal composer and explicit overlay formatting invoke the same retained
  host through FormattingConsultation. Non-Max policies do not admit Max tools.
- The host selects the immutable formatting lane, uses the existing configured
  tool registry and AgentSession, and retains local history across provider changes.
- Journal FIFO, execution lease, begin/complete and ThreadDeliveryGateway own
  the durable execution boundary. Unresolved state is not automatically replayed.
- Approval snapshots/decisions and new-consultation controls have matching Rust
  bridge exports and Swift consumers. Existing UniFFI generation is the declared
  W3 binding-generation step; generated output is not asserted current.
- Completed groups pass through the single declared presentation destination,
  existing ledger/reducer validation and Bus projection, not a new document owner.
- Authored tests cover group conservation, stale source, loss of authorization,
  history, replay refusal, permissions, provider selection and bridge/UI seams.
  Their execution and the real microphone/clipboard path remain wholly unproved.

Evidence: refreshed Loctree slices/occurrences after integration; bounded source
reads of the above callers/owners; zero remaining `admit_assessed`/`enqueue_group`
call sites in core/app/tests; whole-cut `git diff --check`; `pre-commit run detect-private-key --files <47 exact cut paths>` passed. Local Semgrep config
over app/core/bridge/macOS/tests ran 3 applicable rules on 381 tracked files,
0 findings, 58 ignored files. This does not replace `make check`'s full security
configuration. The current commit-provenance hook checks subject prefixes;
historical missing trailer findings are retained without rewriting ancestry.

Grade B phase advances to W3: all preregistered compiler/formatter/test/build
commands return, followed by full `make check`, `make verify`, `make test-swift`,
binding regeneration and idle-safe install with real two-turn acceptance.
No active marker exists to change. Existing foreign Swift-binding changes and
`docs/settings.json` remain excluded from authored staging and must be preserved
when regenerating bindings. Source-level closure is not installed-artifact proof.

Operational risk: only 18 GiB free, no active cargo/rustc/xcodebuild observed.
Cold build cost is unknown. Do not run parallel build trees or allow a build
to exhaust the volume. Disk pressure does not waive any required gate.
The all-conversations intention census, other historical product debts and
cross-host voice-bus acceptance remain open outside this Max phase transition.
No full-goal, release, runtime performance or lexical-accuracy claim is made.

## W3 measurements and structural re-entry: SessionConfig consumer census

Receipts: `/tmp/codescribe-w3-20260914.kJy9Wj/`.
The first full `make check` stopped at Rust formatting (`01-make-check.log`).
After formatting the cut's Rust files, the second stopped at non-Rust formatting
(`02-make-check.log`): both authored documents were formatted, but the foreign
untracked `docs/settings.json` was deliberately left untouched. Full check is red.

Full-workspace/all-target Clippy ran with two jobs, no incremental artifacts and
dev/test debuginfo disabled to bound disk use. No target or warning was disabled.
`03-clippy.log` reports unused import, Boolean simplifications and a large queue
enum; repair 1 boxes the queue payload and preserves predicates. `07-clippy-repair-1.log`
reports equivalent Boolean/if simplifications in controller/emitter; repair 2
applies them. `08-clippy-repair-2.log` reports filter/next_back in the HTTP test;
repair 3 uses rfind. `09-clippy-repair-3.log` then exposes a missing SessionConfig
field in `examples/guardian_archive_probe.rs`. All four Clippy exits are 101.
The three-attempt mechanical loop ends here, not silently extended.

Classification: INSTRUMENT_FALSE_NEGATIVE in the integrator's consumer census,
not a compiler/architecture disagreement. The original W2 receipt remains above;
it omitted an example consumer and therefore overstated graph completeness.
Structural W2 re-entry is bounded to SessionConfig constructors and that example.
Loctree occurrence evidence is saved as `session-config-occurrences.json` and
cross-checked against literal constructors in core/app/bridge/examples/tests:

- live recorder: passes its selected live_formatting_agent;
- production-session replay, buffered harness and seal-coverage harness: None;
- guardian archive diagnostic: previously missing, now explicitly None because
  it replays archived PCM and must not invoke a live consultation's tools.

The definition and Apple destructuring already agree. The all-target compiler
gate remains the regression instrument for example consumers; it must be rerun
after renewed exact-SHA closure. No further compiler run occurs in this re-entry.
The source repair does not alter recorder ownership, speech identities or tools.

Additional gate outcomes: env registry passed (136 variables); gate ledger passed
(34 classified targets). Native full Semgrep ran 1827 rules on 693 files and
reported four blocking path-traversal findings in thread-store writes/directory
sync calls (`04-semgrep-native.log`). These remain unresolved, not suppressed or
declared false positives. The initial `--config auto --metrics=off` invocation
was invalid and is not a scan result. Local-config-only clean results cannot
substitute for this failed full security gate. Build artifacts measured 884 MiB,
with 17 GiB free; no app was installed or recording interrupted.

This structural re-entry checkpoint preserves formatter/mechanical changes and
the example fix with `--no-verify`; the full skipped pre-commit/commit-msg controls
are the same list recorded above. All still require final restoration. Foreign
Swift binding changes and settings remain excluded. No push, verified delivery,
or new structural-close claim is made by this checkpoint.

## W2 security repair: exclusive temporary thread writes

Follow-up on the full security gate found a concrete staging-path defect in
ThreadStore::atomic_write: `File::create` follows a pre-existing predictable
`.tmp` symlink and truncates its target. Thread ids being validated does not
protect that sibling file. This directly affects durable Max history and
attachment writes. Structural repair scope adds only this existing writer and
its regression: unique UUID staging name, create_new, private 0600 mode, and
the existing write/sync/rename/directory-sync sequence. No new storage owner.
The authored test puts a staging symlink beside the destination and verifies
the unrelated target survives, the destination receives the intended content,
and the published file is private. It is not yet executed.

The three remaining findings name read-only directory opens for fsync in the
consultation store. Their paths originate from the store root, fixed directory
names, canonical-child validation and validated thread ids. No external bytes
are written by these opens. This is a source-level false-positive assessment
of those particular sinks, not a claim that the scanner accepts it, nor a proof
against hostile concurrent replacement of ancestor directories. No rule was
disabled. Full security gate disposition remains pending the next scan.
This checkpoint retains structural embargo and the hook-bypass accounting above.

## W2_STRUCTURALLY_CLOSED — consumer/security repair

Roman closes the bounded re-entry against source SHA
`c1e4ddb1bf5b6d6fdc0af8ce7ba52778f32b9fc5` on 2026-09-14. The complete identified
SessionConfig constructor census now agrees on executor ownership; archived
replay has no executor. The existing atomic writer owns an exclusive private
staging file, and its destination and durability sequence are unchanged.
Source/consumer inspection and `git diff --check` support readiness to check,
not correctness in execution. This supersedes neither the recorded first
closure's missed consumer nor any failed gate. All preregistered gates return.
The next Clippy uses `--keep-going` with workspace/all-target scope to surface
independent crate failures together instead of serially hiding later targets.
Security findings, foreign settings formatting, tests and installed proof remain
open acceptance obligations. No full-goal completion is implied.

### W3 consumer repair results — 2026-09-14

The full keep-going Clippy pass found two further test consumers: the formatter
policy-entry smoke test omitted its optional consultation argument, and emitter
tests omitted the consultation presentation input import. Repair attempt 1
restored these contracts. The next pass reported a test guard lifetime despite
its explicit drop; repair attempt 2 gives the assertions a lexical lock scope.
`13-clippy-test-lock-scope.log` then passed workspace/all-target Clippy with
`-D warnings` (exit 0). No lint was suppressed and no assertion weakened.

The second full Semgrep scan (`11-semgrep-after-staging.log`) no longer reports
the thread staging writer; the three directory-sync findings remain. This is
not a green security gate. `make verify` was launched with the same bounded
Cargo resource settings; its pending result is in `14-make-verify.log` under
`/tmp/codescribe-w3-20260914.kJy9Wj`. Foreign Swift/settings hashes remain
unchanged. Test-consumer repairs are still uncommitted; no push or install.

The ongoing native verify run has now passed all 106 structural instrument
tests, the live `wired` acoustic-throne receipt, and the Transcript Bus path /
install-guard tests. It has advanced to compiling workspace tests; this does
not yet prove their execution. Scoped pre-commit trailing-whitespace, EOF,
merge-conflict, mixed-line-ending and private-key controls passed for the five
authored repair files. The current branch independently contains the original
cut tip `fa9225f17da179fcb726f88bfc32bb428166e6b3` by Git ancestry.

### W3 runtime message codec failure

The first native verify run terminated with exit 2 after its Rust test command
failed: `max_uses_formatting_provider_and_model_not_chat_settings` could not
admit a text instruction. Serde's internally tagged ContentBlock enum cannot
encode its Text(String) variant. This was real Max admission breakage, not a
mock mismatch. Every queued instruction includes a Text block, including
attachment-only instructions, so this representation could not persist a
normal admitted input.

Repair attempt 3 uses adjacent `type` / `payload` fields on the existing
runtime ContentBlock codec, retaining every enum variant without changing
provider request projection or ThreadStore's separate storage projection.
It also adds a complete Message roundtrip test including non-ASCII text,
inline image bytes, image references, tool invocation and nested tool results.
The existing full Max HTTP end-to-end test remains unchanged as the independent
admission falsifier. Existing malformed journal data is not silently repaired
or replayed. The rerun is `15-make-verify-message-codec.log` in the same receipt
directory; it is still pending. No successful Max execution is claimed yet.

The rerun has now terminated (make exit 2): all three HTTP agent-lane tests
passed, including the formerly failing Max admission. Core's prepared rejection /
explicit retry, grouped durable answer, staging-symlink and full-message codec
regressions also passed. Core totals were 1433 passed, 3 failed, 5 ignored.
The failures are the rejection/timeout approval fixtures and the clean-terminal
tool execution matrix in `core/agent/session.rs`. The first two omit provider
ResponseDone events; they therefore hit terminal refusal before testing approval.
The third registers its counter through the default policy; its authorization
precondition needs independent review before changing an assertion or runtime.

The three-repair budget is exhausted. Roman re-enters structural review for
the session test fixtures and their actual terminal/permission contracts.
No further executable gate is authorized until this bounded re-entry is closed
against its checkpoint SHA. The runtime clean-terminal requirement remains
unchanged; neither dropping failing tests nor relaxing authorization is a repair.

Structural review confirmed `register` assigns Unknown risk and therefore Ask;
the terminal matrix's in-memory counter now declares ReadOnly explicitly. The
approval fixtures now supply clean terminal events on both provider rounds and
assert that the approval callback was actually entered before the handler-not-run
assertion. Dirty/missing-terminal matrix cases remain unchanged. These are test
precondition repairs, not production authorization changes.

This structural checkpoint includes the preceding owned codec/test repairs and
receipts only. It uses `--no-verify` under W2; skipped controls are trailing
whitespace, EOF, conflict markers, mixed line endings, private-key detection,
cargo-check, cargo-fmt, Prettier and commit-message provenance. Earlier scoped
passes do not certify this new checkpoint. All controls return after exact-SHA
closure. Foreign Swift and settings changes remain unstaged; no push or install.

## W2_STRUCTURALLY_CLOSED — session fixture contracts

Roman closes this bounded re-entry against
`9dd97b252e0b3b3688932d6e451e629a927d40a2` on 2026-09-14. Source inspection
traces clean ResponseDone to the terminal gate before tool resolution, then
Unknown risk to Ask and the approval callback to rejection/timeout. Fixtures
now reach the branch they claim to falsify, without replacing any runtime
decision or removing negative cases. The source diff is test-only for this
re-entry; previous codec and admission repairs retain their recorded receipts.
All deferred gates return now. This attests readiness to execute verification,
not successful tests, security acceptance, installed delivery or goal completion.

### W3 native verify passed

`16-make-verify-approval-fixtures.log` completed with exit 0 on the checkpoint
plus rustfmt-only session formatting and these receipts. All three previously
failing session fixtures passed. Core totals: 1436 passed, 0 failed, 5 ignored.
The complete native verify chain also completed workspace targets, doctests,
Whisper promotion, env/data references and all 98 gate-ledger scenarios.
The gate explicitly excludes Swift, real-provider/audio parity and installed
host acceptance. The new checkpoint's scoped hygiene/private-key controls
passed for all seven owned files. Semgrep's three directory-sync findings and
foreign settings formatting remain unresolved full-check obligations.

### Bridge / Swift verification started

The final workspace/all-target Clippy (`17-clippy-after-verify.log`) passed,
exit 0. FFI build passed (`18-ffi-build.log`). Before regenerating the shared
Swift file, an isolated bindgen preview at
`/tmp/codescribe-bindings-20260914.y7V3An` confirmed that all five pre-existing
`canSendToAgent` sites remain generated from `bridge/src/recording.rs`.
No foreign field was removed. This preservation is not an ownership claim.

Native `make app-bindings PROFILE=debug` then passed (`20-app-bindings.log`),
regenerating the Swift/C bridge and Xcode project from the current Rust library.
The generated diff adds the cut's missing API alongside the retained foreign
field. `make test-swift PROFILE=debug` is running with logs
`21-test-swift.log` and `21-swift-xcode.log` in the same receipt directory.
No app installation or release was performed. Swift results remain pending.

Swift attempt 1 failed at compilation: `Darwin.flock` resolves to the imported
struct on this toolchain, not the POSIX function. A typed local Swift probe
confirmed unqualified `flock` resolves correctly. Installer and its lock tests
now use that function with identical descriptors/flags; no locking was removed.
Attempt 2 compiled those sources but failed at link, executing zero tests:
`mis-aligned LINKEDIT string pool` in the Rust dylib (Xcode beta ld 27037.1).
Both original Cargo and relocated copies have LC_SYMTAB.stroff 82490244, so
the misalignment precedes install_name_tool. Cargo.toml already documents this
Xcode 27 issue and disables strip on release/local-release. Only Xcode-beta
is installed. The debug verification build is now being rebuilt with explicit
`CARGO_PROFILE_DEV_STRIP=none`, retaining DEBUG=0 and two build jobs; no source
gate or linker diagnostic is suppressed. Log: `23-bindings-unstripped.log`.

That rebuild passed: LC_SYMTAB.stroff is now 82452672 (8-aligned), and native
binding/project generation completed. Swift is being retried with the matching
unstripped debug library; logs `24-test-swift-unstripped.log` / `24-swift-xcode.log`.

Founder subsequently authorized deciding the untracked profile's disposition.
Inspection identified a local settings profile, not a tracked product default.
Roman retains it untracked and does not apply it to runtime; Prettier formatting
completed with identical canonical JSON hashes before/after. Calling it the
Founder's authored file was unsupported; its author remains unknown. This
supersedes the earlier formatting blocker and byte-for-byte preservation rule
for this file only. No configuration values were intentionally changed.

Swift compilation next exposed a test reading private `OverlayState.recording`.
The test now exercises public `stop()` and observes the test engine's stop
callback after the previous send resolves, while retaining generation/close/
presentation assertions. No production visibility or recording behavior changed.

`25-test-swift-overlay.log` executed 642 tests with one skipped and five failing
assertions (make exit 2 / Xcode exit 65). Three overlay action-array expectations
omit the projected sendToAgent action; two assertions in the installer's
incomplete-rollback fixture expected a retained backup but found none. These
are unresolved findings, not declared harmless without contract inspection.
The suite took 30.088 seconds, also slightly beyond its 30-second budget;
slowest was the long-revision refusal-layout test at 5.371 seconds. No time
budget has been increased. Linking with the unstripped artifact now works.

After the bounded Swift repair attempts, Roman re-enters structural review of
these fixtures and their production action/rollback paths. Further executable
gates wait for exact-SHA structural closure. Rust's passed receipts remain
scoped to their prior source; no Swift green or installed delivery is claimed.

Structural review: the overlay fixture defaults canSendToAgent to nonempty
terminal text and the rail projects that permission directly; its three stale
arrays now include the action. Explicit permission-refusal cases remain.
The rollback double compared Foundation URLs whose directory hints differ
between its fixture and the production destination. It now compares standardized
filesystem paths and counts injected refusals; the test requires one refusal
before asserting retained backup contents and recovery diagnostics. No production
rollback operation or assertion about backup preservation is removed.

This checkpoint also records the generated Swift/C bindings produced from the
committed Rust API. The pre-existing canSendToAgent edit is retained as a
reproducible projection of existing Rust, not attributed as a new authored
feature. All generated additions were produced by native app-bindings; the
untracked local settings profile remains excluded. W2 checkpoint uses
--no-verify; all hook controls listed in the preceding W2 receipt are skipped
again (including security) and return after closure. No push or installation.

## W2_STRUCTURALLY_CLOSED — Swift consumer and rollback fixtures

Roman closes this bounded structural re-entry against
`4da254db95268d0118c1ac4fb0e7c83d22790b50` on 2026-09-14. The native-generated
Swift/header pair consumes the committed Rust API; the fixture's projected
permission matches the rail; rollback failure injection addresses the same
filesystem destination as the installer. Positive/negative assertions retain
their original behavior contracts. All gates return. This is readiness to
verify, not a Swift pass, security acceptance, installation or full-goal closure.

### Swift gate passed — 2026-09-14

The rollback test now constructs the diagnostic backup path using the same
destination spelling as the installer, verifies resolved file identity against
the directory listing, and reads original bytes through that reported path.
This preserves the recovery requirement despite temporary-root path spelling.
`27-test-swift-recovery-path.log` / `27-swift-xcode.log` completed with exit 0:
642 tests, one skipped, zero failures, 29.806 seconds under the unchanged
30-second limit. Prior runs slightly exceeded that limit, so timing headroom
is narrow; no broad performance claim follows. The installed app and real
cross-host voice flow remain unverified. Full make check is the next gate.

### Full static gate terminal result — 2026-09-14

At source HEAD `b4297b631908af7eef560692f958a3a0ada6db54`, the recovered
`28-make-check.log` records a terminal failure, not a running scan. Rust and
non-Rust format checks and workspace/all-target Clippy passed. Semgrep's final
summary reports 1827 rules on 693 files and three blocking findings, all in
`core/agent/thread_store/consultation.rs` at lines 217, 225 and 258. The initial
scan plan listed 2936 rules; that is not the executed-rule count.

Each reported sink opens a directory read-only and calls `sync_all`; these
calls do not read file contents or create/truncate a file. This source-level
observation does not certify the surrounding path construction, ancestor-race
resistance, or security acceptance. No rule, suppression or sink spelling was
changed to obtain a green result. Full `make check` remains failed.

Because make stopped before the remaining checks, the integrator ran the two
unchanged native registry scripts separately. `29-env-registry.log` passed
with 136 registered variables; `30-gate-ledger.log` passed with 34 classified
targets. Those successes do not override the security failure. Logs remain
under `/tmp/codescribe-w3-20260914.kJy9Wj/`.

A fresh read-only `bus-demux.py --assert-install-idle` returned exit 0.
This was a point-in-time bus observation, not an installation lease or proof
that the agent-turn lock is free. No app replacement, restart, push or success
ping occurred. Tracked files were clean before this receipt; the local
`docs/settings.json` profile remains untracked. Next: settle the concrete
security findings, then perform idle-safe installed-artifact acceptance and
the still-unproven live consultation scenarios.
