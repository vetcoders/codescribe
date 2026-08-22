# codescribe

Foundation skill: how a chat agent plugs into Codescribe.app's transcript bus.
No Vibecrafted worker. The human holds Fn. The agent listens on jsonl.

## Quick reference

| Field            | Value                                                 |
| ---------------- | ----------------------------------------------------- |
| Name             | `codescribe`                                          |
| Version          | `0.2.0`                                               |
| Operator command | **none** — not `vibecrafted codescribe <agent>`       |
| Interactive      | `/codescribe`                                         |
| Canonical doc    | [`SKILL.md`](SKILL.md)                                |
| Follower         | `~/.codescribe/agent-bridge/runtime/bin/bus-demux.py` |

## Homes

| Tree                | Path                                                 |
| ------------------- | ---------------------------------------------------- |
| Codescribe checkout | `skills/codescribe/`                                 |
| Signed app payload  | `Contents/Resources/agent-bridge/skills/codescribe/` |
| Product install     | Codex or Claude skill home + managed receipt/marker  |
| Fleet               | `vibecrafted_core/skills/codescribe/`                |

Keep authoring copies in lockstep. The signed app packages the complete tree and
the Setup Wizard installs it explicitly. Runtime commands use the stable product
helper under `~/.codescribe/agent-bridge/runtime/`, never the checkout.

## Authoring checklist

- [x] Foundation: no fake worker CLI
- [x] Example in `examples/`
- [x] Attach / live-vs-seal in `references/`
- [ ] `make test-skills` from vibecrafted-core when that copy is committed
