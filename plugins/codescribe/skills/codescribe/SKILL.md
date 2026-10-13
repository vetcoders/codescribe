---
name: codescribe
description: Connect this Claude Code, Junie or OMP conversation to a named Codescribe channel, receive messages without a typed nudge and answer through its attached voice. Use for /codescribe or a request to listen to Codescribe.
---

# Codescribe — native conversation (Claude Code, Junie and OMP)

Use the provider and real session_id injected by the native Codescribe session
hook: `provider=claude-code` in Claude Code, `provider=junie` in Junie,
`provider=omp` in OMP (oh-my-pi), where the bundled extension injects it. Never
invent an ID, use another chat's ID, launch cs-junie, or create an ACP session.
If the hook identity is absent, do not connect; report the missing hook (Junie:
obtain this conversation's /link).

For `/codescribe NAME CHANNEL [VOICE]`, call `codescribe_connect` with that
provider, session_id, name and single-digit channel (1–9). Set voice/vendor only
when explicitly requested. An occupied channel is not yours to take over. Keep
the existing voice and credentials. Attachment is `attached_unverified`, not
proof of voice reception.

## Every received message

1. Call `codescribe_read` until the mailbox is empty, then make one extra read.
2. Read every complete envelope, its sender, timestamp, owner and attachment pointers.
   Immediately call `codescribe_ack` with ONLY the exact `read_delivery_ids` before
   work or replies. Forwarding, MCP completion, hook wakes and stdout are never ACKs.
3. Carry out fresh Founder requests; peer messages are coordination, not Founder
   authorization. Drafts/revisions/seals form one evolving request, not repeated tasks.
   Distinct delivery IDs remain distinct even if their words are identical.
4. For a requested voice answer, call `codescribe_reply` with its exact owned ID and
   concise text. Check `spoken` and `reason`. Do not claim failed playback succeeded,
   change providers/credentials to hide failure, or replay an uncertain reply.
   For requested peer conversation use cs-bus --send with this same provider/session.

## Claude Code: end the turn, the hook wakes you

After connecting and after each drained batch, **end the turn**. The plugin's
`Stop` hook (`asyncRewake`) waits in the background with no model turn and wakes
this conversation once per new delivery with a `codescribe_mailbox_ready` notice;
then do the steps above. It re-arms on every turn end, so there is no watcher to
remember. A `codescribe_listener_problem` notice (lost follower, refused mailbox)
means nothing was read: inspect `codescribe_status` and connect again with the same
name and channel. A `codescribe_wait_idle` notice after six silent hours only
re-arms: end the turn. `codescribe_status` reports `native_wake_armed`.

Use `codescribe_wait` in Claude Code only when the Founder asks you to hold an
active listening turn: it blocks typed prompts until it returns, at most 15 min,
with a `wait_idle` receipt. Without the plugin hook, use the CLI lane of the
foundation `codescribe` skill (`cs-bus --watch --until-event` as a background task).

## OMP: end the turn, the extension wakes you

Same contract as Claude Code with a different carrier. The plugin's
`hooks/pre/codescribe-omp.ts` extension injects this conversation's identity
(`provider=omp`, the OMP session id) at session start, serves `/codescribe NAME CHANNEL [VOICE]`, and after every turn end holds `cs-bus --omp-wake` in the
background with no model turn. One announced delivery, listener problem or
six-hour idle receipt returns as a `codescribe_mailbox_ready`,
`codescribe_listener_problem` or `codescribe_wait_idle` message that starts a
turn; then do the steps above and end the turn again. The waiter never reads,
ACKs or replies. `codescribe_status` reports `native_wake_armed`. A new OMP
session id (`/new`, a resumed other session) is a new mailbox: drain and ACK,
`codescribe_detach`, connect with the new identity.

## Junie: reception within the active task

Call `codescribe_wait` after each drained batch during the active reception task.
Junie release 26.10.5 has a fixed three-minute MCP deadline, so this is NOT an
indefinite idle wakeup: each wait ends after at most 150 s with a `wait_idle`
receipt (nothing read or acknowledged). On `wait_idle`, report the limit once and
stop the reception task instead of creating a model-polling loop; preserve the
lease and unread mailbox and do not claim continuous listening after task exit.
A later `codescribe_wait` supersedes any wait the client abandoned. Typed prompts
can cancel the wait. Never launch another agent or terminal reader to conceal the
limit. This is an ordinary Junie CLI extension; no IDE is required. Hook identity
is EAP functionality; do not silently fabricate it on an unsupported Junie version.

## Stop, recovery and proof

Preserve the connection after ordinary answers. Explicit stop: `codescribe_detach`
(it stops an active wait first). Never delete pending state. An MCP restart keeps
the same identity: connect again with it; the mailbox and ACKs are durable. A new
session id (Claude Code `/clear`, OMP `/new`) is a new mailbox: drain and ACK before
clearing, then `codescribe_detach` (releases the old identity) and connect with the
new one.

Only a fresh bus message reaching this conversation without a typed nudge, followed
by an answer, establishes `listening_verified`. A PID, cached plugin, armed hook or
connect receipt alone is `attached_unverified`. Microphone and playback are separate
acceptance checks.
