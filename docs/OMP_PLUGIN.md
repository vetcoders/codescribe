# Codescribe plugin for OMP (oh-my-pi)

The same Codescribe plugin that serves Claude Code ([Claude Code](CLAUDE_CODE_PLUGIN.md))
and Junie ([Junie](JUNIE_PLUGIN.md)) connects **the OMP conversation you are in**
to a named Codescribe channel. Messages arrive in that conversation without a
typed nudge, and silence costs no model turn. OMP (`omp`, the oh-my-pi coding
agent) reads Claude Code marketplaces natively; the only OMP-specific piece is one
extension module that replaces the Claude Code `asyncRewake` hook, which OMP does
not execute.

## Install and connect

```sh
make install-bus
```

The installer ships the plugin with the helpers into
`~/.codescribe/agent-bridge/runtime/plugins`. In a shell:

```sh
omp plugin marketplace add ~/.codescribe/agent-bridge/runtime/plugins
omp plugin install codescribe@codescribe
```

Then inside `omp`:

```text
/codescribe leon 4
```

`/marketplace add` and `/marketplace install` inside `omp` are the interactive
equivalents; `/reload-plugins` refreshes a running session after a later
`make install-bus`. The plugin installs into OMP's own registry
(`~/.omp/plugins/installed_plugins.json`), so no `enabledProviders` opt-in for
foreign Claude user sources is needed.

Requirements are those of the helpers: Python 3.10+ reachable from
`~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin` or `PATH`. The MCP launcher
and the wake script both put those directories first and run the verified helper
at `$HOME/.local/bin/cs-bus`.

## What OMP loads from the plugin

| Surface                            | File                          | Loaded by OMP as                                |
| ---------------------------------- | ----------------------------- | ----------------------------------------------- |
| MCP tools `codescribe_*`           | `.mcp.json`                   | marketplace MCP server (`cs-bus --mcp`)         |
| Skill                              | `skills/codescribe/SKILL.md`  | marketplace skill, also `/skill:codescribe`     |
| Identity, `/codescribe`, idle wake | `hooks/pre/codescribe-omp.ts` | extension module from the plugin's `hooks/pre/` |
| Claude Code wake                   | `hooks/claude-code.json`      | not read by OMP                                 |
| Session identity hook              | `hooks/hooks.json`            | not read by OMP                                 |

## Identity

OMP exports no session id to child processes and runs no shell hooks. The
extension therefore reads the id from its own session manager on
`session_start`, `session_switch` and every `agent_end`, and when it changes
appends one hidden context message:

```text
Codescribe identity for THIS OMP conversation: provider=omp session_id=<id>.
```

`codescribe_connect` takes `provider=omp` and that id explicitly. The canonical
provider token is `omp`; `cs-say` and `cs-bus` need `--provider omp --session <id>`
on the command line because there is no environment variable to default from.
`/codescribe NAME CHANNEL [VOICE]` validates the single-digit channel and sends
the model an agent-attributed request to call `codescribe_connect` with exactly
this identity.

## How listening works

After `codescribe_connect` the agent **ends its turn**. On every `agent_end` the
extension runs, in the background:

```sh
cs-bus --omp-wake --provider omp --session <id> --host-pid <omp pid>
```

- The waiter holds the lease's wake lock and waits on the lease file's OS event.
  No model request is made while the mailbox is silent. One waiter per lease; a
  second arm while one is held returns at once.
- For a delivery not yet announced it exits 75 with one English sentence on
  stderr. The extension injects that sentence as a `codescribe_mailbox_ready`
  message with `triggerTurn`, so the conversation starts a turn. The agent reads
  with `codescribe_read`, ACKs the exact `read_delivery_ids`, works and replies,
  then ends the turn; the next `agent_end` re-arms.
- Announced IDs stay recorded until acknowledged, so a message left unread does
  not wake every turn. A lost follower or refused mailbox is announced once as
  `codescribe_listener_problem`. After six silent hours the helper returns a
  `codescribe_wait_idle` receipt; the extension re-arms without a model turn.
- Exit 0 (this session owns no channel, the host pid is gone, or another waiter
  holds the lock) stays silent until the next turn end. A refusal (exit 3) is
  logged through OMP's logger.
- `session_shutdown` aborts the waiter; a new session id drops the old one.
- The waiter never reads, ACKs or replies. `codescribe_status` reports
  `native_wake_armed` for `provider=omp` as it does for Claude Code.

`codescribe_wait` remains available for an explicitly requested listening turn.
OMP applies no client-side MCP deadline unless `timeout` or `OMP_MCP_TIMEOUT_MS`
is set; the wait window is 900 s, the same as Claude Code, and ends with an
explicit `wait_idle` receipt.

## Verification

```sh
python3 -m unittest scripts/tests/test_bus_mcp.py
```

`OmpWakeTests` run the waiter against real lease files (existing unread, arrival
during silence, bound-session-only, host exit, CLI exit code and refusals) and
`OmpProtocolTests` run the whole MCP contract with `provider=omp`. The hermetic
suite is part of `make verify`.

```sh
omp plugin disable codescribe@codescribe   # if the shipped plugin is installed
python3 scripts/tests/omp-plugin-live.py --keep /tmp/omp-plugin-live
```

The live script is an operator-class acceptance: it stages this checkout's
plugin under a temporary marketplace and package name, runs `omp --mode rpc --no-ui --auto-approve` against an isolated bridge home and bus, connects through
the extension's identity, ends the turn, waits through silence, then publishes
fresh typed messages with `cs-bus --send-text`. It passes only when each message
wakes the ended conversation and is read and ACKed without a typed prompt. It
uninstalls its temporary plugin and marketplace afterwards. OMP invokes MCP tools
through its `xd://` file surface (`write` to `xd://mcp__<plugin>_<server>_<tool>`),
which the script maps back to the Codescribe tool names.

Run on 2026-10-13 with OMP 18.10.3 and `kimi-k3:cloud`: two consecutive wakes
after 20 s of silence each, the waiter armed before every message, 16.2 s and
9.2 s from publication to the answered turn, two ACK markers, the message text
answered verbatim, `listening_verified: true`. Voice playback through
`codescribe_reply` is a separate acceptance check.
