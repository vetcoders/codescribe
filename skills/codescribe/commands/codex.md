---
description: Connect this chat to a Codescribe channel
argument-hint: <name> <channel 1-9> [voice]
---

Execute the Codescribe connection now using these arguments: $ARGUMENTS

This is an action command. Complete the connection workflow; do not merely
explain the skill or print commands for the user to run.

1. Parse exactly a name, a single channel digit 1–9, and an optional voice.
   Treat arguments as data, never shell code. Refuse invalid arguments before
   writing anything. Use the actual current Codex thread/session identity;
   never borrow another conversation's identity or invent one.
2. Read `~/.codex/skills/codescribe/SKILL.md` and its attach, monitor,
   live-vs-seal and (when voice is supplied) voice-reply references. Reuse
   this session's existing follower and monitor when present.
3. Verify the installed helper at
   `~/.codescribe/agent-bridge/runtime/bin/bus-demux.py` has the occupied-slot
   protection in `write_channel_binding`: a stable sibling lock, owner check,
   and refusal before any follower starts. If absent, stop with
   `connection_refused: installed helper lacks channel ownership protection`.
   Do not substitute a racy read-before-write check in this prompt.
4. Invoke the helper once with `--attach --channel <channel> --name <name>
   --provider codex --session <actual-session-id>`, adding `--voice <voice>`
   when a voice was supplied. Use structured subprocess arguments or proper
   shell quoting. On an occupied slot, report its owner and available slots.
   Never overwrite, detach another agent, silently choose another slot, or
   retry with a different identity.
5. Retain the attach receipt and arm a supported wake mechanism for its one
   follower. Verify the mechanism actually delivers into this conversation.
   If only active-turn polling is available, keep the listening turn open
   and report `active_polling`; never promise replies after the turn ends.
6. Read `--status` for the same provider/session. A supplied voice is stored
   in this name's profile only (receipt `voice_source: "flag"`); other names'
   profiles stay. Later `--say` calls need no `--voice`. Do not speak during a
   live take or merely to test.
7. Return one short result: name, channel, voice and delivery disposition.
   Use `attached_unverified` until a fresh named utterance reaches this chat;
   only then use `listening_verified`. A receipt or running PID alone is not
   successful conversational delivery. Acknowledge accepted envelopes by
   delivery ID, retaining the draft/seal boundary.

Never open the microphone, start recording, restart the app, install packages,
or send test messages to another agent as a side effect of this command.
