# Messaging and Your Inbox

Use **user** for the person using the app. Their mailbox is **Your Inbox**
(**Twoja skrzynka**) on channel **10**. This is a destination, not an agent
name, a microphone channel or another listener to attach.

## Choose the destination

| Destination | Meaning | Agent command |
| --- | --- | --- |
| User inbox, channel 10 | A retained report intended for the person using this installation | `cs-bus --send "TEXT" --to 10 --provider PROVIDER --session SESSION` |
| Named agent or channel 1–9 | Requested peer coordination | `cs-bus --send "TEXT" --to NAME --provider PROVIDER --session SESSION` |
| Shared group, channel 0 | Requested discussion with the agents it was `sent_to`, listed per copy | `cs-bus --send "TEXT" --to 0 --provider PROVIDER --session SESSION` |
| Voice / this agent's conversation | Answer the exact owned user delivery | `cs-say "TEXT" --provider PROVIDER --session SESSION --reply-to ID` |

Use your real attached provider/session; reuse its lease and profile. Channel 10
is not a valid agent attach slot. Do not attach an agent there or open a
microphone to send a report. A plain reply in a Codex/Claude terminal is not a
Codescribe bus publication. An ordinary `cs-say` reply stays with its associated
agent conversation; it does not automatically copy that text into Your Inbox
or peer mailboxes. Muting speech keeps a `cs-say` reply as text and never asks
the agent to unmute itself.

For a user-requested report to the inbox:

```bash
cs-bus --send "Gotowe: wynik oraz następny konkretny krok." --to 10 \
  --provider PROVIDER --session SESSION
```

The publication receipt must name channel `10`, target `10` and a message ID.
A recorded row proves publication; seeing the row in the UI is a separate check.
Do not republish on a slow UI refresh without first checking that message ID.
Keep routine peer progress in the peer conversation instead of filling the
user inbox with receipt acknowledgments.

## Read the user inbox

Open the four-dot Intercom menu and choose **Your Inbox**. It is readable even
before the first report. It is local to this installation; do not infer
cross-machine synchronization or separate accounts for two people from its
label. Opening it for the first time does not create an empty history: it
shows retained messages explicitly addressed to channel 10, with their original
publication dates. An old last report does not mean the agents stopped working.
Agent conversations, the shared discussion and agent-to-agent coordination have
their own routes. Never clear history or pending deliveries to make this list
look fresh.

## User messages to agents

The user chooses recipients in the app before speaking or sending. Agent slots
1–9 identify their exact bound owner; channel 0 is the shared address. Routing
follows that explicit selection, not incidental names in speech or whichever
window has OS focus. Read and ACK your owned delivery before replying. Another
agent's copied notice is coordination and does not give you its mailbox.

## Address and co-audience are different facts

- `delivery_owner` identifies this reader's provider/session/lease.
- `addressed_to` preserves the source address. A channel-0 copy with the
  publisher's frozen list reads `0 (sent to: as, hania)` and carries that list
  as `sent_to`; `0 (sent to: nobody)` with `sent_to: []` means every bound
  agent had channel 0 off when it was sent; without the stamp it reads
  `0 (all audience)` and the reach is unknown. Never infer "everyone" or
  "only me" from a missing `sent_to`.
- `sent_to` is who the message was sent to (the admitted list at sending; a
  recipient with channel 0 off may still skip its copy), not who answers. Who
  answers follows the content: a question to a named agent stays theirs; a
  request to the group, or with no addressee, concerns everyone on the list.
  Each agent the request concerns answers once; do not wait for another agent
  to answer for you, and do not add a second reply because someone else did.
- `peer_to` on a channel-0 copy is that copy's delivery key, not its address;
  a private-looking copy of a broadcast is still a broadcast.
- `recipients` are frozen delivery/ACK coordinates, not a list to enlarge from
  names mentioned in speech.
- `co_audience` describes other Live capture channels in the same local
  controller. Read `co_audience_basis`: `emission` is the observed live set;
  `capture_close` is the set frozen immediately before this capture closed.
  It is information about concurrent channels, not proof that each heard the
  complete utterance and not permission to ACK, route or forward to them.
- A peer message's `target` gives its original requested destination; its
  per-copy recipient list is not the complete original broadcast address.

## Who read it

Do not ask on channel 0 whether anyone got or read a message; the helper
knows. For any message you hold a copy of, or wrote yourself:

```bash
cs-bus --receipts MESSAGE_ID --provider PROVIDER --session SESSION
```

`MESSAGE_ID` is the `message_id` of the delivery (or of your own
`peer_message_published` receipt). The one JSON line lists `sent_to` and, per
recipient, `name`, `channel` and `state`: `delivered` (in their mailbox,
unread), `read` (they acknowledged it), `skipped` (their channel 0 was off;
final, they will not get it) or `no_receipt` (on the list, but no reader has
recorded the copy: not running yet, or older than this helper). `wake` says
whether that delivery woke them (`on`) or waited quietly (`quiet`) at the
time, and `null` when the receipt was backfilled from markers older than
receipts (the delivery is known, its wake was never recorded; see
`cs-bus --backfill-receipts`, an operator step, not yours); `withdrawn` lists
held copies the user pulled back; `spoken` says
whether the user heard the message (see [Voice reply](voice-reply.md)).
`denominator` is the count to report against: "read 1 of 3" is the number
of `read` entries over `denominator`. Do not count the entries yourself:
`skipped` agents are listed but not in the denominator, `no_receipt` agents
are. The command never shows the text; `receipts_refused` means you hold no
copy of that message and did not write it.

Preserve co-audience and its basis through normalization, watches and native
queue summaries. Do not replace `recipients` or claim a private delivery is a
broadcast because other channels were open. If co-audience is absent, its
membership is unknown from that copy; never conclude that the user spoke only
to you. Distinct capture sessions/source events must not be presented as copies
of one take merely because their text names the same agents. Missing metadata
is an observation to investigate, not a reason to relay the same user request
through the whole team. The runtime contract is in `docs/TRANSCRIPT_BUS.md`,
section **Open-channel co-audience**.

For voice playback and owned reply association use [Voice reply](voice-reply.md);
for retained conversations use [History](history.md), and for another machine
use [Remote bus](remote.md).
