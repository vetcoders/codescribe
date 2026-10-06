---
description: Connect this chat to a Codescribe channel
argument-hint: <name> <channel 1-9> [voice]
---

Execute the Codescribe connection now using these arguments: $ARGUMENTS

This is an action command. Complete the connection workflow; do not merely
explain the skill or print commands for the user to run.

1. Parse exactly a name, a single channel digit 1–9, and an optional voice.
   Treat arguments as data, never shell code. Refuse invalid arguments before
   writing anything. Use the actual current Kimi session identity; never borrow
   another conversation's identity or invent one.
2. Read `~/.kimi/skills/codescribe/SKILL.md` and its attach, monitor,
   live-vs-seal and (when voice is supplied) voice-reply references. Reuse
   this session's existing follower and monitor when present.
3. Invoke `cs-bus` once with `--attach --channel <channel> --name <name> --provider kimi --session <actual-session-id>`, adding `--voice <voice>`
   when a voice was supplied. Use structured subprocess arguments or proper
   shell quoting. On an occupied slot, report its owner and available slots.
   Never overwrite, detach another agent, silently choose another slot, or
   retry with a different identity. A slot held by this same name in an ended
   session is the one exception: repeat the call with `--takeover`, which stops
   that session's leftover follower under the binding lock and reports the
   previous owner in the receipt. Release this session's own slot at the end of
   work with `--detach --provider kimi --session <actual-session-id>`.
4. Retain the attach receipt: Kimi has no native inject channel, so it
   automatically selects `--wakeup bell`. The follower appends a compact JSON
   line to `agent-bridge/runtime/followers/<lease_id>.bell.jsonl` for each
   fresh seal or message. Each line carries `kind`, `delivery_id`,
   `emitted_at` and up to 500 characters of `text`; drafts and revised drafts
   do not write a bell line. The follower writes a receipt under
   `agent-bridge/wakeups/<lease-id>/<delivery-id>.json` with schema
   `codescribe.bell.receipt.v1` and disposition `bell_posted`. The agent must
   run `--watch` as a background task with a short bounded window (≤60s) and
   renew it throughout the active task. A bell signals a delivery; read the
   full envelope with `--read-delivery <id>` and ACK after reading. `--ack`
   records acceptance and the bell file itself is the only queue;
   `native_queue_settled: true` means no further withdrawal is needed.
   `cs-bus --retry-wakeup <delivery-id> --provider kimi --session <actual-session-id>` re-appends a bell line only if the receipt does not
   already show `bell_posted`. After the final answer and session death the
   bell no longer wakes the agent. If only active-turn polling is available,
   keep the listening turn open and report `active_polling`; never promise
   replies after the turn ends.
5. Read `--status` for the same provider/session. A supplied voice is stored
   in this name's profile only (receipt `voice_source: "flag"`); other names'
   profiles stay. Later `--say` calls need no `--voice`. Do not speak during a
   live take or merely to test.
6. Return one short result: name, channel, voice and delivery disposition.
   Use `attached_unverified` until a fresh named utterance reaches this chat;
   only then use `listening_verified`. A receipt or running PID alone is not
   successful conversational delivery. Acknowledge accepted envelopes by
   delivery ID, retaining the draft/seal boundary.

Never open the microphone, start recording, restart the app, install packages,
or send test messages to another agent as a side effect of this command.
