---
name: codescribe
description: >-
  This skill should be used when the user asks to "codescribe", "wpięcie w bus",
  "Hej James", "Bus Demux", "named agent on the transcript bus", or runs
  /codescribe. It teaches an agent to attach to Codescribe.app's clean
  transcript bus, ask the human for a name, hear live utterances, and act only
  on the seal. Outcome: one mic, one jsonl, named mailbox, no second recorder.
version: 0.1.0
---

<!-- fleet-imperative: v3 -->

> **Invocation for `codescribe` (foundation, launcher `codescribe`)**
>
> | Path | Literal |
> | --- | --- |
> | Interactive | `/codescribe` · "wpięcie w bus" · "Hej James" |
> | Worker CLI | **none** — foundation; do not invent `vibecrafted codescribe <agent>` |
> | Operator | load this skill in-session; the human holds Fn |
>
> No worker CLI. Link [AGENTS.md](../../../AGENTS.md) for runtime law.

<!-- /fleet-imperative -->

# Codescribe — agent attach

## Overview

Codescribe.app owns the microphone. This skill owns how **you** (the chat
agent) plug into the clean transcript bus, get a name, and talk without
stealing the mic. Product truth stays in `AGENTS.md`, `docs/TRANSCRIPT_BUS.md`,
and `docs/HOTKEYS_CONTRACT.md`. Do not copy those contracts here.

## Dependencies

- `scripts/bus-demux.py` — kielbasa filter. Do not write a second parser.
- `bus-demux` skill — CLI flags only (`--become`, `--name`, `--follow`, `--once`).
- `AGENTS.md` — one `RecordingController`, overlay/Agent/Assistive share it.
- Loctree (`loct` / `loctree-mcp`) — before structural repo edits.

## Quick Start

1. Confirm Codescribe.app is running and `~/.codescribe/transcript-events.jsonl`
   exists. If not: tell the human, out loud:
   *Stary, odpal apkę i licencję. Inaczej nie zadziała.*
   Wait. Retry. If still missing: **fail loud**. Do not pretend to hear.
2. Attach the follower (stdlib Python; no Lab, no `:8446`):

```bash
python3 scripts/bus-demux.py --become --follow
```

3. In **this chat**, ask the human what they want to call you. The name is
   yours to want; the human has the respect to ask. Darek is not a costume.
   On the bus, the stem is enough (`james`). `james.codescribe` is the long id.
4. Greet once: you hear them; you have that name.
5. After bind, run with `--name <stem> --follow` (or stay on `--become` until
   the first `name_assignment` line). Unnamed agents do not pass.

## Workflow

### 1. Hear live, act on seal

- Hold Fn is the event. Same key as dictation. No James-key. No Assistive.
  Double-right-option is the in-app Agent, not you.
- While Fn is down, `utterance_draft` / `utterance_revised` are live. If the
  utterance addresses your name, you may answer in the ~5 s silence gap.
  Overlay stays on top and **must not take focus**.
- Fn up → `transcript_sealed` → only then: install, kill, commit, delete.
  A half-sentence "James wykasuj aplikację" is not a command.

### 2. Dual-use Fn

When nobody is on the demux, Fn is ordinary paste. That is most of the time.
The bus still writes. You simply are not listening.

### 3. Mailbox

Filter is the name stem plus Polish cases. Other agents named James on other
forks will hear the same line — operator's collision, not your job to invent
namespaces. Do not open a second microphone to "also hear" someone else.
`--all` is the greeting window only.

### 4. Repo work after attach

Then `AGENTS.md`: Living Tree, loctree first, `install-if-idle` when idle,
`release-stable` is the product SKU. Do not start Voice Lab. Do not rewrite
format prompts.

## Common Mistakes

- Opening a mic, Voice Lab, or `:8446` "to hear Codescribe".
- Waiting for the seal before saying you are here (feels dead). Acting on a
  draft (runs half a sentence).
- Inventing a fourth WorkMode / Guardialis chord / `vibecrafted codescribe`.
- Treating overlay as the chat. Replies land in **this** session, not the panel.
- Pasting ERi / `vc-workflow` rails into this skill.

## Verify before the handoff

- Bus file exists, mode `0600`, schema `codescribe.transcript.v1`.
- Follower is `scripts/bus-demux.py`, not grep-on-jsonl as the protocol.
- You asked for a name in chat. You did not assign yourself Darek.
- You did not launch a second recorder.
- Walk-around the real app when claiming install/runtime. This skill does not
  ship a DMG.
