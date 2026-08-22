# Attach and mailbox

## Bus path

1. `CODESCRIBE_TRANSCRIPT_BUS_PATH`
2. `$XDG_STATE_HOME/codescribe/transcript-events.jsonl`
3. `~/.codescribe/transcript-events.jsonl`

Schema `codescribe.transcript.v1`. File mode `0600`. Observer only — no mic.

## Follower

From the stable product install (no checkout dependency):

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --become --drafts --follow
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --name james --drafts --follow
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py --active-names
```

`--become` hears seals until a name assignment, then filters. Unnamed → exit 2.

The first line is an attach receipt. Preserve its `lease_id` and the follower
handle. After provider recovery, poll the original handle; if it is gone,
reattach with the same provider/session/name. A resumed cursor catches events
written during recovery without replaying the old command. Different provider
sessions — even with the same human name — own different leases and cursors.

Do not grep the jsonl as the protocol. The script parses the schema.

## Naming

The human asks the agent what it wants to be called. Bind that stem.
`james` on the bus; `james.codescribe` as the long id. Collisions across forks
are the operator's problem.

## Overlay

On Fn, overlay may be visible. It must not become the key app. Replies land in
the agent session, not the panel.
