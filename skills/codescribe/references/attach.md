# Attach and mailbox

## Bus path

1. `CODESCRIBE_TRANSCRIPT_BUS_PATH`
2. `$XDG_STATE_HOME/codescribe/transcript-events.jsonl`
3. `~/.codescribe/transcript-events.jsonl`

Schema `codescribe.transcript.v1`. File mode `0600`. Observer only — no mic.

## Follower

From the Codescribe checkout:

```bash
python3 scripts/bus-demux.py --become --follow
python3 scripts/bus-demux.py --name james --follow
python3 scripts/bus-demux.py --name james --once
```

`--become` hears seals until a name assignment, then filters. Unnamed → exit 2.

Do not grep the jsonl as the protocol. The script parses the schema.

## Naming

The human asks the agent what it wants to be called. Bind that stem.
`james` on the bus; `james.codescribe` as the long id. Collisions across forks
are the operator's problem.

## Overlay

On Fn, overlay may be visible. It must not become the key app. Replies land in
the agent session, not the panel.
