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

These checkpoints are structural W1 work. Checkpoint hooks are bypassed in full:
trailing-whitespace, end-of-file-fixer, check-merge-conflict, mixed-line-ending,
detect-private-key (security), cargo-check, cargo-fmt, prettier and
commit-msg-provenance. All remain verification obligations. Only source review
and `git diff --check` have been performed for this step. No executable gate,
installation or integration is claimed; those require W2 structural closure
against the exact assembled SHA first.
