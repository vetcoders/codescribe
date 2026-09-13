---
name: codescribe
version: 0.5.0
description: >
  Connects this chat agent to Codescribe's Transcript Bus with a named mailbox,
  provider wakeup and verified voice delivery. Also guides explicit CLI
  transcript consumption. Use for "/codescribe", "wpięcie w bus", "Hej Roman",
  "named agent on the transcript bus", "transcribe last", or "dyktowanie do CLI".
  Editing this skill or the app is a repository task, not an instruction to
  start another listener.
loctree_value: "primary repo map for structural/literal repository work"
aicx_value: "intent, session, and decision-context retrieval"
dogfooding: "required for repo-impacting work"
---

<!-- fleet-imperative: v3 -->

> **Invocation for codescribe — foundation skill**
>
> | Path           | Invocation                                               |
> | -------------- | -------------------------------------------------------- |
> | Interactive    | `/codescribe` or load this skill in the current chat     |
> | Worker CLI     | None; do not invent `vibecrafted codescribe <agent>`     |
> | Agent-Operator | Load in the existing session; the Founder owns recording |
>
> Execute in-session. Attaching requires no worker dispatch.

<!-- /fleet-imperative -->

# Codescribe — voice delivery to this agent

## Purpose

Deliver a named spoken utterance from Codescribe to this conversation, with
proof that the agent receives it without a typed nudge. One microphone owner,
one runtime-resolved bus, one follower lease per provider session.

Founder means the human; Operator means an agent role. Keep those identities
distinct in messages and receipts.

## When To Use

- Attach or recover this chat's named voice mailbox.
- Diagnose which hop lost a spoken message.
- Consume a transcript through the explicit CLI surface.

For app or skill edits, use the repository's implementation workflow.
For screencast analysis, use `vc-screenscribe`. In-app Agent and Assistive
are separate product surfaces; this skill attaches the current chat.

## Operator Entry

### Living Tree / Worktree Rule

Use the current checkout and branch for repository work. Re-read before edits
and preserve concurrent changes. Attaching does not require a checkout, branch
change, worktree, installation, or release.

### Repository Work Doctrine

For repository changes, consume fresh `vc-init` evidence and use Loctree
`context`, `slice`, `impact`, and `find --literal` as appropriate before
editing. Use AICX for prior intent; use text search for local details.
Record Loctree misses in `~/.vibecrafted/loctree/loctree-fail.md`.
Pure attach and CLI consumption have no repo-orientation prerequisite.

## Pipeline Position

- Upstream: Codescribe produces bus events; a provider hosts this conversation.
- This skill owns follower attachment and verification of conversational delivery.
- Downstream: ordinary task execution within the Founder's authorization.
- This is a foundation capability, not a ship-cycle stage.

## Dependencies and Reading Path

Use the installed helper:
`~/.codescribe/agent-bridge/runtime/bin/bus-demux.py`.

Read only the reference needed by the current operation:

| Operation                                     | Required reference                         |
| --------------------------------------------- | ------------------------------------------ |
| Attach, naming, resume, duplicate follower    | [Attach](references/attach.md)             |
| Select or diagnose notification/wakeup        | [Monitor](references/monitor.md)           |
| Interpret drafts, revisions, refusal and seal | [Live vs seal](references/live-vs-seal.md) |
| CLI transcript or shell insertion             | [CLI](references/cli.md)                   |

The [flow](FLOW.md) summarizes the path. [Examples](examples/example-prompt.md)
include successful delivery, unavailable wakeup, and seal refusal.

## Default Workflow

1. Read attach, monitor and live-vs-seal references. Verify the running app,
   resolved bus, helper support for recent schemas, and stable provider session.
2. Select a supported monitor that wakes this agent on follower output. A
   terminal process handle or diagnostic tail does not satisfy this step.
3. Reuse the session's name, or ask once if none is established. If the Founder
   asks the agent to choose, choose a pronounceable name and bind it directly.
4. Attach one follower with drafts enabled. Retain provider/session, lease,
   cursor, helper path, follower handle, and monitor handle.
5. Verify a fresh named take reaches this conversation without a typed nudge.
   Distinguish live receipt from terminal permission; respond accordingly.
6. On recovery, restore both follower continuity and notification delivery.
   On an explicit stop, close owned handles and report listening stopped.

If the provider has no wake-capable monitor, report that boundary immediately.
For an active listening request, keep the turn open and poll the same follower
with bounded waits. Label this `active_polling`; it is not automatic listening
after a final answer. Do not add extra followers to compensate.

## Authority and Safety

Drafts may support conversational replies or read-only investigation.
A voice-requested state change requires a genuine `transcript_sealed` event
with `state_change_allowed: true`, plus the normal task authorization.
Fn release, silence, clipboard paste and `session_ended` are not substitutes.
A seal does not authorize unrelated actions or replay of a completed command.

Do not open a microphone, start Voice Lab, edit provider configuration, or
install the app merely to attach. Use the product installer only when that
operation is in scope. Never expose credentials through process arguments,
environment dumps, or unfiltered diagnostic output.

## Acceptance Criteria

Automatic voice attachment is complete only when:

- [ ] App and runtime-resolved bus exist; selected helper reads the observed schemas.
- [ ] Name is bound to this provider session, with one follower and retained lease.
- [ ] Monitor receipt identifies the mechanism that actually wakes this agent.
- [ ] A fresh named utterance produces an agent reply without a typed nudge.
- [ ] Draft/seal boundaries are preserved; recovery retains cursor and owner.

Report the actual disposition: `attached_unverified`, `active_polling`,
`listening_verified`, `blocked`, or `stopped`. These are reporting labels,
not new bus schema fields. For CLI-only work, completion is the requested
transcript output; a monitor is not required.

## Output

Give the name, delivery disposition and next relevant fact in a short response.
Retain technical receipts in the current session's existing artifact surface
when available; do not create a second configuration or lease database.
Do not claim "I hear you" from an attach receipt alone.

## Anti-Patterns

- Treating buffered stdout, `tail -F`, or three diagnostic readers as agent wakeup.
- Ending the turn while promising listening that requires active polling.
- Starting a second follower for the same lease or replaying old commands.
- Treating terminal refusal or session completion as permission to act.
- Inventing a worker launcher or running a repo workflow for a simple attach.

## Verify before the handoff

Exercise the actual named take → follower → notification → agent reply path.
A synthetic parser check proves parsing only; a running process proves liveness
only. Recheck recovery before claiming it works. State missing evidence explicitly.

This applies Vibecrafted's Verification Rule locally so the installed skill is
self-contained; framework-level rules remain owned by Vibecrafted.

---

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_
