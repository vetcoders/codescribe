# Native Codescribe extension for Junie

The native extension runs **inside the current Junie CLI conversation**. It uses
Junie's normal extension, skill, hook and MCP mechanisms. No IDE, separate agent
conversation, ACP process or additional executable command is required.

## Install and connect

```sh
make install-bus
```

In ordinary Junie, register and install through its native extension manager:

```text
/extensions marketplace add ~/.codescribe/agent-bridge/runtime/plugins
/extensions install codescribe
/codescribe junie 6
```

Junie owns marketplace identity and caching. No private registry hash is
reimplemented by Codescribe. Installation is hot-loadable and does not require
an app or CLI restart. For an already registered local marketplace,
`cs-bus --install-junie-extension` can enable Codescribe in Junie's user scope;
it preserves unrelated configuration and refuses to invent a marketplace ID.

The MCP launcher resolves the verified helper at `$HOME/.local/bin/cs-bus` and
adds the standard macOS helper/interpreter directories to its own PATH. It also
works when Junie is launched from an app or launchd without a terminal PATH;
HOME, credentials and the user's shell configuration are never changed.

The `SessionStart`/`UserPromptSubmit` hook supplies Junie's **real session_id**.
The skill uses it with `codescribe_connect`; an occupied channel is refused, not
taken over. Existing voice profiles, authentication and permission policy remain
unchanged. Supply a voice only if you want to change it. Hook support is EAP;
without a native hook receipt, obtain `/link` rather than inventing an identity.

## Native reception and the current listening limit

The extension's `codescribe_wait` MCP tool holds one ordinary tool call open
while the mailbox is empty. A fresh message returns one complete canonical
envelope into the same Junie conversation; the agent reads it, explicitly ACKs
its exact ID, acts and replies. The wait does not ACK, launch another Junie,
simulate terminal keys, or rely on background stdout notifications.

**Junie release 26.10.5 imposes a fixed three-minute MCP deadline.** Its server
configuration has no timeout field, and progress notifications do not extend
the outer deadline. This transport therefore proves native read/ACK/voice,
not indefinite, zero-model-polling listening. After a timeout, do not silently
retry a listening loop. Keep the mailbox intact and report that reception needs
to be resumed. Native task exit leaves a collecting follower, not an active agent.

Reception stays within the active task. Cancelling the tool lets a new typed
prompt interrupt it; explicit stop cancels and detaches. This
does **not** inject prompts into an unrelated idle session. Async hook context
is not used as an idle wakeup, and the native Stop hook retry cap is not disabled.

`codescribe_read`/`codescribe_ack` preserve read receipts. The MCP connection
refuses an ACK for an ID it has not completely returned, foreign owners,
truncated/oversized envelopes, parallel waits, and session identity changes.
Distinct IDs remain distinct even when all five messages say “Iwo”. A canceled
wait or a disconnected client leaves the durable mailbox untouched.

For requested voice replies, `codescribe_reply` delegates to the same `cs-say`
lane with the owned, read and acknowledged ID. It retains recording interlocks
and reports `spoken`/`reason`; credentials are never changed to conceal a failed
attempt. An uncertain or already attempted reply is not replayed automatically.
Peer messages remain coordination, not new Founder authorization.

## Verification

```sh
python3 -m unittest discover -s scripts/tests -p test_bus_mcp.py
bash scripts/tests/agent-bridge-payload-test.sh
```

A fresh message after `codescribe_wait` becomes idle, an agent-issued ACK and an
answer without a typed nudge establish native listening. A PID, cached extension
or connect receipt alone remains `attached_unverified`. Microphone reception and
successful speaker playback require separate acceptance evidence.

## Separate headless ACP utility

The previously shipped `cs-junie` is a separate headless ACP runner, **not this
native extension**. It is not needed or started by the native connection. Its
original delivery evidence is archived in `docs/proofs/junie-plugin-20261011.md`.
The instructions below describe that optional runner only.

### ACP runner

`cs-junie` connects Codescribe's owned mailbox to a persistent Junie session
through [ACP](https://agentclientprotocol.com/protocol/v1/initialization).
It does not simulate keystrokes, poll the model, use cron, or depend on a CLI
notifying the agent when a background process writes to stdout.

## Install and start

Requires Python 3.10+ and Junie with `--acp` support (tested against 26.10.5).
Junie's model and authentication stay in Junie's existing configuration.

```sh
make install-bus
cs-junie --name junie --channel 6 --project /path/to/repo
```

Keep this command running in a terminal. It creates a real session with
`session/new`, saves its returned ID, and connects one named Codescribe follower.
Use channel 6 in Codescribe to speak or type to it. No microphone is opened by
the plugin. This is a plugin-managed conversation, not remote control of an
already running Junie TUI or this chat.

An occupied channel is refused, not taken over. Release the previous connection
from its owning conversation before moving its name/channel to this plugin.
Starting a second plugin for the same project/name/channel is also refused.

```sh
cs-junie --name junie --channel 6 --project /path/to/repo --speak
```

`--speak` passes the final answer and the exact delivery ID to `cs-say`. It keeps
the existing voice profile, credentials and recording/playback interlocks.
Missing TTS credentials do not undo the durable text answer; inspect the helper's
`spoken` and `reason` result. No voice vendor or credentials are changed.

## Delivery and permissions

The plugin watches the existing follower, reads one complete canonical envelope
at a time, and sends `session/prompt` to the same Junie session. Additional
messages wait in the durable mailbox while a turn runs. An empty mailbox makes
no model request. Only agent-message chunks from the owned session become answer
text; thought chunks and loaded-session history are not spoken.

Every prompt instructs Junie to read the full envelope and run its exact
`cs-bus --ack` command **before work**. The plugin itself never acknowledges on
stdout, prompt submission or provider acceptance. A completed turn requires a
matching read receipt and ACP `end_turn`; otherwise it stops and preserves the
execution evidence. Five identical utterances with distinct IDs remain five tasks.

Junie retains its normal tool policy. ACP permission requests are shown in the
terminal and can be granted or rejected once. With no interactive terminal they
are cancelled; the plugin does not enable Brave Mode or grant persistent access.
The client advertises neither file nor terminal execution capabilities; those
tools belong to Junie. An agent requiring client-provided tools is not supported.

## Stop and resume

Ctrl-C closes the plugin's ACP process/watch and asks the bus to detach. If
unread deliveries prevent detachment, the helper retains ownership and reports
it. Errors also retain ownership and pending deliveries; they do not silently
clear the mailbox.

Run the same command to load the stored session, lease and cursor. You can also
select an existing **idle** session explicitly:

```sh
cs-junie --name junie --channel 6 --project /path/to/repo \
  --session junie://sessions/SESSION_ID
```

Do not load a session simultaneously owned by another Junie process. If the
agent does not advertise `loadSession`, resume refuses instead of creating a
replacement identity. A plugin slot with a different saved session also refuses.

Connection and execution records are under
`~/.codescribe/agent-bridge/plugins/junie/<slot>/`. The slot hashes the absolute
project path, name and channel. A receipt in `deliveries/<delivery_id>.json`
records submission, completion and answer independently of the bus ACK. An
interrupted or incomplete execution is never automatically submitted again:
inspect its evidence and the original Junie conversation before deciding how to
continue. This avoids repeating a command whose outcome is uncertain.

`--bridge-home` and `--bus` overrides are passed to every helper operation,
including ACK instructions and speech. They support isolated integration proofs
without touching active user channels. Restarting does not promise speech replay
or recovery of an interrupted task.

## Verification

```sh
python3 -m unittest discover -s scripts/tests -p test_cs_junie.py
bash scripts/tests/agent-bridge-payload-test.sh
```

The deterministic ACP peer proves transport boundaries, not live agent delivery.
A live acceptance check uses a fresh message after the plugin has become idle:
canonical mailbox → same Junie session → explicit ACK → answer, without a typed
nudge to Junie. Voice acceptance additionally requires successful `cs-say`
playback. An attach receipt alone remains `attached_unverified`.
