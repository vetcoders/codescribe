# Agent MCP connector

Codescribe's Transcript BUS is normally joined by a local client — Claude Code,
Junie or OMP — through the plugin described in the README. The agent MCP
connector is the equivalent entry point for an agent that does **not** run on
this machine: a hosted assistant, a custom service, anything that can speak MCP
(JSON-RPC 2.0) over stdio behind a transport you control.

It gives that one remote agent its own mailbox on the BUS: read, acknowledge,
send to current participants and, if the owner opts in, requested speech. It is
a participant surface, not an administration surface. No tool can attach or
detach a session, open a shell, read arbitrary files, start a recorder or
reach any mailbox but its own.

Four files make up the connector, all shipped in the managed agent-bridge
payload next to `bus-demux.py`, `bus-mcp.py` and `cs-bus`:

| File                           | Role                                                                                      |
| ------------------------------ | ----------------------------------------------------------------------------------------- |
| `scripts/bus-mcp-agent.py`     | The MCP surface itself. Tools-only; never advertises live Events.                         |
| `scripts/bus-mcp-runtime.py`   | Local bootstrap: scope check, callback preflight, or the pinned Events runtime.           |
| `scripts/bus-mcp-events.py`    | Outbound `codescribe.mailbox.ready` notifications and their metadata.                     |
| `scripts/bus-mcp-key-store.py` | Signing-key backend that talks to a local Codescribe binary over an inherited descriptor. |

## Trust model

**The transport in front of this process must authenticate the connection and
restrict it to the configured principal. Stdio is not an authentication
mechanism.** Anything that can start the process, or reach the socket your
transport exposes, is the agent as far as this surface is concerned. Run it
behind a transport that proves who is calling, and give that transport no
broader reach than the agent needs.

Credentials the transport or its launcher holds in this process's environment
never reach the BUS helpers it starts: `cs-bus` and everything `cs-bus` starts
receive only `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_*`, `TMPDIR`, `TZ`,
`SPEECH_TTS_MODEL_OPENAI`, `LLM_OPENAI_API_KEY`, `CODESCRIBE_ENV_PATH` and the
bus location keys `bus-demux` declares in `BUS_PATH_ENV_KEYS`
(`CODESCRIBE_TRANSCRIPT_BUS_PATH`, `XDG_STATE_HOME`, `CODESCRIBE_DATA_DIR`);
see `CHILD_ENVIRONMENT` in `bus-mcp-agent.py`. The location keys let the speech
capture guard find the same transcript bus the app writes, which it resolves on
its own rather than from the mailbox `--bus`. `LLM_OPENAI_API_KEY` is the one
credential the BUS helper itself reads: OpenAI speech takes it before Keychain,
exactly as for `cs-say`. Bridge home, bus, provider and session travel as
explicit arguments. The native key helper has its own, narrower allowlist. Still
start the process with the smallest environment your transport allows.

Everything the agent is allowed to be — provider, session, name, bus, paths,
capabilities — comes from a scope file that the machine's owner writes and
reviews locally. A remote tool argument never selects identity, a filesystem
path, a signing-key backend, a callback destination or speech permission. The
`principal` field is a label the owner chooses so refusals are attributable; it
is not a credential and does not authenticate anybody by itself.

Read, acknowledge, send, speech and call operations then check ownership
against canonical BUS state; `codescribe_status` reports only this session's own
lease metadata and the participant roster, without message text. The
lease is derived from `provider` plus `session_id`, so a scope with a different
provider simply addresses a different mailbox rather than sharing one. The
lease's recorded name must match the scope's name (compared case-insensitively),
the audience binding used for sending must be owned by this agent, and an
acknowledgment is only accepted for identifiers this connection actually read.
Before this session has a lease, speech under the scope's name is refused while
another session holds a BUS binding for that name. The owner is still
responsible for giving each scope a name that is its own.
Message text arriving from the BUS is untrusted data: receiving it is not
execution, and acknowledging it only records a read.

## Scope file

One JSON object, passed as `--scope /path/to/scope.json`. Unknown keys are
refused; so is a missing required key. Required fields are never defaulted;
optional fields take the defaults shown below.

| Field               | Required                         | Shape                                                  | Meaning                                                                     |
| ------------------- | -------------------------------- | ------------------------------------------------------ | --------------------------------------------------------------------------- |
| `principal`         | yes                              | non-empty string, ≤ 200 chars                          | Label of the authenticated caller the transport admits.                     |
| `provider`          | yes                              | `[a-z0-9][a-z0-9-]{0,63}`                              | Client id this agent attaches as; part of the lease key.                    |
| `session_id`        | yes                              | canonical UUID string                                  | This agent's session; part of the lease key.                                |
| `name`              | yes                              | non-empty string, ≤ 80 chars                           | The agent's BUS name, as other participants address it.                     |
| `bridge_home`       | yes                              | absolute path                                          | Agent-bridge home holding `leases/`, `acknowledgments/` and `runtime/`.     |
| `bus`               | yes                              | absolute path                                          | The exact bus file from this session's attach receipt.                      |
| `state_dir`         | yes — **new in this generation** | absolute path                                          | Per-connection state directory (see below).                                 |
| `call_text`         | no                               | string, 1–200 chars after strip, no control characters | Fixed phrase for `codescribe_call`. Omit it and the tool does not exist.    |
| `speech_enabled`    | no                               | boolean, default `false`                               | Allows `codescribe_say` / `codescribe_call` to actually speak.              |
| `mailbox_enabled`   | no                               | boolean, default `true`                                | When `false`, only status and the fixed call are offered.                   |
| `broadcast_enabled` | no                               | boolean, default `false`                               | Allows sending to channel `0` (all participants).                           |
| `message_targets`   | no                               | `"current_bus"` or a list of ≤ 16 names                | Local recipient policy. The default empty list permits no direct recipient. |

`message_targets` is an owner-side authorization policy, never a protocol enum:
recipients always resolve against the live BUS bindings for slots 1–9 under the
canonical binding lock, so renames and slot moves need no regenerated catalog.

### `state_dir`

`state_dir` is where this connection keeps its own idempotency receipts and
Events metadata. It is required and fails closed — there is no default and no
fallback:

- it must be a direct child of `<bridge_home>/runtime`;
- its basename must match `^[a-z0-9][a-z0-9-]{0,62}-mcp$`;
- it must not be a symlink;
- if it exists it must be a directory owned by the current user and not
  group- or world-writable;
- if it does not exist, a starting connection creates it with mode `0700`
  (validation alone and `--check-scope` never create it).

Per-lease state then lives at `<state_dir>/<lease_id>/`, holding `calls/`,
`send/`, `say/` and `events.sqlite`, so different sessions stay apart. Two
processes must not serve the same lease at once: the Events database lock
refuses the second, and that lease's idempotency receipts belong to whichever
process serves it.

The resolved path of `events.sqlite` is part of the Events signing-key
namespace. A deployment that already has keys keeps them by naming the directory
it already uses — the layout under `state_dir` is unchanged, so the namespace is
unchanged. Confirm that before switching, with `--check-scope`.

## Tools

Request identifiers are 24 lowercase hex characters. Sends are bounded to
64 KiB, speech to 1200 characters. Retrying with the same `request_id` and the
same input returns the first result instead of acting twice; an attempt whose
outcome is unknown is recorded as `uncertain` and never replayed automatically.
Idempotency records store hashes and references, not another copy of the text.

| Tool                | Arguments                                 | Effect                                                                                                    |
| ------------------- | ----------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `codescribe_status` | none                                      | This agent's own status and the current participant roster. Status never proves that anyone is listening. |
| `codescribe_read`   | none                                      | One complete unread message from this agent's mailbox, with provenance. Does not acknowledge.             |
| `codescribe_ack`    | `delivery_ids`                            | Acknowledges identifiers this connection read. ACK means read, not executed.                              |
| `codescribe_send`   | `request_id`, `to`, `text`                | Publishes one message to a current participant by name or slot 1–9, or to `0` when broadcast is enabled.  |
| `codescribe_say`    | `request_id`, `text`, optional `reply_to` | Requested speech. An addressed reply requires that this agent read and acknowledged that delivery.        |
| `codescribe_call`   | `request_id`                              | Prepares the configured `call_text`. Listed only when `call_text` is set.                                 |

Without `speech_enabled`, `codescribe_say` and `codescribe_call` return a
prepared result and attempt nothing. `spoken: true` requires a matching terminal
playback receipt owned by this agent's lease.

Every request must carry `params._meta` with
`io.modelcontextprotocol/protocolVersion: "2026-07-28"` and a bounded
`io.modelcontextprotocol/clientCapabilities` object. The standard optional
`clientInfo` and `logLevel` fields are accepted. Additional bounded metadata is
stripped before business dispatch: it never grants authority, enables speech or
relaxes argument validation. Refusals carry a closed `data.reason` code and
never echo the values that were supplied.

## Events

One event, `codescribe.mailbox.ready`, for this agent's own mailbox, with no
remote filters. It carries only delivery and session identifiers plus the time
of occurrence — never message text. Delivery is a Standard Webhooks POST to one
HTTPS destination, signed with a key the owner supplies at subscription time;
the body and event id are stable across retries and restarts. Three bounded
attempts; `410` and `413` are never retried. A verified challenge precedes
admission, and changing the destination URL requires an explicit unsubscribe.

A 2xx from the callback is receipt of a notification. It is not a read and not
an acknowledgment — those remain `codescribe_read` and `codescribe_ack`.

Signing material never enters the metadata database, which holds only opaque
references. The reference namespace is derived from the principal, the session
and the resolved path of `events.sqlite`.

## Running it

### Check a scope

```sh
python3 scripts/bus-mcp-runtime.py --scope /path/to/scope.json --check-scope
```

Validates the scope and prints one JSON object — schema label, provider, name,
lease id, whether `state_dir` already exists, resolved state path, resolved
`events.sqlite` path and the Events key namespace — then exits. It writes
nothing: it starts no server, creates no directory, reads no mailbox, opens no
key backend and contacts no network. Only a starting connection creates an
absent `state_dir`. The namespace is a hash, not a secret. Use it to compare an
old and a new configuration before switching one over, and to confirm that the
key namespace is unchanged and the existing directory is found. The output
names a real lease and local paths: keep it on the machine, out of repositories
and shared reports.

`--check-scope` is exclusive with the callback modes below and with a native key
binding.

### Tools only

```sh
python3 -E scripts/bus-mcp-agent.py --scope /path/to/scope.json
```

The standalone agent CLI is deliberately tools-only. It never advertises live
Events and never selects credentials by itself; setting capability flags cannot
turn Events on.

### Callback preflight

```sh
python3 scripts/bus-mcp-runtime.py --scope /path/to/scope.json --callback-preflight
```

An explicit local mode for discovering which callback host a hosted client
actually uses. It opens no key backend, metadata database or lock, starts no
watcher and makes no network call. Discovery lists status and the mailbox event;
every otherwise-valid `events/subscribe` is refused with `-32015` and
`data.reason: "host_unpinned"` plus only `data.callbackHost`. Nothing is
subscribed, pinned or delivered — the candidate host is not yet a verified
destination. Invalid URLs, secrets, TTLs or owners are refused without
disclosing a host at all.

### Pinned Events

```sh
python3 scripts/bus-mcp-runtime.py --scope /path/to/scope.json \
  --codescribe /reviewed/path/to/codescribe --native-sha256 REVIEWED_SHA256 \
  --callback-host reviewed.callback.host
```

All arguments are reviewed local metadata, not credentials. The owner supplies
the key-store binding and the callback allowlist; neither can arrive through an
MCP tool argument.

This mode needs a Codescribe binary that provides the `mcp-event-key --fd N`
interface, which the Python driver pins by hash and talks to over an inherited
Unix stream. That binary ships separately from this connector; if your build
does not provide that subcommand, use `--callback-preflight` or the tools-only
CLI instead.

`bus-mcp-runtime.run(scope_path, secret_store=..., callback_hosts=...)` is the
equivalent in-process entry point for embedding.

## Managed updates

The agent-bridge payload carries these four files alongside `bus-mcp.py`,
`bus-demux.py` and `cs-bus`. `make install-bus` stages, verifies and atomically
replaces that code through the existing installer. Lease, cursor, acknowledgment
and Events state live outside the replaceable payload. The regression in
`scripts/tests/test_install_bus_payload.py` stages the payload, reads and
acknowledges through it, restages and reads again; it does not run the Swift
installer or a live connection.

Replacing the payload replaces these files under the stable path below. A
running connection keeps the code it loaded, but its next start uses the new
generation: a scope written before `state_dir` existed then refuses to start.
Move such a connection to a scope with `state_dir` — compare both with
`--check-scope` — or to a copy it owns, before installing a payload that
carries this generation.

Point the connector at the stable managed path,
`~/.codescribe/agent-bridge/runtime/bin/bus-mcp-runtime.py`, and start it with
`python3 -E` so host `PYTHONHOME` and `PYTHONPATH` cannot reach it.

`Mailbox.api_version = 1` is the shared contract between this surface and the
canonical BUS client. Additive receipt fields stay valid; a breaking change bumps
that version and updates its consumers in the same source cut, and an
unsupported version refuses before any BUS operation.

## Tests

```sh
python3 -m unittest scripts/tests/test_bus_mcp_agent.py \
  scripts/tests/test_bus_mcp_runtime.py scripts/tests/test_bus_mcp_key_store.py
```

Synthetic throughout: no network, no real signing key, no sound, no real
Keychain. They cover tool ownership, read/ACK, two agents proving they cannot
reach each other's lease or binding, `state_dir` validation, key references,
rotation, deletion faults and orphans, retry-body stability, destination and
deadline bounds, discovery, lease revocation, watcher re-arm, EOF cleanup and
the key-helper spawn boundary.
