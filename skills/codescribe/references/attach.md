# Attach, naming and recovery

## One-command attach (engine)

With a known name and channel, one call replaces the manual saga below:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --attach --channel <1-9> --name <name> \
  --provider <claude-code|codex|...> --session <provider-session-id> \
  [--voice <voice-id>] [--speed <factor>] [--tts-vendor xai|openai]
```

With `--provider claude-code`, `--session` may be omitted: the helper reads
`$CLAUDE_CODE_SESSION_ID`. Codex exposes no such variable; pass its thread id.
This holds for every command below.

It atomically writes the channel's entry in
`~/.codescribe/agent-bridge/vc.agent-audience-binding.v1.json`, ensures exactly
one live follower for the session's lease (pidfile and readable log under
`agent-bridge/runtime/followers/`), and prints an attach receipt: `lease_id`,
`cursor`, `resumed`, `follower_pid`, `follower_spawned`, `follower_log`,
`follower_events`, and
the `voice` profile from `voices.json`. A second attach with a live follower
reuses it (`follower_spawned: false`). The spawned follower always coalesces
(`--coalesce`): the newest draft/revision replaces its predecessors per
document, while every seal stays its own envelope. Pass
`--on-seal '<cmd>'` to forward a detached wake hook to the spawned follower;
the hook receives `CODESCRIBE_SEAL_DELIVERY_ID`, `CODESCRIBE_SEAL_SESSION_ID`
and `CODESCRIBE_SEAL_TEXT`, and fires exactly once per freshly queued seal.

`--voice`, `--speed` and `--tts-vendor` are stored in this name's profile in
`~/.codescribe/agent-bridge/voices.json` by a locked merge; other names'
profiles and keys stay. Every later `--say` for the name uses it. Pass them only
when the Founder named the voice. The receipt reports `voice_source` (`flag`,
`profile` or `default` = `leo`) and `voices_file` (`present` or `missing`). An
unreadable `voices.json` refuses the attach before the channel is claimed.

An occupied channel refuses a different provider, session or name before starting
any follower. The error identifies the owner and free slots. Repeating the same
binding is idempotent. Claims use an exclusive lock around read/check/write, so
simultaneous callers cannot replace each other or lose distinct-slot updates.
Unreadable bindings refuse writes; they are never treated as an empty map.

Read the channel's one truthful state at any time:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --status --provider <provider> --session <provider-session-id>
```

`--status` reports both `follower_log` (`<lease_id>.log`, human one-line
envelopes) and `follower_events` (`<lease_id>.events.jsonl`, private full JSON).
Tail the `.log` for readable words; never tail `events.jsonl` as a notification
bell. Use `--watch` for compact JSON notifications or `--watch --human` for
readable notifications. `backlog` is pending minus acknowledgment markers; the raw `pending`
length of the lease file overstates it, because acknowledged envelopes stay
in the file until the follower's next sweep. The receipt never proves
listening — verify with a fresh named take before claiming it. Feed the
monitor from `--watch` ([Monitor](monitor.md#watch-stream)).

A take whose ledger refuses terminal finality (incomplete acoustic coverage)
still reaches the mailbox as a seal with `coverage: "refused"` and
`state_change_allowed: false`, once the channel moves on: a silence seal, a
reopen, or the session's `session_ended` row. A hang-up (second digit or Fn
press) releases it only when the app writes one of those rows.

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
