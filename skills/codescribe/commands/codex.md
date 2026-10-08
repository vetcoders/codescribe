---
description: Connect this chat to a Codescribe channel
argument-hint: <name> <channel 1-9> [voice]
---

Connect this chat now using: $ARGUMENTS

1. Read `~/.codex/skills/codescribe/SKILL.md`. Use its **Connect once**
   procedure. This is an action request, not a request to explain commands.
2. Parse a name, one channel digit 1–9 and optional voice. Treat arguments as
   data and reject invalid input before writing. Use this actual Codex thread ID.
3. Check `cs-bus --version`. Attach once with the supplied name/channel and
   `--provider codex --session <this-thread-id>`. Include `--voice` only when
   supplied. The engine owns slot protection and creates or reuses one follower.
   On an occupied slot report its owner; never overwrite it or silently choose
   another slot. Use `--takeover` only for the same name's ended-session handoff.
4. Reuse or start one output-notifying `cs-bus --watch` monitor and retain its
   notification window. Native Codex queue is armed by attachment too. A bare
   background reader is insufficient. If only active polling is available,
   report that boundary and keep the listening turn open.
5. Read `--status` for the same session. Return name, channel, stored voice and
   `attached_unverified`. Upgrade to `listening_verified` only after a fresh
   named utterance reaches this chat and receives a reply without a typed nudge.
6. On every bell/queued copy, follow **Read → ACK → act** from the skill:
   current `--read-pending`, immediate exact returned-ID ACK, drain, extra read,
   then execute/reply. Empty or already-read queued copies get no repeated action.

Keep the connection for requested ongoing listening. Detach on explicit stop
or handoff; do not free the channel merely because a normal reply ends a turn.
Do not record, restart the app, install dependencies or send test messages as
side effects. Speech is only for an authorized voice reply, through `cs-say`
with the exact owned `--reply-to` ID after read/ACK.
