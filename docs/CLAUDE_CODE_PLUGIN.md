# Codescribe plugin for Claude Code

The Codescribe plugin connects **the Claude Code conversation you are in** to a
named Codescribe channel. Messages arrive in that conversation without a typed
nudge, and silence costs no model turn. The same plugin serves Junie
([Junie](JUNIE_PLUGIN.md)); this page covers Claude Code.

## Install and connect

```sh
make install-bus
```

The installer ships the plugin with the helpers into
`~/.codescribe/agent-bridge/runtime/plugins`. In Claude Code:

```text
/plugin marketplace add ~/.codescribe/agent-bridge/runtime/plugins
/plugin install codescribe@codescribe
/codescribe roman 3
```

The command line equivalent is `claude plugin marketplace add PATH` and
`claude plugin install codescribe@codescribe`. Claude Code reads a local
marketplace in place, so a later `make install-bus` updates the plugin; use
`/reload-plugins` in a running session. When the user-level `codescribe` skill is
also installed, `/codescribe` routes to this plugin's skill
(`/codescribe:codescribe`) whenever the `codescribe_*` tools are present.

Requirements: Python 3.10+ in `~/.local/bin`, `/opt/homebrew/bin`,
`/usr/local/bin` or `PATH`. Every plugin launcher puts those directories first, so
a pinned or refusing `python3` earlier on `PATH` does not break the plugin. The
installed helpers start as `python3 -E` and remove every `PYTHON*` variable before
spawning children, so a host that leaks its own interpreter's `PYTHONHOME` or
`PYTHONPATH` (seen 2026-10-15 from a Vibecrafted launcher pointing at a 3.14
stdlib) cannot kill them before their first line. The identity hook runs
`session.py` with `python3 -E` for the same reason.

## Identity

A `SessionStart` hook (it also runs on resume, `/clear` and compaction) gives the
conversation `provider=claude-code` and its real `session_id`. Claude Code exports
`CLAUDE_CODE_SESSION_ID` to every hook; the hook trusts only the agreement of that
variable with the hook payload. `codescribe_connect` takes the provider and
session explicitly. The canonical provider token is `claude-code`; `claude` is a
different lease and is never produced by the plugin.

## How listening works

After `codescribe_connect` the agent **ends its turn**. The plugin's `Stop` and
`SessionStart` hooks run `cs-bus --claude-code-wake` as Claude Code `asyncRewake`
hooks:

- The hook process waits in the background on the lease file's OS event. No model
  request is made while the mailbox is silent.
- For a delivery not yet announced it exits 75, which the hook command maps to the
  wake code 2. Claude Code then starts a turn with a `codescribe_mailbox_ready`
  notice. Exit 0 (no channel, another waiter holds the lock, detached) is silent.
  Any other exit, including a helper that crashed or cannot parse the hook
  command, never wakes the model: the hook exits 1, so Claude Code shows the
  helper's last stderr and `Codescribe: wake helper failed (exit N)` as a hook
  error, and appends `TIMESTAMP FLAG exit N: LAST_STDERR_LINE` to
  `~/.codescribe/agent-bridge/runtime/hook-failures.log`. The `PostToolUse` probe
  stays exit 0 and writes the same receipt. An empty mailbox and a dead helper no
  longer look alike.
- The agent reads with `codescribe_read`, ACKs the exact `read_delivery_ids`,
  works and replies, then ends the turn. The next `Stop` re-arms the hook; one
  lock per lease keeps a single waiter.
- The same waiter command also runs as a second `PostToolUse` hook, so the first
  tool call of a turn that a wake started arms the next waiter. Claude Code runs
  no `Stop` hook after a user interrupt and runs `StopFailure` instead after an
  API error; before this, such a turn left the conversation without a waiter until
  someone typed. With a waiter already armed the extra hook returns at once.
- A `cs-bus --watch --until-event` background task of the same lease takes
  precedence while it lives: the task holds `runtime/native-wake/LEASE.watch.lock`
  shared and records what it reported in the announced ledger before it exits.
  The waiter stays armed but announces no delivery while that mark is held, and
  an idle waiter ends silently instead of spending a turn on its six-hour re-arm.
  One message wakes the conversation once; when the task is gone the waiter takes
  over and announces anything the task never reported.
- Announced IDs stay recorded until acknowledged, so a message left unread does
  not wake every turn. A lost follower or refused mailbox is announced once as
  `codescribe_listener_problem`. After six silent hours the hook wakes once with
  `codescribe_wait_idle` only to re-arm. Detach or Claude Code exit ends it quietly.
- The hook never reads, ACKs or replies. `codescribe_status` reports
  `native_wake_armed`.
- Inside a running turn the `PostToolUse` hook runs `cs-bus --claude-code-probe`
  after every tool call. With nothing unannounced it prints nothing and exits 0
  (one lease read, no model cost). With an unannounced delivery it prints the
  `codescribe_mailbox_ready` notice as `additionalContext`, so the model sees it
  after the current tool call instead of after the whole turn, and records the ID
  in the same announced ledger so the Stop waiter stays quiet about it. The
  probe never waits, reads, ACKs or replies; mailbox problems remain the
  waiter's to announce. The interruption lands after the running tool call
  returns, so a long build still finishes first.

`codescribe_wait` remains available for an explicitly requested listening turn. In
Claude Code it blocks typed prompts until it returns and ends after at most
900 seconds with a `wait_idle` receipt; a newer wait supersedes an abandoned one.

A new session id after `/clear` is a new mailbox: drain and ACK first, then
`codescribe_detach` and connect with the new identity.

## Measured client limits (Claude Code 2.1.296)

| Mechanism                 | Limit                               | Consequence                     |
| ------------------------- | ----------------------------------- | ------------------------------- |
| MCP stdio tool call, idle | 30 min without response or progress | `codescribe_wait` ends at 900 s |
| MCP tool call, hard       | `MCP_TOOL_TIMEOUT` or 1e8 ms        | not binding                     |
| Command hook              | `timeout`, default 600 s            | wake hook sets 21660 s          |
| `Monitor` tool            | expires after at most 30 min        | not used for idle wakeup        |
| Background `Bash`         | stopped at its timeout, at most 2 h | CLI lane only                   |

## Verification

```sh
python3 -m unittest scripts/tests/test_bus_mcp.py
python3 scripts/tests/claude-code-plugin-live.py --keep /tmp/claude-plugin-live
```

The hermetic suite is part of `make verify`. It runs the whole native contract as
both `junie` and `claude-code`, the wake against real lease files, and the exact
hook commands under bash and zsh. The live script is an operator-class acceptance:
it runs `claude -p` with this checkout's plugin against an isolated bridge home and
bus, connects through the hook identity, ends the turn, waits through silence, then
publishes fresh typed messages. It passes only when each message wakes the ended
conversation and is read and ACKed without a typed prompt.

Run on 2026-10-11 with Claude Code 2.1.296: two consecutive wakes after 30 s of
silence each, armed before every message, 5.6 s from publication to the answered
turn, two ACK markers, `listening_verified: true`. Microphone capture and voice
playback through `codescribe_reply` are separate acceptance checks.
