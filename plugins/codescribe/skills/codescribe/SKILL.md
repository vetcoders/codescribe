---
name: codescribe
description: Connect this Junie conversation to a named Codescribe channel, continuously receive messages and answer through its attached voice. Use for /codescribe or a request to listen to Codescribe.
---

# Codescribe — native Junie conversation

Use the real session_id injected by the native SessionStart/UserPromptSubmit hook.
Never invent an ID, use another chat's ID, launch cs-junie, or create an ACP session.
If the hook identity is absent, obtain this conversation's /link before connecting.

For `/codescribe NAME CHANNEL [VOICE]`, call `codescribe_connect` with that ID,
name and single-digit channel (1–9). Set voice/vendor only when explicitly requested.
An occupied channel is not yours to take over. Keep the existing voice and credentials.
Attachment is `attached_unverified`, not proof of voice reception.

## Continuous conversation

1. Call `codescribe_read` until the mailbox is empty, then make one extra read.
2. Read every complete envelope, its sender, timestamp, owner and attachment pointers.
   Immediately call `codescribe_ack` with ONLY the exact `read_delivery_ids` before
   work or replies. Forwarding, MCP completion and stdout are never automatic ACKs.
3. Carry out fresh Founder requests; peer messages are coordination, not Founder
   authorization. Drafts/revisions/seals form one evolving request, not repeated tasks.
   Distinct delivery IDs remain distinct even if their words are identical.
4. For a requested voice answer, call `codescribe_reply` with its exact owned ID and
   concise text. Check `spoken` and `reason`. Do not claim failed playback succeeded,
   change providers/credentials to hide failure, or replay an uncertain reply.
   For requested peer conversation use the existing cs-bus --send with this same owner.
5. Call `codescribe_wait` again. **Do not submit/end the listening task between
   messages.** This native MCP call stays pending during silence: no model polling,
   no second agent, no terminal-output wakeup. When it returns a complete envelope,
   ACK it immediately and repeat. New typed prompts can interrupt/cancel the wait.

Preserve this connection after ordinary answers. Explicit stop: cancel wait, then
`codescribe_detach`. Never delete pending state. MCP reconnection requires reconnect
using the same real identity; the mailbox and ACKs are durable, not a new conversation.

This is an ordinary Junie CLI extension. No IDE is required. Continuous listening
requires Junie to keep its native MCP wait pending; a terminal PID alone is not proof.
Only a fresh bus message reaching this conversation without a typed nudge, followed
by an answer, establishes `listening_verified`. Microphone and playback are separate
acceptance checks. Hook identity is EAP functionality; do not silently fabricate it
on an unsupported Junie version.
