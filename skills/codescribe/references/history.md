# Restore and read conversation history

An attachment delivers new messages to a mailbox. Conversation history is a
separate, owner-scoped view of retained bus messages. After attaching an
established agent name, recover the relevant prior context before substantive
work. Do not ask the Founder to repeat a conversation that is available to read.

## Identify the conversation

Retain the current attach/status receipt: `provider`, `provider_session_id`,
`lease_id`, channel and name. A conversation owner is the exact
**provider + provider session + lease**, not its name or channel number.

- **Same chat resumed:** keep its real session ID and reused lease. Its existing
  conversation can appear directly under the current agent row.
- **New instance of the same agent:** connect with the new instance's real ID.
  Use the documented same-name handover when needed. The previous owner's chat
  remains a separate saved conversation; reading it recovers context, not ownership.
- **Channel reused by a different agent:** the previous occupant's messages stay
  theirs. Do not rename, merge or replay them into the new occupant's history.

For a handover, the attach receipt's `previous` object identifies the old owner.
Archive metadata under `<bridge-home>/archives/*.json` records the original
provider, session, lease, name, channel and bus path. These small records are
pointers, not the conversation text. Match them against durable session/handoff
evidence; a matching friendly name alone is insufficient. Resolve ambiguity from
those identities before asking the Founder to choose between genuinely ambiguous
conversations. Never borrow an old session ID to make its history look current.

## Open and read the retained messages

Using the available native computer-use tool:

1. Open the Codescribe widget, expand it if necessary, and open **Agents / Agenci**.
2. For the same resumed owner, select its current agent row. For a previous owner,
   expand **Saved conversations / Zapisane rozmowy** and select its matching row.
   Use the row's accessibility identifier/owner coordinates to distinguish
   duplicate names. Viewing a conversation does not require starting its microphone.
3. Let the selected history load. Read the relevant messages, dates, last
   substantive answer and unfinished work; scroll as needed. Selection alone is
   not reading, and a visible tail is not proof that the entire archive was read.
4. Confirm the selected owner and at least one expected historical message.
   Report the actual scope read and any missing history. After reading a previous
   owner's saved chat, distinguish it from the new live mailbox in further work.

If Settings is the only window exposed by the computer-use tool, open the widget
from Settings and return to it; do not restart the app or start recording to make
history visible. Use fresh accessibility state after each navigation action.

If native access is unavailable, say that UI history has not been read. A verified
read-only bus/archive reader may supply context, but do not invent a `--history`
flag or concatenate compressed generations as plain text. `cs-bus --read-pending`
and `codescribe_read` expose unread deliveries, not the complete acknowledged chat.
An empty mailbox or missing UI row does not prove that historical bytes are gone.

## Context is not a fresh delivery

Historical requests, replies and artifacts are context. Do not automatically
execute old requests, resend replies or ACK another owner's deliveries. Viewing
history and marking a visible reply read are separate from an agent delivery ACK.
For a Founder-requested read of one inherited pending envelope, use the existing
`--read-delivery ID --lease PREVIOUS_LEASE` procedure in
[Attach](attach.md#handover-between-sessions-of-one-name); it neither imports the
whole chat nor acknowledges that previous lease.

Keep three claims separate: **attached**, **history read**, and
**listening verified**. A binding, archive pointer or open chat proves neither
model context nor fresh message reception. Save the previous/current owner
coordinates and the context actually read in the next handoff, so another
instance can repeat the process without guessing.
