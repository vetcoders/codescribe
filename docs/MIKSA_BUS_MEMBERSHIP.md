# Miksa: direct BUS activation and acceptance

Current integration base: PR170 `d195b3e7f1b6dc882000a175f760ba0fd4edb570`,
branch `build/miksa-bus-pr170-current`. Xcode27.0 is already installed on Silver.

## Existing resources and authorization

Use the existing private **Miksa Codescribe BUS** connector and
**miksa-codescribe-bus-silver** tunnel in **Libraxis AI**. No second connector or
tunnel is required. Runtime key has only Tunnels Read + Use; never read its value
into tools/model arguments or logs. The approved local file reference is already
configured. No model/audio API scopes, purchases or autostart were added.

Own identity: codex / Miksa / `01a110a4-af1a-74de-8b54-f132e1e7303d`, channel2,
lease `f35f49aaa871a65369bed71a0775890c`. PR170's default attachment uses
`/Users/tester/.codescribe/agent-bridge/buses/channel-2.jsonl`. Set the local scope's
`bus` to the verified attach receipt's exact bus; the old global
`transcript-events.jsonl` scope fails ownership admission. Do not change the
lease, cursor or ownership guard to accommodate an outdated scope. Scope and Events metadata live
under this lease's `agent-bridge/runtime/miksa-mcp` directory, outside payload.

Monika's explicit latest instruction: **Miksa ma móc robic wszystko to co oferuje
bus**. Scope uses `message_targets:"current_bus"`, `broadcast_enabled:true`,
mailbox/speech enabled. The current roster comes from canonical slots1–9; names
and future occupants need no config change. Broadcast0 excludes the sender.
Ownership, exact read/ACK, idempotency and capture/playback interlocks still apply.

The private tunnel/account boundary provides access control. A fixed principal
label does not authenticate or isolate one chat. Another authorized chat using
this connector could access this same fixed mailbox. Keep the plugin private.

## Deployment and updates

Use the existing managed installer (`make install-bus`) to ship the shared BUS
helpers and four MCP files together. It retains mailbox state and foreign living
followers. The tunnel profile uses the stable managed path
`~/.codescribe/agent-bridge/runtime/bin/bus-mcp-runtime.py`, launched with Python
`-E`. Source stays in one repository; no old helper snapshot is deployed beside it.

For ordinary updates, install the canonical payload and adopt that generation
for the own follower/client as needed; retain this scope, lease, cursor, signing
key references and request receipts. A breaking `Mailbox.api_version` change
requires coordinated client changes and contract tests. Additive receipt fields
and roster/rename changes must keep the upgrade regression green.

The integration must be admitted into the shared release sources before another
release's installer can preserve these MCP payload files. An older app/helper
payload can replace `runtime/bin` and remove files absent from its own manifest.
Check the installed manifest and files before restarting the tunnel client;
do not alternate competing installers or deploy a second helper snapshot.

Native event-key binding remains the independently accepted R2 executable with
an explicit hash. Keep it pinned; a changed executable requires normal artifact
verification, never automatic secret delivery to newly replaced code. Its Rust
source bytes are unchanged by this Python deployment cut. No Rust build was
performed on Silver for this cut.

## Status and first callback host

Real connected `codescribe_status({})` passed after metadata fixes. Discovery
exports MCP2026-07-28, `tools:{}` and `events:{}`. In `--callback-preflight` mode
only status is a tool; the one event is `codescribe.mailbox.ready` with empty
arguments. No Keychain/metadata store/watcher/subscription is activated here.

ChatGPT operational calls include informational metadata beyond protocolVersion,
clientCapabilities and clientInfo. The protocol envelope is bounded and stripped
before strict business validation; it cannot change owner, speech or callback
policy. Refusal traces contain shapes, closed reason codes and, only for a fully valid
preflight subscribe, the sanitized candidate `callbackHost`. No payload values,
unknown key names, callback URL/query or signing secret are logged.

`mailbox_bus_mismatch` means the reviewed scope and canonical lease disagree.
Inspect the same provider/session/name/channel owner, then correct only the local
scope from its attach receipt; never suppress this guard. Other admission and
event validation failures report their own fixed categories. These codes grant
no authority and do not expose a callback host on invalid requests.

A failed preflight may be followed by `events/unsubscribe`. It validates the same
principal, owned mailbox, event, empty arguments and HTTPS destination syntax,
then returns `{}` idempotently: preflight has no DB, keys or network state to
remove. It cannot delete a live subscription from another runtime.

The target root must use its native Events subscription mechanism. Events are
protocol methods, not another codescribe tool. ChatGPT supplies delivery URL and
signing secret automatically. Never ask the human/model to type either. Official
flow: https://developers.openai.com/plugins/build/mcp-events .

The first valid subscribe returns `-32015`, `reason:host_unpinned` and only
`callbackHost`. Verify that host's origin from the real approved root request.
Stop the own foreground client/stdio, keep the same profile, and replace
`--callback-preflight` with reviewed `--codescribe`, `--native-sha256` and exact
`--callback-host`. Do not auto-pin/wildcard. Start one client and Refresh tools
on the same connector. Normal mode exposes six tools and the event. Retry the
native subscribe and require signed challenge plus opaque native Keychain refs.

## Acceptance

End the target root turn. A fresh named utterance must wake that same root
without a typed nudge or agent relay. Read full provenance and exact IDs; ACK
only complete owned reads immediately, drain and re-read. Send a substantive
message to a currently resolved participant; require its canonical receipt and
peer response. Voice uses the owned read/ACK ID, the existing profile and quiet
capture lane; require matching terminal `spoken:true` and Monika hearing it.
Repeat after another ended root turn to prove re-arm. `attached_unverified`
remains the state until this live evidence exists; callback2xx alone is not it.

Uncertain publication/playback is never repeated with a new request ID. No GUI,
app restart, capture override, credential/vendor change or new bridge is part
of that test. Review normal plugin permissions when a real action requests it.

## Verification

Run the scoped Python suites, canonical read/ACK/addressing tests and
`scripts/tests/agent-bridge-payload-test.sh`. The latter compiles the existing
Swift6 installer and installs only into a temporary test home. No microphone,
live callback, user mailbox, real speech or Rust compilation is part of it.
