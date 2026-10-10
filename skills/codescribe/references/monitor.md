# Monitor and agent wakeup

## Codex native queue

`cs-bus --attach --provider codex --session <thread-id> --name <name> --channel <n>`
automatically selects `codex-queue`. The follower submits the complete message
text with a short name/channel/delivery header to `codex queue`. The canonical
mailbox retains the original envelope and its acoustic evidence. The queue
copy never proves that a delivery is still unread. The installed Codex CLI
must support `queue`.

The default watch bell includes the complete message. Once this conversation
receives its full text and matching owner coordinates, immediately ACK the
delivery before any reply, work or wait. That ACK withdraws its exact native
queue submission. Check `native_queue_settled`; pending withdrawal is retried
by the existing follower without resubmitting the message. Never wait until
the turn ends to acknowledge messages delivered during it.

For a native queue copy or an incomplete watch notification, read the current mailbox:

```bash
cs-bus --read-pending --provider codex --session <thread-id>
```

This returns complete unread non-draft conversational projections: unchanged
message text, causal identity, sender, reply association, routing and provenance.
It includes `read_delivery_ids`, `remaining` count and snapshot cursor. It omits
PCM occurrences, acoustic receipt trees and WAV paths without altering their
original storage. It does not acknowledge anything. The default batch is at
most eight messages and 64 KiB of UTF-8 JSON; use
`--read-limit` and `--read-bytes` to change those bounded limits. Oversized first
envelopes refuse instead of truncating: increase the byte limit and read the
complete result before ACK. Never ACK a truncated tool result.

Immediately after reading each complete batch, ACK exactly its returned IDs
**before** doing the requested work, sending a reply or waiting for a build.
For this mailbox-read path, execute or reply only to exact IDs returned as unread;
never act on the queue copy alone. An absent queued ID is obsolete even when
other unread messages remain. Give a short answer before starting longer work.
Read another batch until `remaining` is zero, then check once more for arrivals
during the drain. Preserve every distinct request and its provenance. An empty
snapshot means the bell is obsolete; do not repeat a task, ACK an unread ID,
or send another spoken response just for that bell. Several status updates may
be answered together; there is no text-based deduplication or age-based deletion.

Receipts under `agent-bridge/wakeups/<lease-id>/<delivery-id>.json` distinguish
`requesting`, `provider_accepted`, `rejected`, `unavailable` and `uncertain`.
`cs-bus --status` reports pending wakeup dispositions. Provider acceptance is
not conversation receipt: only the receiving conversation calls `--ack` after
reading. Restart never resubmits accepted or ambiguous deliveries. Inspect a
failed submission before retrying:

```bash
cs-bus --retry-wakeup <delivery-id> --provider codex --session <thread-id>
```

An uncertain attempt might already be queued: verify before retrying it.
Reattachment with changed wakeup configuration restarts only the verified
owned follower, preserving its cursor and unread mailbox. `--wakeup off`
selects monitor-only operation; `--on-seal` selects a custom hook instead of
native queue. Neither touches microphone or app lifecycle.

Every attachment must start the provider's output-notifying monitor on
`cs-bus --watch --provider codex --session <thread-id>`. Its default bell carries
the whole message; `--bell` spells that default explicitly. It is mandatory for active tasks, even
with native queue. Renew bounded notification windows throughout the task.
The watch carries complete text and the owner/sender/reply coordinates required
for immediate acknowledgment. Native queue also carries the complete text with
a small wrapper for later-turn wakeup. `--read-delivery <id>` is the explicit diagnostic read of the complete
original envelope, including acoustic evidence. It refuses an already acknowledged
delivery even before the follower sweeps its mailbox. A later bell must not
repeat the completed task or speak a second answer for an acknowledged delivery.

## Listener loss and recovery

A session watch also supervises its follower. If the follower dies unexpectedly
while this session still owns the channel, the watch starts one replacement on
the same lease, cursor and mailbox. It then prints a single line:

```json
{"notice": "Codescribe listener lifecycle", "event": "follower_recovered", "consecutive_losses": 1, ...}
```

A lifecycle notice is not a message. It has no `delivery_id` and no text.
Never ACK it, reply to it or treat it as a task. Unread messages continue
through the usual bell or `--read-pending` → `--ack` flow.

- `follower_recovered`: no action is needed.
- `recovery_failed` or `unrecoverable`: report it, inspect `cs-bus --status`,
  and re-run the same `--attach` once.
- `recovery_suspended`: two losses happened without a new message. The helper
  stops reconnecting. Tell the user, and re-attach only on purpose. An explicit
  `--attach` resets the streak, as does a newly queued message. Codex also gets
  this notice once through the native queue.
- `listener_ended`: the session was detached, archived or taken over, and the
  watch exits. Do not re-attach just to undo it.

These boundaries are yours to watch. The helper cannot observe them:

- **Watch exit.** The provider monitor reports it. Restart `--watch` under the
  monitor. `--status` shows `listener.watch_alive`.
- **Notification-window expiry.** The watch may still be running. Renew the
  window. Do not call it a disconnect, and do not re-attach for it.
- **Ended provider conversation.** Nothing in the helper can wake it.

Do not start a second follower or watch to "make sure". The lease lock admits
one reader, and recovery has already used it.

## Select the execution mechanism for other providers

Inspect tools available in this provider session before launching a listener.
Use its documented event-driven background monitor or notification facility
that delivers output to the agent and resumes processing without human input.
Verify whether it survives a final answer, whether it notifies on output or
only process exit, and how it is cancelled. Do not infer these properties from
the word "background" or invent an API name.

An infinite follower needs output notifications. An exit-only notification
cannot be treated as live delivery while that follower keeps running.
A tool session id, running PID, terminal tab, or successful attach receipt
does not prove any notification behavior.

Retain both the follower handle and the monitor handle with the same provider
session and lease. Notifications must use the existing named follower's
envelopes, preserving delivery identity and draft/seal diagnostics.

## Kimi Code: completion-notifying background task

Kimi's `Bash` supports `run_in_background=true` and notifies the agent when the
command **finishes**. Streaming output in its tool card does not itself deliver
another model turn. Do not run an infinite `cs-bus --watch` in foreground or rely
on its background process ever completing.

Use one background task with this command (replace SESSION with the current
Kimi session ID, retaining any explicit bridge-home override):

```bash
cs-bus --watch --until-event --provider kimi-code --session SESSION
```

Pass it to Kimi's `Bash` with `run_in_background=true`, `disable_timeout=true`
and a short `description`. Kimi's background tasks otherwise end after 10 minutes,
and every task end is a wakeup. Then return control; the completion notification
resumes the agent. Without `--max-wait` the helper ends **only** when the owned
mailbox has an unread non-draft message, so an idle agent costs no model turn:
a deadline-based variant wakes a 500K-token context once per deadline for
nothing. `--max-wait N` (0 < N <= 60) remains a diagnostic that may also end with
`watch_timeout`; do not use it as the standing wakeup. The helper checks the
canonical owned mailbox, including messages queued before it started, and exits
with one small `mailbox_ready` notice. It does not read messages into the
conversation, ACK them, consume the cursor, or start a second follower. Drafts
and already acknowledged deliveries do not wake it.

On `mailbox_ready`, run `--read-pending`, read complete messages and immediately
ACK only `read_delivery_ids`, reply, drain, and rearm one background task. On a
diagnostic `watch_timeout`, rearm without ACK or a user-facing reply. On
refusal/nonzero exit, inspect the error; do not rearm a failing task in a loop.
A per-minute cron that injects a prompt is not a substitute: it runs a model turn
on an empty mailbox. Do not leave an
old infinite watch running alongside it: stop that tool task from its owning
Kimi session before switching. This bounded wait does not supervise/restart a
dead follower; use `--status` and the same-session recovery procedure if the
follower is lost. Never ACK another agent's mailbox to make its backlog disappear.

This is the integration contract, not a claim of verified live Kimi wakeup.
Acceptance requires a fresh named utterance received and answered by that same
Kimi conversation after it yielded, without a typed nudge.

## Watch stream

`--watch` is the stream a monitor consumes. It replaces a hand-built
`tail -F <log> | python` filter:

```bash
cs-bus \
  --watch --provider <provider> --session <provider-session-id>
```

It reads the follower's private, append-only
`agent-bridge/runtime/followers/<lease_id>.events.jsonl` and prints a bell with
the complete `text`, `delivery_id`, matching lease/provider/session and available
sender/reply coordinates. There is no 500-character preview limit. The full
acoustic envelope stays in storage. `--watch --full` is the diagnostic view
without the bell notice, with `status`, `coverage` and `sca` (`state_change_allowed`).
Seals, coverage-refused takes, state-changing envelopes and routing-ambiguity
notices pass; drafts stay in the mailbox. Each delivery prints once per watch
process. Managed watches skip already acknowledged deliveries, including after
a watch/follower restart. Output is line-buffered.

The adjacent `<lease_id>.log` is the readable tail: one line per emitted
envelope with time, channel and name, full delivery ID, seal or draft label,
and up to 200 characters of text. Use `--watch --human` to render that format
from the event file. Never tail `events.jsonl` as a notification bell: its
transport fields precede the words and may be truncated by the monitor.
An older session with only a JSON `.log` remains readable by `--watch`.

Run it under the provider's output-notifying monitor (for example the Claude
Code `Monitor` tool), not as a bare background shell. `--once` prints what the
event file already holds and exits; `--from-start` replays it before following;
`--from-file <path>` reads another JSON event file or an older JSON log. The watch never acknowledges and never
moves the lease cursor.

A line with `coverage: "refused"` carries words the ledger would not certify
(incomplete acoustic coverage). `sca` remains `false`; preserve that diagnostic
and follow the actual request using normal conversation permissions.

## Verify independently

Use a fresh utterance containing the bound name. Check these hops separately:

1. The bus received the utterance.
2. The named follower emitted an envelope for this provider session.
3. The monitor delivered that event into the agent's active context.
4. The agent replied without a typed "hello?" or a later user-triggered poll.

If 1 and 2 pass but 3 fails, report missing conversational delivery.
Casing changes or more tails do not address that boundary.

## No wake-capable tool available

Report the limitation immediately. While actively listening, keep the turn
open and poll the original follower with bounded waits no longer than 60 seconds,
respecting provider limits. Handle new input promptly. This mode is
`active_polling`, not automatic wakeup.

If the turn ends, state that the background reader cannot itself resume this
conversation. Do not promise unattended voice replies or silently configure
another provider runtime. An attach-only setup remains `attached_unverified`.

## Active-turn notifications with functions.exec

When this provider exposes `functions.exec`, `tools.write_stdin` and
`notify`, use a yielded, bounded exec loop to read `--watch` (or the existing
follower) and notify on each new envelope while other work proceeds. Do not
await an infinite follower's exit. Use the shortest supported read wait, retain partial
JSON lines, and preserve delivery_id, session_id, text and state_change_allowed.
Deduplicate delivery IDs across notification windows, not just within one read.
Forward the entire message, not only its bell or a text prefix. Set a tool output
budget sufficient for the full packet and preserve partial JSON lines between
reads. If the result is truncated, recover the complete message through
`--read-pending` before ACK; never acknowledge the stdout pump automatically.

Retain the exec cell separately from the follower session. Renew completed
windows on that same follower. After interruption or context recovery, verify
that new output actually arrives; a cell still labelled running is not proof.
Stop an unresponsive owned notification cell before replacing its reader, so
two loops do not consume the same stream.

This mechanism has demonstrated delivery during an active turn. It does not
establish wakeup after a final answer. Report that distinction and keep an
active listening turn open when post-final wakeup is unavailable.

## Acknowledge conversation receipt

The session-scoped helper retains emitted envelopes until explicit receipt.
After this conversation has received the complete conversational projection,
retain its delivery ID and disposition in the conversation record and immediately
run, before any task execution or reply. Acoustic diagnostics are not a reading
prerequisite:

```bash
cs-bus \
  --provider codex --session SESSION_ID --ack DELIVERY_ID [DELIVERY_ID ...]
```

Use the actual provider/session and the same `--bus`/`--bridge-home` overrides
as the follower. Several ids are all or nothing: one id that is not pending
refuses the call and no marker is written. Each accepted id prints one
`acknowledged` line. The immutable marker records `read_at` once: this is the
read receipt, not proof of execution. A complete message received in the
conversation is sufficient to call ACK immediately; stdout emission or provider
queue acceptance alone does not create this marker. For Codex, ACK also withdraws the exact pending native queue
submission. `native_queue_settled: true` means removal was confirmed or no entry
remains pending; false means the ACK was saved but provider withdrawal is still
pending or unresolved. The existing follower retries transport failures in the
background; `--status` exposes `pending_native_withdrawals`. A message already
consumed by the model cannot be recalled. Original envelopes and transcript/audio
history remain retained. This command does not start another reader. A successful
`acknowledged` receipt proves acceptance was recorded, not that a command was
executed. Keep execution disposition separately; do not repeat a completed
action when its envelope is replayed.

Drafts stay observation-only. Acknowledge complete routing-ambiguity notices;
transcription diagnostics do not change task permissions. Report ambiguous recipients
and ask for a clear address instead of choosing one. Never acknowledge from
the stdout pump before the conversation receives the message, from a delivery
ID alone, or after a truncated/incomplete tool result. Unaccepted envelopes
remain in the lease's ordered `pending` array and replay on reattachment.

At 256 pending envelopes or 8 MiB, continuous following waits for receipts
and resumes after space is freed. A non-following read exits 4; acknowledge
accepted items and resume the same lease. Do not clear the queue by deleting
lease files, creating another session, or acknowledging unread messages.
If the installed helper lacks `--ack`, report a generation mismatch; do not
pretend its stdout is durable conversational delivery.

## Diagnostic readers (observation only)

`scripts/bus-tail.sh --all` combines the bus and application log for diagnosis.
Its human view may truncate text; it is not an agent notification mechanism.
Additional readers are observers, never alternate command-delivery authorities.
Keep any requested diagnostic output scoped and redact secrets.
