# Attach, naming and recovery

## One-command attach (engine)

With a known name and channel, one call replaces the manual saga below:

```bash
cs-bus \
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
spoken message, while every seal stays its own envelope. Queueing that seal
also retires the message's remaining previews, so only the terminal envelope
waits for your acknowledgment. A channel take is
one message: however many PCM documents it holds, it seals once and carries
them as `occurrences`. Pass
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
Unreadable bindings refuse writes; they are never treated as an empty map. When
the channel is held by the same name in a different session, the refusal also
names `--takeover` (below): that is the ended-session case, not a reason to edit
the binding file by hand or to kill a follower manually.

### Handover between sessions of one name

A follower outlives the session that spawned it, and the binding stays with it.
Two flags move a channel from an ended session to the next one of the same name.

```bash
cs-bus \
  --detach --provider <provider> --session <provider-session-id>
```

`--detach` releases every channel bound to this provider session and stops this
session's own follower, verifying its identity exactly as `--attach` does (the
lease names that pid and session, and its command line follows
`--follow --session <this session>`, plus the exact helper, provider, bridge home, bus and recorded process start stamp) and waiting up to five seconds. Like a
takeover, it takes the binding lock and validates the file before anything is
stopped: a binding file it cannot read refuses with
`channel bindings are unreadable or invalid; nothing changed`, a non-zero exit,
the file untouched and the follower still running. The lease file, byte cursor,
pending envelopes and acknowledgment markers stay: they are durable identity a
later session reads on demand. Nothing bound and no follower is not an error —
the receipt then reports `was_attached: false`. `--detach` needs
`--provider`/`--session` and combines with no other command, `--from-file`
included.

The `detach_receipt` (`codescribe.agent-bridge.detach-receipt.v1`) reports
`released_channels`, `lease_id`, `follower_pid`, `follower_state`,
`binding_changed`, `was_attached` (the state this command found; `--status`
owns `attached` for what is true now), `unacked_deliveries` with
`unacked_delivery_ids`, and `binding_path`. That count and those ids are what
`--status` reports as `unacked_seals`: pending sealed takes and typed messages
with no acknowledgment marker, in mailbox order. A draft revision is not a
delivery waiting for a reader and is never listed. A follower that is alive but
unverifiable (`unverified_retained`) or does not exit (`did_not_exit`) refuses
the detach with a non-zero exit and leaves the bindings untouched.

```bash
cs-bus \
  --attach --channel <channel> --name <same-name> \
  --provider <provider> --session <provider-session-id> --takeover
```

`--takeover` is valid only with `--attach`. It claims the channel only when the
current entry carries the same `audience` name (case-insensitive) and a
different provider/session identity; the provider may differ, because the name
is what the Founder speaks to. A different name refuses exactly like any
occupied channel. A free channel, or one this session already owns, is a plain
idempotent attach. The authorization is the explicit flag plus the matching
name, never a dead reader.

The claim holds the binding lock across verification, retirement, new-reader
startup and binding publication. The previous reader must have a saved cursor
covering the verified logical bus extent and no unfinished channel document.
Unread source rows refuse retirement; resume the old reader to drain them first.
Pending envelopes remain in its mailbox and do not block handover once the
source has been consumed. Missing or malformed bound-owner recovery state also
refuses.

The previous follower is verified against the lease's recorded process start
stamp and full invocation: interpreter, helper, provider, session, bridge home
and bus. A stale heartbeat does not establish process death. Existing readers
without that recorded identity refuse; no unverified PID is signaled. States
include `stopped`, `not_running`, `unverified_retained`, `did_not_exit`,
`undrained_retained` and `undrained_stopped`. The last state means the reader
exited but unread source remains; routing is still retained.

The new channel entry is assigned only after the new reader is verified ready.
Log, spawn, pidfile or readiness failures keep the old binding; the old reader
may already be stopped and is not automatically restarted. If an atomic binding
replacement raises after publication, restoration is conditional on the current
document still matching this operation's attempted write. A confirmed restoration
reports `binding_changed: false` and `binding_state: "unchanged"`; an unconfirmed
restoration reports `binding_changed: null` and `binding_state: "uncertain"`.
Never interpret an uncertain receipt as permission to repeat a delivered command.

Both outcomes print a JSON receipt with a `previous` object — `provider`,
`provider_session_id`, `lease_id`, `audience`, `follower_pid`,
`follower_state`, `unacked_deliveries`, `unacked_delivery_ids` — plus
`binding_changed`. The unacknowledged count and ids are the ones `--status`
reports as `unacked_seals`: pending sealed takes and typed messages with no
acknowledgment marker, in mailbox order; draft revisions are not listed.
Success extends the normal `attach_receipt` with those two fields; a refusal
emits a `takeover_receipt`
(`codescribe.agent-bridge.takeover-receipt.v1`), writes the one-line reason to
stderr in the `bus-demux: attach failed: ...` form, and exits non-zero.
`binding_changed` is true only when another session's entry was rewritten.

The previous lease is never moved, replayed, acknowledged or rewound. Read one
inherited envelope explicitly, only when the Founder asks for it:

```bash
cs-bus \
  --read-delivery <delivery-id> --lease <previous-lease-id> \
  --provider <provider> --session <provider-session-id>
```

This succeeds only while the caller's session owns a channel bound to the same
name as that lease. It acknowledges nothing, advances no cursor and mutates
nothing; the envelope is marked with `inherited_from` (previous provider,
session, lease id, name) so it is never mistaken for a fresh request. Report
inherited unacknowledged deliveries to the Founder; never execute them
automatically. Acknowledging a delivery on behalf of the previous lease is not
supported.

Read the channel's one truthful state at any time:

```bash
cs-bus \
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
cs-bus --help
cs-bus --print-bus-path
cs-bus --active-names
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
cs-bus \
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
cs-bus \
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

End of work or handoff: run `--detach` so the digit is free for the next
session, or state in the handoff that the channel stays bound and to which
provider and session. Entering a session from a handoff that names a channel:
read `--status`, check for a running follower, then attach with the same name
and `--takeover`, and verify with a fresh named take before claiming anything
is heard. Takeover is for the same agent name only; another agent's channel is
never claimed this way.
