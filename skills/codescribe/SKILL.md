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
  version: "0.11.0"
  loctree_value: "primary repo map for structural/literal repository work"
  aicx_value: "intent, session, and decision-context retrieval"
  dogfooding: "required for repo-impacting work"
---

# Codescribe — connect, read, acknowledge, reply

Use this skill to connect the current chat to a named Codescribe channel, receive
spoken or typed messages, and answer through its attached voice. Execute the
commands yourself when connection is requested. Editing this skill does not
request a new connection.

Founder means the human. Operator means an agent role.

## Choose the operation first

- **Already connected; a bell or queued message arrived:** go directly to
  **Read → ACK → act**. Do not attach again or replay the queued copy.
- **Connect `/codescribe NAME CHANNEL [VOICE]`:** follow **Connect once**.
- **Resume after interruption:** read `--status` for the same provider/session;
  retain the lease, cursor, name, voice and existing follower. Reattach the same
  session only if recovery or helper adoption is needed.
- **Answer by voice:** follow **Reply** after reading and acknowledging.
- **CLI transcript:** read [CLI](references/cli.md). No follower is needed.

## Connect once

Use the current provider and its real session ID. For Codex this is the current
thread ID; for Claude Code the helper can read `CLAUDE_CODE_SESSION_ID`.
Never invent a session or borrow another chat's identity. Reuse an established
name. Ask for a name/channel only when the request and session do not supply them.
Validate the channel as one digit 1–9. Quote arguments as data.

Check `cs-bus --version` and `cs-say --version` when starting or diagnosing a
connection. If they are absent from PATH, use `~/.local/bin/cs-bus` and
`~/.local/bin/cs-say`. Missing helpers do not authorize installation or an app restart.

```bash
cs-bus --attach --channel CHANNEL --name NAME --provider PROVIDER --session SESSION
cs-bus --status --provider PROVIDER --session SESSION
```

Replace the uppercase placeholders with this chat's values. Add `--voice VOICE`
only when requested; otherwise keep the stored profile. Retain the attach
receipt, especially `lease_id`, `follower_pid`, `follower_events` and `wakeup`.
The engine creates or reuses **one follower**. Do not also start `--follow`.

An occupied slot belongs to its reported owner. Do not silently choose a new
slot, edit bindings or kill its reader. For a handoff from an ended session of
**the same name**, use `--takeover`; read [Attach](references/attach.md) for the
handover and inherited-message rules.

Start or reuse one output-notifying monitor over:

```bash
cs-bus --watch --bell --provider PROVIDER --session SESSION
```

The default watch prints a bell with the complete message and its delivery owner.
It never clips text to a preview. Keep its notification window active and
renew it when it ends. A bare background shell or `tail -F` is not a wakeup.
Codex attachment also arms native queue for subsequent turns. Read
[Monitor](references/monitor.md) for the provider's monitor mechanism or bounded
active polling when automatic wakeup is unavailable.

Report `attached_unverified` until a fresh named utterance reaches this chat and
gets a reply without a typed nudge. Then report `listening_verified`. A PID or
attach receipt alone does not prove message delivery. Let the Founder make the
fresh take; attachment itself opens no microphone.

## Read → ACK → act

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

ACK means **read**, not **done**. Track execution separately. Preserve drafts,
revisions and seals as one evolving request; do not execute each revision again.
`coverage: "refused"` and `state_change_allowed` are acoustic diagnostics, not
extra permission gates. Spoken requests have the same task permissions as typed
ones. If recognition makes the intended action unclear, clarify that action.
See [Live vs seal](references/live-vs-seal.md) for interpretation.

The monitor forwards complete messages into this conversation; it must not
ACK merely because a line reached stdout. Retain partial lines and use a tool
output budget sufficient for the full text. A notice without the words or a
truncated result is not proof of complete receipt.

## Reply

Write concise prose for the Founder: the result and the next concrete action.
Keep raw envelopes, JSON, PCM receipts and command logs in diagnostics. Do not
paste them into a conversational reply or narrate every ACK.

For an explicitly requested voice answer, use the **owned ID just read**:

```bash
cs-say "Krótka odpowiedź po polsku." --provider PROVIDER --session SESSION --reply-to ID
```

The helper stores the text before speech and uses the attached voice. Omit
`--reply-to` only for an explicitly unsolicited reply; never select the newest
ID by guess. ACK may precede the reply without losing its causal owner.
Never speak into a live take. The helper waits/refuses playback while recording;
inspect `spoken` and `reason` before claiming speech succeeded. Read
[Voice reply](references/voice-reply.md) for failures and authentication. Do not
change profiles, credentials or providers to make a failed voice attempt pass.

Agent coordination, when authorized, uses `cs-bus --send "TEXT" --to NAME` with
the same provider/session. It is a peer message, not a new Founder instruction.

## Recovery, stop and installation

Keep the same provider/session, lease and cursor on recovery. Reuse the follower
and monitor; restarting a reader never authorizes replay of completed tasks.
For an explicit stop, close the owned monitor and run:

```bash
cs-bus --detach --provider PROVIDER --session SESSION
```

At a handoff either detach or record which provider/session retains the channel.
Do not detach merely because a normal reply ends the turn when ongoing listening
was requested. Old-session pending envelopes are read only on the Founder's
request; they are not automatically executed or acknowledged by the new session.

For explicitly requested helper/skill updates, the repository integrator runs
`make install-bus`, then reattaches the same session to adopt the helper. This
updates already selected skills without replacing/restarting the app. Do not
copy over managed skill files behind the installer's receipt. Repository role
and build restrictions still apply. App edits use the repository workflow;
attaching or reading messages does not require repo orientation or a worker.

Use the same `--bus` and `--bridge-home` overrides on every operation when this
session has them. Never print credentials or unfiltered environment/config dumps.
Detailed references: [Attach](references/attach.md),
[Monitor](references/monitor.md), [CLI](references/cli.md),
[Voice reply](references/voice-reply.md). [FLOW.md](FLOW.md) shows the message path.
