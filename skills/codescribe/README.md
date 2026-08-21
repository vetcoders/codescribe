# codescribe

Foundation skill: how a chat agent plugs into Codescribe.app's transcript bus.
No Vibecrafted worker. The human holds Fn. The agent listens on jsonl.

## Quick reference

| Field            | Value                                                      |
| ---------------- | ---------------------------------------------------------- |
| Name             | `codescribe`                                               |
| Version          | `0.1.0`                                                    |
| Operator command | **none** — not `vibecrafted codescribe <agent>`            |
| Interactive      | `/codescribe`                                              |
| Canonical doc    | [`SKILL.md`](SKILL.md)                                     |
| Follower         | Codescribe checkout `scripts/bus-demux.py`                 |
| Codex voice loop | `scripts/codex-voice-bridge.py --name <stem> --cwd <repo>` |

## Homes

| Tree                | Path                                  |
| ------------------- | ------------------------------------- |
| Codescribe checkout | `skills/codescribe/`                  |
| Fleet               | `vibecrafted_core/skills/codescribe/` |

Keep both copies in lockstep. Parser stays in the Codescribe repo.

## Authoring checklist

- [x] Foundation: no fake worker CLI
- [x] Example in `examples/`
- [x] Attach / live-vs-seal in `references/`
- [ ] `make test-skills` from vibecrafted-core when that copy is committed

## Optional Codex voice loop

The foundation attach remains the smallest path. When the operator also wants
the named mailbox to own a dedicated coding task and speak final replies, use
[`docs/CODEX_VOICE_BRIDGE.md`](../../docs/CODEX_VOICE_BRIDGE.md). The bridge
reuses this skill's demux; it does not open another microphone or parser.
