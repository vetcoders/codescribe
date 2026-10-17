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

Other providers need an output-notifying monitor over
`cs-bus --remote HOST --watch --bell --provider PROVIDER --session SESSION`.
Reuse one monitor for that connection. Do not infer remote wakeup from a live
follower or an attach receipt; only a fresh delivery and reply prove it.

Transport tests and a successful SSH command are not proof of native delivery
to an agent on another host. Report precisely which path was exercised.
