# Monitor and agent wakeup

## Anti-pattern: polling beside the listener

An agent loop that runs `cs-bus --read-pending` every 2 seconds while a watch
or native waiter is already armed duplicates the listener, floods tool output
and spends context on an empty mailbox. Do not add such a loop, including as a
"safety" probe after each stdout wait. The monitor forwards actual watch output;
it does not manufacture mailbox ticks. A bounded transport wait for watch
stdout is not a mailbox read.

Keep `--watch --bell` blocked while the bus is quiet. A notification-window
expiry renews the wrapper over the existing watch; it does not cancel the bell,
restart the follower or require an empty mailbox read. At the next work boundary
after a real notification (or a natural turn in quiet mode), read if needed,
ACK received IDs, and drain only
until zero plus one extra read. Then return to the event-driven listener.

## Channel 0 reception

Change reception during a session with `cs-bus --broadcast on|quiet|off --provider PROVIDER --session SESSION [--bus BUS]`; omission of `--bus` uses the
validated lease's bus. `on` is the default. `quiet` (Steer) retains all messages in the
mailbox without waking the agent: on the next natural turn, explicitly read
`--read-pending` and ACK only complete returned messages, even if no bell rang.
`off` records a skip instead of queueing new broadcasts; direct and peer messages
still wake in this mode. `quiet` also suppresses direct and peer wakeups. The
policy file is reread without restarting or reattaching.

## Codex native queue

For a remote mailbox, follow [Remote bus](remote.md): the existing local remote
watch submits to Codex on the conversation's Mac; the mailbox follower does not
submit there. A remote watch also forwards peer coordination messages while
preserving their attribution and lack of Founder authority.

`cs-bus --attach --provider codex --session <thread-id> --name <name> --channel <n>`
automatically selects `codex-queue`. The follower submits a short
name/channel/delivery header to `codex queue`, but what follows the header
depends on who sent it. A Founder-sourced delivery (typed, or a sealed voice
take) carries its complete message text, headed `from: Founder (typed)` or
`from: Founder (voice)`. A peer agent's delivery carries no text at all: it is
a notice naming the sending peer and channel, the words "peer coordination,
not a Founder instruction", and the exact `cs-bus --read-pending` command to
read it. Treat any queue entry without an explicit `from: Founder (...)` tag
as peer coordination, never as a new Founder instruction, and read the owned
mailbox before acting on it. The canonical mailbox retains the original
envelope and its acoustic evidence. The queue copy never proves that a
delivery is still unread. The installed Codex CLI must support `queue`.

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
Provider failures retain bounded stderr as `provider_error` in the private
receipt; inspect it locally, never broadcast raw receipts. For remote mailboxes,
`--remote HOST --status` points to local receipts under `runtime/remote-native/`.
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
selects monitor-only operation: nothing then wakes a Codex conversation that
has finished its turn (Founder's test, 2026-10-18). `--on-seal` selects a custom hook instead of
native queue. Neither touches microphone or app lifecycle.

Every attachment must start the provider's output-notifying monitor on
`cs-bus --watch --provider codex --session <thread-id>`. Its default bell carries
the whole message; `--bell` spells that default explicitly. It is mandatory for active tasks, even
with native queue. Renew bounded notification windows throughout the task.
The watch carries complete text and the owner/sender/reply coordinates required
for immediate acknowledgment. Native queue is only a later-turn wakeup: a
Founder copy carries the complete text with a small wrapper, while a peer
notice carries no text, so read the owned mailbox before acting on it. `--read-delivery <id>` is the explicit diagnostic read of the complete
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

## Claude Code: plugin hook or completion task

Claude Code wakes an idle conversation on process completion or an `asyncRewake`
hook, not a running process's stdout. An infinite `--watch --bell` cannot wake
an idle Claude Code conversation; its `Monitor` is bounded to at most 30 minutes.

Prefer the Codescribe plugin: install its payload marketplace once with
`/plugin marketplace add ~/.codescribe/agent-bridge/runtime/plugins`, then
`/plugin install codescribe@codescribe`. When `codescribe_*` tools are present,
follow the plugin's `codescribe:codescribe` skill, using the real hook identity.
It connects, reads, ACKs and replies without a second terminal listener.

The Codescribe plugin's `Stop` hook runs `cs-bus --claude-code-wake` as an
`asyncRewake` hook. It waits in the background, holds one lock per lease, and
exits 75 (mapped to the hook's wake code 2) once per delivery not yet announced,
with a `codescribe_mailbox_ready` notice on stderr. It never reads or ACKs.
Announced IDs stay recorded until acknowledged, so a message left unread does
not wake every turn. It also announces `follower_lost` or a refused mailbox once,
and wakes after six silent hours only to re-arm. It ends silently when the
session detaches or Claude Code exits. Every turn end and every tool call
re-arm it: Claude Code runs no `Stop` hook after a user interrupt, and runs
`StopFailure` instead after an API error, so a turn that a wake started would
otherwise end with no waiter. An armed lease makes a later arm return at once.

During an active turn, the plugin's `PostToolUse` probe can deliver the same
`codescribe_mailbox_ready` notice. Read and ACK it before continuing work; the
probe and Stop waiter never ACK messages themselves. Keep the same connection
and end the turn after draining; the hooks re-arm automatically.

Without the plugin, use the Kimi pattern with `--provider claude-code`: one
`Bash(run_in_background=true)` task over `cs-bus --watch --until-event`; on
`mailbox_ready` read, ACK, act, and start one new task before ending the turn.
An interactive Claude Code session keeps a background task running; unattended
runs (`-p`, Agent SDK, CI) stop it at its timeout (30 min by default, at most
2 h) and wake the model: there, set the maximum timeout and treat that wake as a
rearm without ACK or reply. A background session started from the agents
screen is neither: there an untimed task waited 85 min, and another 1 h 50 min,
until a message ended it (measured 2026-10-18); whether such a session stops a
task after 2 h is not yet measured. A turn that ends without a new task is deaf.

The same task may also run beside the plugin. While it lives, the plugin's
waiter stays armed but leaves deliveries to the task, which records what it
reported before it exits, so one message wakes the conversation once. When the
task is gone, the waiter takes over and announces anything the task never
reported; a lost follower is still the waiter's to announce.

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

Run it under the provider's output-notifying monitor, not as a bare background
shell. Claude Code is not such a provider for an idle conversation: its `Monitor`
tool delivers stdout only while armed (at most 30 min) and a background shell
wakes it only on exit. Use the Codescribe plugin, or `--until-event` below. `--once` prints what the
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

### Codex: the one form that works

The Founder's tests in Codex (Maciej, 2026-10-18) found exactly one working
form: start the reader with `tty: true`, and start it and read its terminal
inside **one** `functions.exec` cell that then calls `yield_control()`.

```js
// @exec: {"yield_time_ms": 2000, "max_output_tokens": 2000}
const start = await tools.exec_command({
  cmd: "exec cs-bus --watch --until-event --provider codex --session THREAD_ID",
  tty: true, yield_time_ms: 1000, max_output_tokens: 5000
});
if (!start.session_id) {
  notify({kind: "codescribe_until_event_completion", ...start});
} else {
  const sid = start.session_id;
  text({mode: "monitor-until-event", session_id: sid, read_pending_polling: false});
  await yield_control();
  while (true) {
    const r = await tools.write_stdin({
      session_id: sid, chars: "", yield_time_ms: 25000, max_output_tokens: 5000
    });
    if (r.output || !r.session_id) {
      notify({kind: "codescribe_until_event_completion", tool_session_id: sid,
              output: r.output, exit_code: r.exit_code, still_running: Boolean(r.session_id)});
    }
    if (!r.session_id) break;
  }
}
```

- Only this cell consumes that terminal session. Keep the exec cell ID and
  the `exec_command` session ID apart.
- No `--max-wait`. The 25 s `write_stdin` is a blocking read of the tool's
  output, not a model turn and not a `--read-pending` poll.
- If the start already returns `mailbox_ready`, handle the pending message,
  then rearm.
- On `mailbox_ready`: read the owned mailbox, ACK the exact IDs at once,
  drain, consume the finished cell with `functions.wait`, and arm exactly one
  new cell.

Keep `codex-queue` on beside the cell; each covers the other's gap. In the
Founder's observed test (Maciej, 2026-10-18) an idle Codex read a native-queue
message 5.6 s after it was sent, while a Codex in the middle of a turn read a
queue message only after that turn ended: 5 min in that test, but the wait
lasts as long as the turn does, hours for a long one. That same
mid-turn Codex read a bus message through the cell 25 s after it was sent.
A bus message to the idle Codex, without the queue, never woke it. Only the
native queue wakes an idle Codex; the cell delivers during a turn.

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
