# Max consultation — implementation contract

Status: structural design, not implemented or verified.
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
   an Agent tool executor. The built-in Max prompt expressly forbids answering
   instructions and requires 1.5–3x expansion.
5. `core/agent/session.rs::AgentSession` already owns history, provider chaining,
   tool round-trips and permission decisions. Reuse it; do not create another
   model/tool loop.
6. `app/controller/helpers.rs::AgentRuntimeState` demonstrates durable thread
   recovery and serialized turns, but its shared slot follows Agent UI thread
   selection. Do not attach Max blindly to that slot.
7. `app/agent/mod.rs::create_default_provider` explicitly selects the assistive
   lane. Max must retain the immutable formatting lane's provider/model/auth
   identity. Concrete provider `from_lane` constructors already accept a sealed
   lane and request timing.
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

No Max source change, gate result, installation or integration is claimed by
this document. Structural work follows the compile-embargo contract; executable
verification requires the appropriate integration phase and exact SHA receipt.
