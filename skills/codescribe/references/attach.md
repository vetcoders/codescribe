# Attach, naming and recovery

## Preflight

Confirm the app is running and inspect the installed helper:

```bash
pgrep -x Codescribe
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py --help
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py --print-bus-path
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py --active-names
```

Use the printed bus path. Resolution is explicit override, then
`CODESCRIBE_TRANSCRIPT_BUS_PATH`, then
`$XDG_STATE_HOME/codescribe/transcript-events.jsonl`, then
`~/.codescribe/transcript-events.jsonl`. Inspect recent schema names without
dumping transcripts. If the app or bus is unavailable, explain the missing
prerequisite; do not claim listening.

Current schema families include `codescribe.transcript.v1` and
`codescribe.transcript-evidence.v1`. Source text mentioning a schema is a
preflight hint, not proof of parsing. Prove support through a real emitted
envelope. If the installed helper cannot read the schema, an inspected checkout
`scripts/bus-demux.py` may be selected explicitly; report the exact path.
Reinstallation is a separate action, not an automatic attach step.

## Name and lease

Reuse a name already chosen in this conversation. Otherwise ask once; if the
Founder delegates naming, choose a pronounceable name and proceed.
The helper normalizes names; inspect the attach receipt rather than assuming
case-sensitive routing. Human display name Roman can have bus stem `roman`.
Do not promise inflected-name matching without testing the actual helper.

Obtain the real stable provider session/thread id from the provider's exposed
session context. Do not invent an id or reuse another conversation's lease.

With a known name, attach directly:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> \
  --name roman --drafts --follow
```

Use the provider token actually in use (`codex` or `claude-code`).
The command is the follower; connect it to the mechanism in
[Monitor](monitor.md). A shell handle alone is insufficient.

For a temporary greeting window while choosing a name, use `--become`
instead of `--name`. Before binding the name, stop that follower and wait for
its handle to close. Reattach with the same provider session and receipt:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --lease <lease-id> \
  --name roman --drafts --follow
```

## Recovery and stop

Retain helper path, bus path, provider/session, lease id, cursor, name, follower
handle and monitor handle. If the follower lives, reuse it; never create a
duplicate for the same lease. If its handle is gone, reattach with the same
identity. `resumed: true` proves cursor recovery, not notification delivery.
Reverify the monitor separately.

Preserve the established delivery identity when recovering; do not execute a
previously handled command again. On an explicit stop, close the owned monitor
and follower using their handles. Do not broadly kill the app or other readers.
