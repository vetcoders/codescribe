---
name: codescribe
description: Connect this Claude Code, Junie or OMP conversation to a named Codescribe channel, receive messages without a typed nudge and answer through its attached voice. Use for /codescribe or a request to listen to Codescribe.
---

# Codescribe — native conversation (Claude Code, Junie and OMP)

In Claude Code, use `provider=claude-code` and the real session_id injected by
the native Codescribe session hook. In OMP (oh-my-pi), use `provider=omp` and
the real session_id injected by the bundled Codescribe extension.
In Junie, use `provider=junie` and this
conversation's real session_id from its native `/link` shared by the Founder.
Junie's SessionStart hook displays the identity to the user, not to the model;
it never injects prompt context because Junie 26.10.5 also executes that context
as part of `!` commands. An identity already supplied for this conversation
remains valid. Never invent an ID, use another chat's ID, launch cs-junie or
create an ACP session. If identity is absent, do not connect: obtain `/link`
in Junie or report the missing session hook in Claude Code or OMP.

For `/codescribe NAME CHANNEL [VOICE]`, call `codescribe_connect` with that
provider, session_id, name and single-digit channel (1–9). Set voice/vendor only
when explicitly requested. An occupied channel is not yours to take over. Keep
the existing voice and credentials. Attachment is `attached_unverified`, not
proof of voice reception.

## Restore the conversation context

For an established agent, recover relevant history after connecting. Match the
exact provider/session/lease from the connect/status receipt, not just the channel
or name. In Codescribe's **Agents / Agenci** panel, open that current conversation
or its predecessor under **Saved conversations / Zapisane rozmowy**. Read the
relevant messages and confirm an expected historical message and its owner.
A resumed session keeps its existing owner; a new session reads its predecessor's
chat as context without borrowing its identity or moving the old messages.

The unread mailbox is not the full chat, and opening a conversation does not
inject it into model context. Report what was actually read. If native history
access is unavailable, state that limit rather than claiming an import. Do not
ACK old deliveries or execute old requests merely because history was opened.
The full procedure is in the foundation skill's `references/history.md` under
`<bridge-home>/runtime/skills/codescribe/` (normally
`~/.codescribe/agent-bridge/runtime/skills/codescribe/references/history.md`).

Maintain this connection until the user cancels it, or an authorized successor
takes over/detaches it after your handoff. A reply ending or a quiet mailbox
does not end bus tenure; keep the existing native wake mechanism armed.

## Anti-patterns

Do not poll `codescribe_read` or `cs-bus --read-pending` on a timer beside the
native waiter or watch (for example every 2 seconds). Handle its actual
notification at the next work boundary, then perform the bounded read/ACK/drain.
Keep a quiet waiter or `--watch --bell` waiting; silence does not authorize
cancelling it, restarting its follower or adding a second reader.

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
then do the steps above. It re-arms on every turn end and after every tool call,
so an interrupted turn (Claude Code runs no `Stop` then) does not leave you deaf
and there is no watcher to remember. A `cs-bus --watch --until-event` background
task of this session may run beside it: the waiter then leaves the wake to that
task and takes over when it is gone. A `codescribe_listener_problem` notice (lost follower, refused mailbox)
means nothing was read: inspect `codescribe_status` and connect again with the same
name and channel. A `codescribe_wait_idle` notice after six silent hours only
re-arms: end the turn. `codescribe_status` reports `native_wake_armed`.

A message can also reach you **inside a running turn**: the plugin's
`PostToolUse` hook probes the mailbox after every tool call and, when a delivery
is unannounced, adds the same `codescribe_mailbox_ready` notice as context. Treat
it exactly like a wake: read, ACK the exact IDs, act on it, then continue the
current work. The probe prints nothing while the mailbox is silent and never
reads, ACKs or replies; the Stop waiter does not announce a probed delivery again.

### A channel on another Mac

Another Mac with Codescribe (an ssh host such as `host-a`) has its own bus.
Attach there under this same identity:
`cs-bus --remote HOST --attach --channel N --name NAME --provider claude-code`
(the helper sends this session's id). The Stop hook then also waits on HOST,
through HOST's own waiter, and wakes this conversation with the same notice
followed by `[Mailbox on HOST, over ssh …]`. The `codescribe_*` tools read only
this Mac: read, ACK, send and detach there with `cs-bus --remote HOST --read-pending | --ack ID | --send … | --detach --provider claude-code`. The
in-turn probe does not reach HOST; its messages arrive with the waiter's wake,
which can also land inside a running turn. Needs ssh key access (BatchMode)
and the managed helper at `~/.local/bin/cs-bus` on HOST.

Use `codescribe_wait` in Claude Code only when the Founder asks you to hold an
active listening turn: it blocks typed prompts until it returns, at most 15 min,
with a `wait_idle` receipt. Without the plugin hook, use the CLI lane of the
foundation `codescribe` skill (`cs-bus --watch --until-event` as a background task).

## OMP: end the turn, the extension wakes you

Same contract as Claude Code with a different carrier. The plugin's
`hooks/pre/codescribe-omp.ts` extension injects this conversation's identity
(`provider=omp`, the OMP session id) at session start, serves `/codescribe NAME CHANNEL [VOICE]`, and after every turn end holds `cs-bus --omp-wake` in the
background with no model turn. A new delivery or listener problem returns as
`codescribe_mailbox_ready` or `codescribe_listener_problem` and starts a turn;
then do the steps above and end the turn again. After six silent hours, the
extension re-arms the idle waiter without starting a model turn. The waiter
never reads, ACKs or replies. `codescribe_status` reports `native_wake_armed`. A new OMP
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
limit. This is an ordinary Junie CLI extension; no IDE is required. Never obtain
identity by adding a UserPromptSubmit context hook: it corrupts shell commands.

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
