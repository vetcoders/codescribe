---
name: codescribe
version: 0.2.0
description: >-
  This skill should be used when the user asks to "codescribe", "wpięcie w bus",
  "Hej James", "Bus Demux", "named agent on the transcript bus", or runs
  /codescribe. It teaches an agent to attach to Codescribe.app's clean
  transcript bus, ask the human for a name, hear live utterances, and act only
  on the seal. Outcome: one mic, one jsonl, named mailbox, no second recorder.
loctree_value: "primary repo map for structural/literal repository work"
aicx_value: "intent, session, and decision-context retrieval"
dogfooding: "required for repo-impacting work"
---

<!-- fleet-imperative: v3 -->

> **Invocation for `codescribe` (foundation, launcher `codescribe`)**
>
> Not a core `vibecrafted codescribe <agent>` worker. Load interactively.
> See [Foundation skills](../DELEGATION_MATRIX.md#foundation-no-core-vibecrafted-name-agent-worker-of-their-own)
> when this copy lives under vibecrafted-core.
>
> | Path        | Literal                                                   |
> | ----------- | --------------------------------------------------------- |
> | Worker CLI  | **none** — do not invent `vibecrafted codescribe <agent>` |
> | Interactive | `/codescribe` · "wpięcie w bus" · "Hej James"             |
> | Operator    | load this skill in-session; the human holds Fn            |
>
> No worker CLI. Codescribe checkout runtime law: `AGENTS.md`.

<!-- /fleet-imperative -->

# Codescribe — agent attach

## Operator Entry

### Living Tree / Worktree Rule

This workflow runs in the operator's current checkout and current branch. Do not
create implementation worktrees for Codescribe. Re-read files before editing.
See [Living Tree Rule](../LIVING_TREE_RULE.md) when this copy lives under
vibecrafted-core; otherwise `AGENTS.md` at the Codescribe repo root.

## Repository Work Doctrine

For repository work, start with Loctree: `loct context`, `loct slice`,
`loct find --literal`. AICX for intent. grep is a local magnifier. Loctree
miss → append `~/.vibecrafted/loctree/loctree-fail.md`.

## Purpose

Teach **this chat agent** to plug into Codescribe.app's clean transcript bus,
receive a name from the human, hear live utterances, and perform side effects
only on `transcript_sealed`. One microphone. One jsonl. Named mailbox.

The Codescribe Setup Wizard is the product installer. This skill never writes
itself into a client and never invents another settings plane. It is not Voice
Lab and not a fourth WorkMode.

## When To Use

- A new agent session in a Codescribe checkout needs to hear the operator's
  Hold Fn takes
- The operator says "Hej James", "wpięcie w bus", "Bus Demux", or `/codescribe`
- Multi-agent mailbox routing on `codescribe.transcript.v1`

**When NOT to use:**

- In-app Agent / Assistive (double-right-option, `⌘⇧Space`) — that is Codescribe UI
- `vc-init` / `vc-implement` / `vc-justdo` for repo surgery after you are already attached
- Inventing `vibecrafted codescribe <agent>` or a James-key

## Pipeline Position

- Upstream: human launched Codescribe.app (license on). Optional `vc-init` if
  the session will also edit the repo.
- Downstream: ordinary repo skills (`vc-justdo`, `vc-implement`, …) after attach.
- Not a ship-cycle stage.

## Dependencies

- Stable installed helper:
  `~/.codescribe/agent-bridge/runtime/bin/bus-demux.py` (kielbasa filter).
  Do not depend on a Codescribe checkout, write a second parser, or MCP Voice Lab.
- Codescribe contracts: `AGENTS.md`, `docs/TRANSCRIPT_BUS.md`, `docs/HOTKEYS_CONTRACT.md`
- Loctree / `vc-loctree` before structural edits
- `vc-aicx` when recovering a past naming or bus decision

## Quick Start

1. Confirm Codescribe.app is running and `~/.codescribe/transcript-events.jsonl`
   exists. If not, tell the human: _Stary, odpal apkę i licencję. Inaczej nie
   zadziała._ Wait. Retry. Still missing → **fail loud**. Do not pretend to hear.
2. Choose the client token (`codex` or `claude-code`) and this provider's stable
   session/thread id. Attach the installed follower with drafts enabled:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --become --drafts --follow
```

Preserve the running tool handle. The first JSON line is an attach receipt;
retain its `lease_id` with the provider session. 3. In **this chat**, ask the human what they want to call you. The name is
yours to want; they have the respect to ask. Darek is not a costume.
Bus stem is enough (`james`). Long id: `james.codescribe`. 4. Greet once: you hear them; you have that name. 5. Bind the same provider session:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --name <stem> --drafts --follow
```

Unnamed agents do not pass (exit 2).

## Workflow

### 1. Hear live, act on seal

Hold Fn is the event. Same key as dictation. No James-key. Double-right-option
is in-app Agent, not you. Overlay stays on top and **must not take focus**.

While Fn is down, `utterance_draft` / `utterance_revised` are live. If the
utterance addresses your name, you may answer in the ~5 s silence gap. Their
envelopes say `state_change_allowed: false`.
Fn up → `transcript_sealed` with `state_change_allowed: true` → only then:
install, kill, commit, delete.
A half-sentence "James wykasuj aplikację" is not a command.

Detail: [`references/live-vs-seal.md`](references/live-vs-seal.md).

### 2. Dual-use Fn

When nobody is on the demux, Fn is ordinary paste. That is most of the time.
The bus still writes. You simply are not listening.

### 3. Mailbox

Name stem plus Polish cases. Other Jameses on other forks hear the same line —
operator collision, not your namespace to invent. `--all` is the greeting
window only. Detail: [`references/attach.md`](references/attach.md).

### 4. Provider recovery

Keep polling the original follower handle through ordinary provider turns. If
the provider compacts or recovers and the handle is gone, rerun the installed
helper with the same `--provider`, `--session`, name, and `--drafts`. Its lease
resumes from the persisted byte cursor rather than jumping to EOF or replaying
an old command. An attach receipt with `resumed: true` proves recovery.

If the old follower is still alive, the helper refuses a duplicate process;
poll the original handle. Never create a second cursor for the same provider
session. Duplicate human names remain isolated because provider + session own
the lease. Inspect non-stale names for future acoustic routing with:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py --active-names
```

### 5. Repo work after attach

Then `AGENTS.md`: Living Tree, loctree first, `install-if-idle` when idle,
`release-stable` is the product SKU. Do not start Voice Lab. Do not rewrite
format prompts.

## Acceptance Criteria

The attach run is **done** when:

- [ ] Bus file exists, schema `codescribe.transcript.v1`
- [ ] Follower is the installed `~/.codescribe/agent-bridge/runtime/bin/bus-demux.py`, not a second mic
- [ ] You asked for a name in chat and bound `--name <stem>`
- [ ] Attach receipt names provider/session/lease and the running handle is preserved
- [ ] Live command includes `--drafts`; state changes still wait for the seal
- [ ] You greet in this session, not in the overlay
- [ ] You have not launched Voice Lab / `:8446` / a recorder

## Anti-Patterns

- Opening a microphone to observe Codescribe
- Waiting for the seal before saying you are here; acting on a draft
- Inventing a fourth WorkMode / Guardialis chord / `vibecrafted codescribe`
- Treating overlay as the chat
- Pasting ERi / `vc-workflow` rails into this skill
- Assigning yourself Darek

## Examples

See [`examples/example-prompt.md`](examples/example-prompt.md).

## Verify before the handoff

Walk around the truck — [Verification Rule](../VERIFICATION_RULE.md) when this
copy lives under vibecrafted-core. This skill does not ship a DMG. Prove attach
by a Hold Fn take that contains your name and a reply in the gap.

---

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_
