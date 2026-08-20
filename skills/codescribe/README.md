# codescribe

Foundation skill: how a chat agent plugs into Codescribe.app's transcript bus.
No Vibecrafted worker. The human holds Fn. The agent listens on jsonl.

## Quick reference

| Field | Value |
| --- | --- |
| Name | `codescribe` |
| Version | `0.1.0` |
| Operator command | **none** — not `vibecrafted codescribe <agent>` |
| Interactive | `/codescribe` |
| Canonical doc | [`SKILL.md`](SKILL.md) |
| Follower | Codescribe checkout `scripts/bus-demux.py` |

## Homes

| Tree | Path |
| --- | --- |
| Codescribe checkout | `skills/codescribe/` |
| Fleet | `vibecrafted_core/skills/codescribe/` |

Keep both copies in lockstep. Parser stays in the Codescribe repo.

## Authoring checklist

- [x] Foundation: no fake worker CLI
- [x] Example in `examples/`
- [x] Attach / live-vs-seal in `references/`
- [ ] `make test-skills` from vibecrafted-core when that copy is committed
