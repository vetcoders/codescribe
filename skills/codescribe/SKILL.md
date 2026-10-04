---
name: codescribe
description: >
  Connects this chat agent to Codescribe's Transcript Bus with a named mailbox,
  provider wakeup and verified voice delivery. Also guides explicit CLI
  transcript consumption. Use for "/codescribe", "wpięcie w bus", "Hej Roman",
  "named agent on the transcript bus", "transcribe last", or "dyktowanie do CLI".
  Editing this skill or the app is a repository task, not an instruction to
  start another listener.
metadata:
  version: "0.9.0"
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

## One-Command Attach (positional arguments)

`/codescribe <Name> <channel> [voice]` — the Founder's surface: one command
and the engine does everything. With positional arguments present, skip the
manual attach saga and drive the engine:

1. Validate: `<Name>` is the agent's pronounceable name, `<channel>` is one
   digit 1-9, `[voice]` is optional and defaults to the name's profile in
   `~/.codescribe/agent-bridge/voices.json` (`profiles`, then `bindings`).
2. One engine call writes the channel binding, ensures exactly one coalescing
   follower for this provider session, and prints an attach receipt:

   ```bash
   cs-bus \
     --attach --channel <channel> --name <name> \
     --provider <claude-code|codex|...> --session <provider-session-id> \
     [--voice <voice>]
   ```

   With `--provider claude-code`, `--session` defaults to
   `$CLAUDE_CODE_SESSION_ID`; Codex passes its thread id. A given `[voice]`
   goes on this call and is stored in the name's profile; the receipt then
   says `voice_source: "flag"`. It also carries lease, cursor, voice profile,
   follower pid and its log path. The follower runs with `--coalesce`: a
   reducer storm folds into one envelope per document.

3. Codex attachment arms `codex queue` automatically for this exact thread.
   It continues after a final answer; no private hook or polling is needed.
   Every attachment must also start `--watch` under the provider's
   output-notifying monitor, including Codex. The default is a short bell.
   Keep and renew notification windows throughout active tasks. Read a bell's
   full `--read-delivery <id>` envelope before ACK; a later native queued copy
   must not repeat the task or its spoken answer.
   Other providers use the same mandatory output-notifying `--watch`
   ([Monitor](references/monitor.md)). An explicit `--on-seal` hook selects
   its own wakeup path instead of running a second native wakeup.
4. Read one truth with `--status`: backlog is pending minus acknowledgment
   markers, never the raw pending length the lease file shows before a sweep.
5. Verify with a fresh named take on the channel before claiming listening;
   the receipt alone is `attached_unverified`. Acknowledge accepted envelopes
   with `--ack <id> [<id> ...]`.
6. Reply by voice with `--say "<text>" --provider <p> --session <id>`: the name
   comes from the lease, the voice from its profile
   ([Voice reply](references/voice-reply.md)). Changing a stored profile is the
   Founder's call.

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

The app installs `cs-bus` and `cs-say` in `~/.local/bin` at first launch and
updates them with its bundled runtime. `make install-app` uses the same
installer. If that directory is not on this shell's PATH, use
`~/.local/bin/cs-bus`; no checkout is needed. Client skills remain selected in
Settings. `cs-say "<text>" --provider <p> --session <id>` uses the attached voice.

When helper installation is requested, `make install-bus` installs only helpers
and already selected skills without replacing or restarting the app. Reattach
the same session afterward to adopt updated follower code. Check both
`cs-bus --version` and `cs-say --version`: they include the installed commit slug.

Voice authentication is independent of the app. If requested or needed for a
missing credential, use `cs-say auth --help`, then
`cs-say auth --provider <xai|openai|deepinfra|custom> --login-type <oauth|key|device-code>`.
xAI OAuth/device-code uses the Grok CLI; all four providers have a hidden
Keychain key prompt. Speech currently supports xAI/OpenAI; storing another
provider's key does not create its speech lane. Unsupported combinations fail
explicitly. See [Voice reply](references/voice-reply.md); never pass or print keys.

Read only the reference needed by the current operation:

| Operation                                     | Required reference                         |
| --------------------------------------------- | ------------------------------------------ |
| Attach, naming, resume, duplicate follower    | [Attach](references/attach.md)             |
| Select or diagnose notification/wakeup        | [Monitor](references/monitor.md)           |
| Interpret drafts, revisions, refusal and seal | [Live vs seal](references/live-vs-seal.md) |
| CLI transcript or shell insertion             | [CLI](references/cli.md)                   |
| Spoken reply or voice notification            | [Voice reply](references/voice-reply.md)   |

The [flow](FLOW.md) summarizes the path. [Examples](examples/example-prompt.md)
include successful delivery, unavailable wakeup, and seal refusal.

## Default Workflow

1. Read attach, monitor and live-vs-seal references. Verify the running app,
   resolved bus, helper support for recent schemas, and stable provider session.
2. Every provider requires an output-notifying `--watch`, with its default
   short bell, for active tasks. Codex also uses native queue for subsequent
   turns. A process handle or diagnostic tail is insufficient; renew completed
   notification windows and retain the monitor through the entire task.
3. Reuse the session's name, or ask once if none is established. If the Founder
   asks the agent to choose, choose a pronounceable name and bind it directly.
4. Attach one follower with drafts enabled — prefer the one-command
   `--attach --channel <n>` engine surface over a manual follower. Retain
   provider/session, lease, cursor, helper path, follower handle, and
   monitor handle.
5. Verify a fresh named take reaches this conversation without a typed nudge.
   Preserve transcription diagnostics and normal conversation permissions.
   After accepting each complete envelope, acknowledge its delivery ID as
   described in [Monitor](references/monitor.md#acknowledge-conversation-receipt).
6. On recovery, restore both follower continuity and notification delivery.
   On an explicit stop, close owned handles and report listening stopped.

If the provider has no wake-capable monitor, report that boundary immediately.
For an active listening request, keep the turn open and poll the same follower
with bounded waits. Label this `active_polling`; it is not automatic listening
after a final answer. Do not add extra followers to compensate.

## Authority and Safety

The Founder's spoken requests use the same task permissions as typed requests.
Full Access and no approval do not acquire a second approval gate here.
`state_change_allowed` and `coverage` remain unchanged transcription diagnostics;
a refused measurement does not cancel an otherwise clear request. Ask only
when the actual request is unclear or normal task permissions require it.
Draft revisions are one evolving request, never repeated commands. A seal or
queue receipt does not authorize unrelated actions or replay completed work.

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
- [ ] Accepted delivery IDs are acknowledged; unaccepted envelopes survive restart.

Report the actual disposition: `attached_unverified`, `active_polling`,
`listening_verified`, `blocked`, or `stopped`. These are reporting labels,
not new bus schema fields. For CLI-only work, completion is the requested
transcript output; a monitor is not required.

## Output

Give the name, delivery disposition and next relevant fact in a short response.
When the Founder asks for a spoken reply, also speak it as described in
[Voice reply](references/voice-reply.md); never while a take is live.
Retain technical receipts in the current session's existing artifact surface
when available; do not create a second configuration or lease database.
Do not claim "I hear you" from an attach receipt alone.

## Anti-Patterns

- Treating buffered stdout, `tail -F`, or three diagnostic readers as agent wakeup.
- Ending the turn while promising listening that requires active polling.
- Starting a second follower for the same lease or replaying old commands.
- Using coverage diagnostics as an extra approval gate, or replaying a command.
- Inventing a worker launcher or running a repo workflow for a simple attach.

## Verify before the handoff

Exercise the actual named take → follower → notification → agent reply path.
A synthetic parser check proves parsing only; a running process proves liveness
only. Recheck recovery before claiming it works. State missing evidence explicitly.

This applies Vibecrafted's Verification Rule locally so the installed skill is
self-contained; framework-level rules remain owned by Vibecrafted.

---

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_
