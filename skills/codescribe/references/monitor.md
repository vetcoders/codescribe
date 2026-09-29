# Monitor and agent wakeup

## Select the execution mechanism

Inspect tools available in this provider session before launching a listener.
Use its documented event-driven background monitor or notification facility
that delivers output to the agent and resumes processing without human input.
Verify whether it survives a final answer, whether it notifies on output or
only process exit, and how it is cancelled. Do not infer these properties from
the word "background" or invent an API name.

An infinite follower needs output notifications. An exit-only notification
cannot be treated as live delivery while that follower keeps running.
A tool session id, running PID, terminal tab, or successful attach receipt
does not prove any notification behavior.

Retain both the follower handle and the monitor handle with the same provider
session and lease. Notifications must use the existing named follower's
envelopes, preserving delivery identity and draft/seal permission.

## Verify independently

Use a fresh utterance containing the bound name. Check these hops separately:

1. The bus received the utterance.
2. The named follower emitted an envelope for this provider session.
3. The monitor delivered that event into the agent's active context.
4. The agent replied without a typed "hello?" or a later user-triggered poll.

If 1 and 2 pass but 3 fails, report missing conversational delivery.
Casing changes or more tails do not address that boundary.

## No wake-capable tool available

Report the limitation immediately. While actively listening, keep the turn
open and poll the original follower with bounded waits no longer than 60 seconds,
respecting provider limits. Handle new input promptly. This mode is
`active_polling`, not automatic wakeup.

If the turn ends, state that the background reader cannot itself resume this
conversation. Do not promise unattended voice replies or silently configure
another provider runtime. An attach-only setup remains `attached_unverified`.

## Active-turn notifications with functions.exec

When this provider exposes `functions.exec`, `tools.write_stdin` and
`notify`, use a yielded, bounded exec loop to read the existing follower and
notify on each new envelope while other work proceeds. Do not await an
infinite follower's exit. Use the shortest supported read wait, retain partial
JSON lines, and preserve delivery_id, session_id, text and state_change_allowed.
Deduplicate delivery IDs across notification windows, not just within one read.

Retain the exec cell separately from the follower session. Renew completed
windows on that same follower. After interruption or context recovery, verify
that new output actually arrives; a cell still labelled running is not proof.
Stop an unresponsive owned notification cell before replacing its reader, so
two loops do not consume the same stream.

This mechanism has demonstrated delivery during an active turn. It does not
establish wakeup after a final answer. Report that distinction and keep an
active listening turn open when post-final wakeup is unavailable.

## Acknowledge conversation receipt

The session-scoped helper retains emitted envelopes until explicit receipt.
After this conversation has received and accepted the complete envelope,
retain its delivery ID and disposition in the conversation record, then run:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session SESSION_ID --ack DELIVERY_ID
```

Use the actual provider/session and the same `--bus`/`--bridge-home` overrides
as the follower. This command does not start another reader. A successful
`acknowledged` receipt proves acceptance was recorded, not that a command was
executed. Keep execution disposition separately; do not repeat a completed
action when its envelope is replayed.

Acknowledge accepted drafts and routing-ambiguity notices too, without
promoting them to permission for state changes. Report ambiguous recipients
and ask for a clear address instead of choosing one. Never acknowledge from
the stdout pump before the conversation receives the message, from a delivery
ID alone, or after a truncated/incomplete tool result. Unaccepted envelopes
remain in the lease's ordered `pending` array and replay on reattachment.

At 256 pending envelopes or 8 MiB, continuous following waits for receipts
and resumes after space is freed. A non-following read exits 4; acknowledge
accepted items and resume the same lease. Do not clear the queue by deleting
lease files, creating another session, or acknowledging unread messages.
If the installed helper lacks `--ack`, report a generation mismatch; do not
pretend its stdout is durable conversational delivery.

## Diagnostic readers (observation only)

`scripts/bus-tail.sh --all` combines the bus and application log for diagnosis.
Its human view may truncate text; it is not an agent notification mechanism.
Additional readers are observers, never alternate command-delivery authorities.
Keep any requested diagnostic output scoped and redact secrets.
