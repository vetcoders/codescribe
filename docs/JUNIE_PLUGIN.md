# Junie plugin for Codescribe

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
