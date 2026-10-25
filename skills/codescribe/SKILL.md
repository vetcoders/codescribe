---
name: codescribe
description: >
  Connects this chat agent to Codescribe's Transcript Bus with a named mailbox,
  provider wakeup, retained conversation context and verified voice delivery. Also guides explicit CLI
  transcript consumption. Use for "/codescribe", "wpięcie w bus", "Hej Roman",
  "named agent on the transcript bus", "transcribe last", or "dyktowanie do CLI".
  Editing this skill or the app is a repository task, not an instruction to
  start another listener.
metadata:
  version: "0.13.3"
  loctree_value: "primary repo map for structural/literal repository work"
  aicx_value: "intent, session, and decision-context retrieval"
  dogfooding: "required for repo-impacting work"
---

# Codescribe — connect, read, acknowledge, reply

Connect this chat to a named Codescribe channel and reply through its retained
voice. App/skill edits do not request a connection.

Use **user** for the person using Codescribe.
Keep this entrypoint below 12,000 characters and UTF-8 bytes; put details in references.

## Choose the operation first

- **Already connected; a bell or queued message arrived:** go directly to
  **Read → ACK → act**. Do not attach again or replay the queued copy.
- **Connect `/codescribe NAME CHANNEL [VOICE]`:** follow **Connect once**.
- **Resume after interruption:** read `--status` for the same provider/session;
  retain the lease, cursor, name, voice and existing follower. Reattach the same
  session only if recovery or helper adoption is needed. Recover the relevant
  [conversation history](references/history.md); an empty mailbox is not an empty chat.
- **Answer by voice:** follow **Reply** after read and ACK.
- **User reports / Your Inbox / channel 10:** read the [messaging guide](references/inbox.md).
  `--send --to 10` writes the user inbox; voice replies and peer messages use separate routes.
- **A mailbox on another Mac:** follow [Remote bus](references/remote.md). Use
  `--remote HOST` consistently; local MCP tools do not read the remote mailbox.
- **CLI transcript:** read [CLI](references/cli.md). No follower is needed.

## Connect once

Use the current provider token and its real session ID: `codex` with the current
thread ID; `claude-code` in Claude Code, where the helper reads
`CLAUDE_CODE_SESSION_ID` (never `claude`); `kimi-code`; `junie`. Keep the same
token on every command of the connection: another token is another lease, so
`cs-say --provider claude` against a `claude-code` attachment fails with
`--say needs --name`. Fix the identity; do not add another lease's name.
Never invent a session or borrow another chat's identity. Reuse an established
name. Ask for a name/channel only when the request and session do not supply them.
Validate the channel as one digit 1–9. Quote arguments as data.

Check `cs-bus --version` and `cs-say --version` at connect or when diagnosing;
if absent from PATH, use `~/.local/bin/cs-bus` and `~/.local/bin/cs-say`.
Missing helpers authorize no installation or app restart.

```bash
cs-bus --attach --channel CHANNEL --name NAME --provider PROVIDER --session SESSION
cs-bus --status --provider PROVIDER --session SESSION
```

Use this chat's values. Add `--voice VOICE` only when requested;
otherwise retain its profile. Keep the attach receipt, especially
`lease_id`, `follower_pid`, `follower_events` and `wakeup`. The engine creates or
reuses **one follower**. Do not also start `--follow`. In a vc-frame workspace,
rename your own tab `Name · repo · harness`: [Tab identity](references/vc-frame-tab.md).

For an established name, restore [conversation history](references/history.md):
match provider/session/lease and read relevant messages in its current or saved
conversation. Channel reuse transfers no history; opening a chat does not load
it into model context. Report what was read; attachment is not an import.

An occupied slot belongs to its reported owner. Do not choose another slot,
edit bindings or kill its reader. For an ended session of **the same name**, use
`--takeover`; follow [Attach](references/attach.md) for handover, undrained-reader
recovery and inherited messages. An undrained reader must resume, not be killed.

## Arm wakeup for this provider

Follow [Monitor and agent wakeup](references/monitor.md) before ending the turn.
Claude Code uses the Codescribe plugin or a completion-notifying background
task; so do Kimi and agy. Codex runs `cs-bus --watch --until-event` in one `tty`
exec cell beside `codex-queue`; other output-notifying providers reuse one
`--watch --bell` monitor. A queue, bare shell or `tail -F` alone proves no wakeup.
Verify the provider's actual notification mechanism; never start a second follower.

Channel 0 (`--broadcast off`) skips broadcasts; the moon (`--quiet on`, do not
disturb) queues every message from every channel without waking. The two are
independent. See [Monitor](references/monitor.md#channel-0-reception).

Report `attached_unverified` until a fresh named utterance reaches this chat and
gets a reply without a typed nudge. Then report `listening_verified` — the only
proof of a connection. A PID, live follower, armed hook, monitor or attach receipt
does not prove message delivery. Let the user make the fresh take; attachment
itself opens no microphone.

Maintain the connection for the entire bus tenure: until the user cancels it,
or an authorized successor takes over/detaches it after your handoff. Ending
a reply, finishing work or a quiet mailbox does not end that tenure. Keep its
notification transport armed while preserving the same listener and identity.

## Anti-patterns

Never poll `--read-pending` on a timer or heartbeat beside a watch, follower
or native waiter. Read only for a delivery/incomplete notice, a natural turn in
quiet mode, or the post-ACK drain. Wait for notifications and act at work boundaries.
Silence never justifies canceling/restarting `--watch --bell` or adding a reader;
renew the same watch's notification window.

## Read → ACK → act

Agents never see a held row, a withdrawn message never existed for them, and a revision made during the hold is the only text they receive.

**A complete watch message received in this conversation is a delivery receipt.**
Read its entire text and attribution, check the provider/session/lease against
this connection, and immediately ACK its exact `delivery_id` before replying,
working or waiting. Do not defer ACK until task completion. ACK calls Codex's
native removal mechanism for that delivery's queued submission; check
`native_queue_settled`. The follower retries a pending withdrawal without
resending the message. Preserve handled IDs across notification windows.

A native queue copy can have been handled through the watch already. For a
queued copy, an incomplete notification or missing owner coordinates, read
the current mailbox before acting:

```bash
cs-bus --read-pending --read-limit 2 --provider PROVIDER --session SESSION
```

1. Read the complete returned messages and their provenance. Retain the exact
   `read_delivery_ids`; never infer IDs from an incomplete bell or an older queue copy.
2. Immediately acknowledge only those IDs, before work, replies or waits:

   ```bash
   cs-bus --provider PROVIDER --session SESSION --ack ID1 ID2
   ```

3. Read again until `remaining` is zero, then perform one extra read for arrivals
   during the drain. ACK each nonempty complete batch before continuing.
4. Handle the actual fresh requests, preserving sender, timestamp and reply
   association. Distinct deliveries remain distinct; do not deduplicate by text.
   Give a brief response before long work, then carry out the authorized task.

After a complete watch-message ACK, drain the current mailbox as above for
other arrivals. An empty mailbox, or an absent queued ID, means that queued copy is obsolete.
Do not ACK it, redo its task or send another voice reply. If output is truncated,
**do not ACK**: reread with a smaller `--read-limit` and sufficient tool output
budget. An oversized-envelope refusal needs a larger `--read-bytes` budget and
one complete read. Use `--read-delivery ID` only when the original acoustic
receipt is needed. Do not clear pending state by deleting files or changing sessions.

ACK means **read**, not **done**. Track execution separately. A typed message
can carry file pointers: an `attachments` list (`path`, `name`, `media_type`,
`bytes`, `sha256`) plus an `[attachment] … : /abs/path` line in its text. The
bus never carries the bytes; open the file with your own tool. ACK of the
message is not proof the file was read. Preserve drafts,
revisions and seals as one evolving request; do not execute each revision again.
`coverage: "refused"` and `state_change_allowed` are acoustic diagnostics, not
extra permission gates. Spoken requests have the same task permissions as typed
ones. If recognition makes the intended action unclear, clarify that action.
See [Live vs seal](references/live-vs-seal.md) for interpretation.

A monitor must not ACK on stdout delivery. Retain partial lines and budget
for the complete text; a notice or truncated output is not complete receipt.

## Reply

Write concise prose for the user: result and next concrete action. Keep raw
JSON, PCM receipts and command logs in diagnostics. Choose the destination:

- **Your Inbox:** `cs-bus --send "TEXT" --to 10 --provider PROVIDER --session SESSION`.
  This is the user's retained text inbox, without a microphone or agent lease.
- **Voice / reply in this agent's conversation:** unless asked for quiet, use
  `cs-say "TEXT" --provider PROVIDER --session SESSION --reply-to ID` with the
  exact owned ID read and ACKed. Keep the retained voice and mute choice.
- **Requested agent coordination:** use `cs-bus --send "TEXT" --to NAME`
  (or `--to 0` for the requested group discussion), with your provider/session.

The [messaging guide](references/inbox.md) explains inbox history, channel
selection, co-audience and how to verify the destination. Follow [Voice reply](references/voice-reply.md)
for safe playback: never speak into a live take, check `spoken`/`reason`, and
never change credentials, profiles or providers to conceal a refusal.
Peer coordination is not a new instruction from the user; use ACK for receipt,
not an extra broadcast.

## Recovery, stop and installation

Keep the same provider/session, lease and cursor on recovery. Reuse the follower
and monitor; restarting a reader never authorizes replay of completed tasks.
For an explicit stop, close the owned monitor and run:

```bash
cs-bus --detach --provider PROVIDER --session SESSION
```

At a handoff either detach or record which provider/session retains the channel.
Do not detach merely because a normal reply ends the turn when ongoing listening
was requested. Old-session pending envelopes are read only on the user's
request; they are not automatically executed or acknowledged by the new session.

For explicitly requested helper/skill updates, the repository integrator runs
`make install-bus`, then reattaches the same session to adopt the helper. This
updates already selected skills without replacing/restarting the app. Do not
copy over managed skill files behind the installer's receipt. Repository role
and build restrictions still apply. App edits use the repository workflow;
attaching or reading messages does not require repo orientation or a worker.

Use the same `--bus` and `--bridge-home` overrides on every operation when this
session has them. Never print credentials or unfiltered environment/config dumps.
See [FLOW.md](FLOW.md) for the message path.
