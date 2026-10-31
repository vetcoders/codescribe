# Codescribe for local Codex

This is the manually installed local desktop variant. It uses a stdio MCP server
for connect/read/ACK/status/reply/detach, a reviewed Codex lifecycle hook for
session identity, and the existing follower's `codex queue` for idle wake.
The application, mailbox, publisher, queue receipts and playback remain the
existing owners. There is no new agent runner or microphone.

## Install and upgrade

Use the matching Codescribe BUS package. For a source build, `make install-bus`
stages and installs the complete managed payload without replacing or restarting
Codescribe.app or existing followers. It also materializes the portable plugin
with its Python server dependencies and local marketplace. Python 3.9+ and the
Codex CLI are prerequisites; the app's canonical Rust publisher must be installed.

Register/install through Codex's normal plugin commands:

```sh
codex plugin marketplace add ~/.codescribe/agent-bridge/runtime/plugins
codex plugin add codescribe@codescribe-local --json
```

The root manifest is `codescribe/plugin.json`, the MCP declaration is `mcp.json`,
and the marketplace is `.agents/plugins/marketplace.json` relative to that
marketplace root. The root OpenAI extension explicitly selects `hooks/codex.json`;
Claude/Junie default hooks and Claude's Stop/asyncRewake are never auto-selected.
The existing `.claude-plugin` and `.mcp.json` integrations keep their own client
configuration.

Open the installed plugin's setup skill in the client and review/trust its exact
hook definition. Installing does not trust hooks. Load the plugin in a fresh
conversation, or use the client's supported reload at an agreed idle moment.
Do not restart a shared daemon or an active conversation to force activation.
Setup asks for a name and free channel, then calls the MCP connect tool. There is
no shell attach to copy, and no change to global command permissions.

On upgrade, repeat the matching BUS install and plugin install/refresh. The
installed cache can differ from source. The entrypoint hashes both its bundled
Python dependencies and managed counterparts before loading the managed owner;
a mismatch refuses startup with an actionable error. Updating files does not
prove that an already loaded process adopted them. Check cache digests, process
path/start identity, the connected lease and a new read/ACK/reply round.

For rollback retain the **complete** previous managed payload and plugin package,
including manifests and dependencies, and use the canonical installer's rollback
receipt. A config backup alone is insufficient. Do not overwrite lease, cursor,
ACK, pending, voice profiles, Events keys/subscriptions or protected global hooks.

## Identity and access

The trusted host hook receives Codex's real `session_id` and issues a random
256-bit local bearer capability in a private record. Every MCP tool requires
that capability; model-supplied provider/session/path fields are rejected. The
server resolves each request independently, with a separate native Mailbox per
capability, so a shared MCP process has no single mutable conversation owner.
This prevents accidental cross-conversation read/ACK/reply. It is not protection
against other programs running as the same macOS user or intentional disclosure
of the bearer capability. Do not put capabilities in reports or user replies.

Compact and prompt hooks retain the capability. SessionEnd revokes it while
retaining mailbox state; a resumed conversation receives a new capability for
the same provider/session lease. A new session/fork gets a different scope.
Codex subagent hooks carry the parent's ID: subagents are not separate supported
mailbox principals and must not receive this capability.

`codescribe_read` returns one full owned delivery, including attachment pointers,
or explicitly refuses a delivery beyond the canonical read budget. It never
ACKs. ACK requires exact IDs completely returned through that MCP instance;
restart requires re-reading before a new ACK. A durable previous ACK can authorize
a reply retry after reconnect. Replies reuse the constrained connector's durable
request store: retry with the same request ID and content. `uncertain` is retained
and never blindly replayed. Text-only is the default; explicit speech has its own
terminal playback receipt and still requires a human listening check.

Detach explicitly when ending bus tenure. Disable/uninstall removes tool access;
it does not silently discard pending or terminate a follower. SessionEnd revokes
tool capability without removing history. A failed wake leaves pending readable.

## Status and evidence

CLI and MCP share the same status owner. `lease_recorded` means history exists;
`channel_bound` means this exact provider/session/derived lease/bus owns a slot;
`heartbeat_fresh` is a timestamp observation; `follower_verified` checks the
recorded process incarnation, full command, helper path and owner. `attached`
requires both a binding and verified follower. Process probe permission denial,
timeout, nonzero exit, malformed output and stopped/zombie state remain distinct.
No failed observation relaxes the identity test or authorizes signaling a foreign
process.

Attach refusals name their stage: binding lock, takeover, existing follower
verification, follower launch/readiness, voice profile, previous mailbox drain
or binding commit. Readiness timeouts distinguish the first missing lease field,
missing event file and unverified follower identity. Permission refusals reported
by `ps` and undecodable process output remain explicit observation failures;
raw process output is never copied into these diagnostics. A monitor PID without
its recorded process incarnation cannot certify a live monitor.

`wakeup` is configured intent. `pending_wakeups[].disposition` is the canonical
queue attempt. Accepted is not a new turn, and ACK settlement is not wake success.
Local idle acceptance requires a fresh recognizable message after final, the
actual new turn in the intended conversation, full read, exact ACK and reply.
Record client/version/profile, source SHA, plugin cache digest, loaded MCP PID
and path/start, owner/lease/channel, message/delivery/queue IDs, turn, ACK, reply
and separate playback ticket. Omit tokens and private transcript bodies.

Work/Cloud with a local executor is a separate transport/wake variant. This
package neither enables MCP Events nor claims that a local queue can load cloud
threads. It never edits Codex's private databases or creates a replacement agent
runner. Silver-host sandbox, GUI/SSH Keychain, audio and installed UI visibility
need their own receipts. Hermetic tests do not certify those surfaces.

Report delivery stages independently: implemented, tested, installed, live
accepted, publicly published. The source test suite is
`python3 -m unittest scripts.tests.test_bus_mcp_sessions scripts.tests.test_install_bus_payload` plus the existing native MCP, constrained
connector, handover and listener-recovery suites. Unrelated baseline gate failures
remain separately reported.

## Official platform contract

- [Package your plugin](https://developers.openai.com/plugins/build/plugins):
  portable root manifest/MCP, local marketplace, installed cache and hook selection.
- [Hooks](https://learn.chatgpt.com/docs/hooks): real host session input, review and
  trust, parent IDs for subagents, informational background output without idle wake.
- [CLI reference](https://learn.chatgpt.com/docs/cli/reference): supported plugin
  install and `queue --thread` interfaces; actual backend routing still needs a test.
- [MCP security boundaries](https://learn.chatgpt.com/docs/enterprise/security-cookbook):
  local stdio execution boundary; constrain command, arguments, cwd and environment.
- [Desktop-only plugins](https://developers.openai.com/plugins/deploy/desktop-only):
  bundled local server/dependencies; public local-MCP submission requires allowlisting.
  Hook-containing packages are for manual desktop distribution, not public directory
  submission. Publishing remains a separate Founder action.
