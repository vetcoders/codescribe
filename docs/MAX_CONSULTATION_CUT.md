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
Image asset existence and tool-call/result pairing need further validation.

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

The controller currently creates a fresh consultation id after process launch;
durable selection/reset UI is not wired. Approval requests without a host broker
remain refused. Its event callback currently reports errors only; streaming Max
UI is not connected. Live occurrence formatting still passes None, deliberately
not executing one tool turn per acoustic fragment. This remains unassembled and
must not be installed as a completed Max cut.

Outstanding: production host-owned consultation selection/reset; durable
queued-but-not-started instructions; explicit
reconciliation of unresolved turns (never implicit replay); cancellation and
install lease; host execution handoff on all three formatting paths; permission
UI and live presentation/delivery wiring. The new owner is not connected to
production. The journal protects before-effects admission, not all acknowledged
in-memory queue entries; do not claim complete crash recovery yet.

Both checkpoints are structural W1 work. Checkpoint hooks are bypassed in full:
trailing-whitespace, end-of-file-fixer, check-merge-conflict, mixed-line-ending,
detect-private-key (security), cargo-check, cargo-fmt, prettier and
commit-msg-provenance. All remain verification obligations. Only source review
and `git diff --check` have been performed for this step. No executable gate,
installation or integration is claimed; those require W2 structural closure
against the exact assembled SHA first.
