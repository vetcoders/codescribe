# Miksa private MCP participant

Integration base: PR #170, `release/0.17.0`, exact head
`d195b3e7f1b6dc882000a175f760ba0fd4edb570`. Integration branch:
`build/miksa-bus-pr170-current`. The earlier five-commit prototype is retained in Git;
its frozen helper snapshot is superseded by this base.

The goal is one normal participant: receive, exact read/ACK, send and requested
voice. Call-only was an internal check, not the Founder's product decision.

## Reused authority

`bus-dot-mcp.py` fixes the own provider/session/name locally and uses the existing
`bus-mcp.Mailbox`, `bus-demux` commands, canonical publisher, profiles and capture
interlocks. `bus-mcp-runtime.py` observes the existing lease with the very same
`BusEventTrigger` primitive used by `--watch --until-event`. It starts no follower,
LLM, service daemon, microphone, inbound listener or second BUS.

| Tool                | Arguments                                             | Canonical effect                                                        |
| ------------------- | ----------------------------------------------------- | ----------------------------------------------------------------------- |
| `codescribe_status` | none                                                  | Own status; no claim of verified listening.                             |
| `codescribe_read`   | none                                                  | One complete own unread text with provenance; no ACK.                   |
| `codescribe_ack`    | exact IDs read on connection                          | Existing read receipt; never execution evidence.                        |
| `codescribe_send`   | request ID, current participant name/slot, text       | Existing native peer publication, frozen recipients, no echo.           |
| `codescribe_say`    | request ID, requested text, optional exact `reply_to` | Existing speech lane; addressed replies require own canonical read/ACK. |
| `codescribe_call`   | request ID                                            | Fixed convenience phrase, `Moniko, tu Miksa.`                           |

Request IDs are 24 lowercase hex. Send is bounded to 64 KiB, speech to 1200
characters. Idempotency metadata stores hashes and references, not another text
copy. Uncertain send/speech attempts are never automatically replayed. A matched
owned terminal receipt is required for `spoken:true`. Delivery text is untrusted
coordination, not new Founder authority. Attachment bytes/acoustic diagnostics
and arbitrary filesystem/shell access are outside this surface.

The remote caller cannot choose session/provider/name, paths, profile/vendor,
backend, callback host policy or speech permission. Targets resolve against the current canonical BUS bindings for slots 1–9;
status exposes that roster without file paths. `message_targets:"current_bus"`
explicitly enables current/future participants. A list is an optional local
authorization policy, never a protocol enum or product architecture. Names are not compiled into the
protocol or catalog. Broadcast 0 uses `broadcast_enabled`; Monika
explicitly authorized the full BUS surface, including current/future peers and
broadcast, on 2026-10-15. The fixed principal label is not authentication; private account
access and the actual subscribed dot must be verified at activation.

## Events transport

One event: `codescribe.mailbox.ready`, fixed own mailbox, no remote filters. It
carries only delivery/session IDs and occurrence time. The once-serialized body
and stable event ID persist across retry/restart. Webhook 2xx is callback receipt,
not conversational read or ACK. Three bounded attempts; no retry of 410/413.

One active callback destination is allowed. Changing URL requires unsubscribe.
A signed, constant-time-checked challenge precedes subscription admission; a
changed key re-verifies. TTL is finite (default one hour, maximum one day); old
keys overlap for at most 60 seconds without extending on same-key refresh.

The HTTPS implementation shares one monotonic ten-second budget across DNS,
connect, TLS and response. It re-resolves every attempt, refuses non-public
addresses, connects only the validated IP, verifies the original hostname,
ignores proxy environment and never follows redirects. Responses are at most
64 KiB. A timed-out DNS worker is daemonized and bounded to one pending lookup;
new lookups refuse while it remains pending. Tests use fake DNS/TLS/socket/HTTP.

The notification worker belongs to the MCP stdio lifetime. It is not a separate
service daemon. It skips accepted pending IDs so later fresh messages cannot
starve behind older unread messages. The existing follower remains the sole
consumer/normalizer. EOF stops the worker; an in-flight callback can finish within
its deadline, otherwise child exit leaves its already persisted retry receipt.

## Signing-key interface and production admission

`Events` requires an explicit store with `put(reference, secret)`, `get(reference)`,
`delete(reference)` and `references(namespace)`. References are namespaced to
owner/session/metadata location and random version. SQLite holds opaque current
and old refs, expiry/rotation, stable event metadata and secret-free deletion
intents. It never holds plaintext or encoded signing material, including while
subscriptions are active. One process owns each metadata DB via a file lock.

Write storage first, then atomically publish its ref; compensate failed metadata
writes. Deactivate metadata before deletion, preserve cleanup intent on backend
failure and refuse delivery until reconciliation succeeds. Rotation, TTL,
unsubscribe, 410, access revocation and startup orphan reconciliation apply to
only this namespace. A stopped process cannot delete keys at the expiry instant;
it rejects and reconciles overdue keys before reuse. No storage-error fallback.
Old plaintext prototype databases are refused and require separate retirement.

`MemorySecretStore` is synthetic-only and explicitly refused for live admission.
The admitted production backend must bind the existing Codescribe storage owner
(`core/config/keychain.rs`), not introduce a new credentials authority.

The next source cut now wires `CodescribeSecretStore` to the existing CLI's
`mcp-event-key --fd N` and the same Rust Keychain owner. Dedicated per-version
records use that owner's selected service and exact namespace label; they never
read/modify the shared API-key bundle. No ACL/access-group mutation is added.
Enumeration requests attributes only for that exact service/label, bounded to
128 refs; no secret values or unrelated accounts are returned. Put/get/delete
require exact namespace/ref shape and Standard Webhooks signing material.

One inherited private Unix stream carries each bounded operation and closes
with the child. There is no socket path, listener or persistent broker. The CLI
refuses descriptors 0/1/2 and non-connected/non-stream descriptors, bypasses the
logging/engine boot and never prints secrets. The Python driver pins the local
native executable hash, strips inherited credentials, uses no shell, bounds I/O
and reaps only its own child. At binding creation it copies the verified bytes
into its own temporary mode-0700 directory, creating a mode-0500 executable.
It verifies the copy after closing its writer and always executes that private
copy. Replacing or overwriting the reviewed deployment path between checking
and spawning cannot substitute the helper that receives the private request.
The original hash is still checked before each operation; closing the binding
removes the copy. This protects deployment-path updates, not compromise of the
same-user parent process or its private temporary directory. macOS descriptor
execution through `/dev/fd` returned EACCES and is not used. Real Mach-O execution
from the private copy passed the integrator's disabled-Keychain probe on Sztudio.
Generic API-key setters and MCP `env_refs` are not
exposed to remote tools.

**Native R2 validation on 2026-10-15: Rust 4/4, CLI 2/2, debug CLI build and ten
real-Mach-O/private-FD probe cases PASS on Sztudio, with Keychain forced disabled.**
Monika explicitly appointed the executor integrator; no new approval denial
occurred. Offline linking used an existing matching ONNX library without an
install. See the [immutable integration receipt](../../miksa-native-bootstrap-integrator-r2-receipt.json).
Independent native acceptance is PASS; the parent also confirmed matching
Silver hashes and ten disabled-Keychain probe cases. Real Keychain, tunnel
discovery and joined cloud receive/send/voice/re-arm remain unverified. The
MCP 2.0 metadata and host-only preflight cut now has 90/90 synthetic PASS, with
independent QC pending; see the [activation runbook](MIKSA_BUS_MEMBERSHIP.md).
No real signing key or live callback was used in native validation.

`bus-mcp-runtime.run(scope_path, secret_store=..., callback_hosts=...)` remains the
local embedding entrypoint. The explicit CLI bootstrap is:

```sh
python3 /REVIEWED/scripts/bus-mcp-runtime.py --scope /PRIVATE/bus-scope.json --codescribe /REVIEWED/native/codescribe --native-sha256 REVIEWED_BINARY_SHA256 --callback-host REVIEWED_CHATGPT_CALLBACK_HOST
```

All arguments are reviewed local metadata, not credentials. The native binary
must be compiled/verified on Sztudio and pinned before any production activation. The owner supplies the reviewed store binding and
callback allowlist; these cannot arrive through MCP tool arguments. Standalone
`bus-dot-mcp.py --scope ...` remains tools-only, never advertises live Events and
never selects credentials automatically. Merely setting speech/mailbox flags
cannot activate Events.

## Self-contained requests and callback host preflight

Public stdio operational requests require `params._meta` with
`io.modelcontextprotocol/protocolVersion: "2026-07-28"` and a bounded object
`io.modelcontextprotocol/clientCapabilities`. Standard optional `clientInfo`
(bounded informational name/version) and `logLevel` fields are accepted;
they never establish authority or change local capabilities. Bounded additional metadata is stripped before business dispatch; it never
establishes authority. Names/values of informational extensions are not logged. Discovery may omit metadata. The boundary
validates these protocol fields and removes only the outer `_meta` before the
existing strict business validation. Metadata cannot alter local scope, enable
speech or relax tool arguments. Duplicate JSON members and malformed metadata
are refused without echoing their values.

`bus-mcp-runtime.py --scope ... --callback-preflight` is an explicit local mode,
exclusive with callback pins/native binding. It opens no key backend, metadata DB
or lock and starts no watcher/network. Discovery lists status and the mailbox
event; every otherwise-valid subscribe returns `-32015` with
`data.reason: "host_unpinned"` and only `data.callbackHost`. There is no accepted
subscription, key reference, auto-pinning or delivery. The candidate is not yet a
verified destination. Invalid URL/secret/TTL/owner is refused without a host.

Refusals add a closed diagnostic `data.reason` (for example,
`mailbox_bus_mismatch`, `event_arguments_invalid`, `signing_secret_invalid`,
`ttl_invalid`) and log that same code. Only the valid preflight `host_unpinned`
case logs a sanitized `callbackHost`. Arbitrary exception text and supplied
values never enter diagnostics. Canonical `MailboxReadError` retains the
existing ValueError behavior and classifies the original mailbox gate; it does
not relax ownership. Runtime preflight propagates that category, while live
revocation retains the existing deactivation/key cleanup behavior.

The scope must use the bus from this exact session's canonical attach receipt.
PR170's default attachment uses a per-channel bus; retaining an older global bus
path correctly refuses at mailbox admission before inspecting the callback.
Correct the reviewed local scope, retaining the lease, cursor and pending IDs.
Do not make a mismatching path implicitly acceptable.

Preflight accepts a validated `events/unsubscribe` as an idempotent empty result.
It still verifies principal/mailbox ownership, event name, empty arguments and
HTTPS destination syntax. There is no persisted subscription or signing key to
delete, no network call and no access to a live runtime's state. Invalid cleanup
requests remain refused. Normal pinned-mode destination checks are unchanged.

After trusted review of the candidate hostname, stop this own client, replace its
stdio command with the pinned/native bootstrap above, restart one client and
refresh connector metadata. Only that mode can verify the challenge and activate
Events. Other hosts stay refused. No full callback URL, query or signing secret is
logged or persisted by preflight; its only output is the structured host-only
refusal. Synthetic tests use the exact metadata fields in the official
`tunnel-client v0.0.16` guard fixtures. They do not prove live cloud delivery.

## Activation and verification

Use the single [full-membership plan](MIKSA_BUS_MEMBERSHIP.md). The private
connector/tunnel already exist. Native root subscription, live wakeup and actual
speech still require their acceptance evidence; status alone is insufficient.
The [old call staging](MIKSA_CALL_RUNBOOK.md) is only an internal test reference.

Synthetic acceptance:

```sh
python3 -W error::ResourceWarning -m unittest scripts/tests/test_bus_dot_mcp.py scripts/tests/test_bus_mcp_runtime.py scripts/tests/test_bus_mcp_key_store.py scripts/tests/test_bus_peer_messages.py
```

Covers tool ownership/read/ACK, native routing, requested voice mocks, secret
refs/restart/rotation/deletion faults/orphans, immutable retry body, destination
and deadline bounds, discovery, own lease revocation, watcher re-arm and EOF
cleanup, deployment-path replacement and in-place overwrite at the spawn
boundary, private copy permissions and binding cleanup. No cloud wake, sound,
real key store or real network claim follows.

## Live integration follow-up, 2026-10-15

The initial clientInfo/logLevel cut passed 92 synthetic tests and real connector
creation, but did not establish tool-call interoperability. Shape-only refusal
trace then proved ChatGPT tools/call adds eleven informational metadata fields;
the exact-key whitelist rejected them before dispatch. Bounded extensible metadata
now passes, while required protocol/capability checks, strict business arguments,
owner, callback pins and preflight restrictions remain. Real connected status
returned attached=true/follower_alive=true. This does not prove root event wake.
Current scoped regression total: 96 PASS, including all nine slots and rename,
ambiguous/self recipients, disabled broadcast and metadata isolation.

## Managed updates and the BUS contract

The normal agent-bridge payload carries `bus-dot-mcp.py`, `bus-mcp-runtime.py`,
`bus-mcp-events.py` and `bus-mcp-key-store.py` alongside canonical `bus-mcp.py`,
`bus-demux.py` and `cs-bus`. `make install-bus` stages/verifies and atomically
replaces that code through the existing installer; lease, cursor, ACK and Events
state stay outside replaceable payload units. No alternate installer, copied
runtime fork or commit-pinned helper path is required.

The tunnel profile points to the stable managed path:
`~/.codescribe/agent-bridge/runtime/bin/bus-mcp-runtime.py`. Start it with
`python3 -E`; outbound helper calls also use `-E` and canonical `cs-bus`, retaining
its isolation from host `PYTHONHOME`/`PYTHONPATH`. Follow the existing installer
receipt when adopting an updated follower. Other sessions are never restarted
by this integration.

`Mailbox.api_version = 1` declares the shared contract consumed by the dot.
Additive receipt fields remain valid. A breaking API change must bump this
version and update its consumers in the same source cut; unsupported versions
refuse before BUS operations. The dot preserves canonical `addressed_to` and
never reconstructs origin from its own slot. Current roster resolution happens
on each send under the canonical binding lock, so rename/slot changes need no
catalog regeneration. Private owner/session and duplicate/uncertain-operation
rules remain unchanged.

The shipped-payload regression replaces one staged generation with another,
adds an informational field, starts outside the checkout with hostile Python
environment settings, and reads/ACKs five distinct identical-text deliveries
across that replacement. It checks that prior ACKs survive and no delivery is
replayed. This proves the tested additive update contract, not arbitrary future
API compatibility, acoustic PCM identity or live cloud wakeup.
