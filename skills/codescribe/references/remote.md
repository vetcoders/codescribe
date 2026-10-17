# A mailbox on another Mac

`cs-bus --remote HOST` runs the selected operation in HOST's managed helper,
under this conversation's real provider and session. The Founder chooses the
host and channel; a local attachment does not authorize another attachment.
Use an SSH host alias with key access (`BatchMode=yes`). The managed helper
must already exist at `~/.local/bin/cs-bus` on HOST.

```bash
cs-bus --remote HOST --attach --channel CHANNEL --name NAME --provider PROVIDER --session SESSION
cs-bus --remote HOST --read-pending --read-limit 2 --provider PROVIDER --session SESSION
cs-bus --remote HOST --provider PROVIDER --session SESSION --ack DELIVERY_ID
```

Keep `--remote HOST` on read, ACK, status, send and detach. Read complete
envelopes and ACK exact fresh IDs immediately, then drain to zero and read
once more. The local `codescribe_*` MCP tools still address this Mac only.
Local file and bus-path overrides are refused remotely; do not copy local
paths into a command intended for another Mac. Same-session identity rules,
occupied-channel ownership and history recovery still apply.

For Claude Code, a successful remote attach records HOST for the local Stop
hook. It waits on local and remote mailboxes, and a remote notice names its
host and tells the agent to read and ACK there. The in-turn local probe does
not poll remote hosts. A successful remote detach removes that host from the
local hook's list.

For Codex, run the remote commands **on the Mac that owns the Codex thread**.
Remote attach keeps the mailbox follower's native wakeup off. The single local
`--remote HOST --watch --bell --provider codex --session SESSION` streams that
mailbox and submits `codex queue` on the client Mac. It needs only the existing
client-to-mailbox SSH connection; reverse SSH and new network listeners are not
required. Both installed helpers must support this protocol. Reattach the same
lease after updating an older helper, then replace its existing monitor once.

The watch replays unread history on startup, preserving delivery IDs. Its
transport receipts are local and separated by mailbox host and lease; it does
not copy the mailbox or acknowledge words. The same stream carries confirmed
remote ACKs, so local queue withdrawals recover after reconnect and also work
when an ACK was issued directly on the mailbox Mac. `--remote HOST --ack` reports
settlement of both sides. `--remote HOST --status` includes `local_native_queue`
with monitor liveness and unsettled receipts; `--retry-wakeup ID` retries on the
client Mac. Failed or ambiguous submissions are never automatically replayed.
Use `--wakeup off` on the **watch** for deliberate monitor-only operation.
An explicit `--wakeup codex-queue` on remote attach is refused because it would
target the wrong provider store. Keep this watch alive after ending a reply:
the local process is the later-turn wakeup transport, not the output poll window.

Other providers need an output-notifying monitor over
`cs-bus --remote HOST --watch --bell --provider PROVIDER --session SESSION`.
Reuse one monitor for that connection. Do not infer remote wakeup from a live
follower or an attach receipt; only a fresh delivery and reply prove it.

Transport tests and a successful SSH command are not proof of native delivery
to an agent on another host. Report precisely which path was exercised.
