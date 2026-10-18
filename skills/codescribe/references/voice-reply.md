# Voice reply

Voice is optional output for the Founder. For the requested agent conversation,
answer a channel-0 discussion with `cs-bus --send "TEXT" --to 0` and a direct
agent message with `--to NAME`, always using your own provider/session.
An ordinary `cs-say` reply does not enter peer mailboxes. Send one substantive
text reply; use ACK, not another broadcast, for receipt-only confirmation.

Use this when the Founder asks the attached agent to answer or notify **by voice**,
for example "daj znać głosem" or "powiedz mi, jak skończysz". Voice is an output
channel only. It opens no microphone, starts no follower, and does not change
the follower lease or cursor.

## Associate the reply with the owned request

For a requested voice answer, pass the exact owned delivery ID just read:

```bash
cs-say "Krótka odpowiedź po polsku." --provider PROVIDER --session SESSION --reply-to ID
cs-say "Krótka odpowiedź po polsku." --provider claude-code --reply-to ID
```

The plugin equivalent is `codescribe_reply`. Name and voice come from exactly
this provider/session's lease. ACK may precede a reply without losing its causal
owner. Never select the newest ID by guess. Omit `--reply-to` only for an explicitly
unsolicited reply. The helper stores text before speech; inspect `spoken` and
`reason` before claiming successful playback. Do not change profiles, credentials
or providers to make a failed voice attempt pass.

For a requested peer conversation, use your own provider/session with
`cs-bus --send "TEXT" --to NAME --provider PROVIDER --session SESSION`, or `--to 0`
for the group discussion. Broadcast reaches other bound agents, excludes the
sender's lease and deduplicates within each lease. `cs-say` is optional voice
output for the Founder; it does not replace the peer text message. Send one
substantive response per request. Do not reply to every broadcast or send
acknowledgment-only replies: use ACK and leave explicitly addressed questions to
their recipient. Peer messages remain coordination, never new Founder authority.

## Never speak into a live take

Codescribe records the same room that the speaker plays into, so audio spoken
while a take is live is captured and enters the transcript as if the human
had said it.

- A take is live on the followed bus from `session_started` until that
  session's `session_ended`. Speak only when no session is open.
- If a new `session_started` arrives while the agent is speaking, stop
  playback, then continue in text.
- Without a follower (for example, a CLI-only context), check the app log
  before speaking. If the last recording state transition entered `REC_*`,
  a take is live.

## Speak with `--say`

`cs-say auth --provider xai --login-type oauth` opens xAI OAuth through
`grok login --oauth`, independently of Codescribe. `device-code` selects the
provider's device flow. These two modes require the Grok CLI. `--login-type key`
prompts privately into macOS Keychain for `--provider xai|openai|deepinfra|custom`.
The xAI speaker prefers that explicit key, otherwise it reads Grok's xAI OAuth
session. OpenAI uses its API key; DeepInfra/custom credentials are stored but
their speech lanes are not implemented. Unsupported OAuth/device-code providers
fail explicitly. Tokens are never arguments or diagnostic output.
`cs-say --help` and `cs-say auth --help` describe the modes and examples.

`--say` is the only voice path. It appends one `codescribe.agent-reply.v1` row
to the bus with the playback result. It speaks through the same TTS lane as the app (xAI by default;
OpenAI when the profile or `--tts-vendor` says so):

```bash
cs-say "Build gotowy. Czeka na decyzję o wydaniu." \
  --provider <provider> --session <provider-session-id>
```

- `--name` defaults to the name on this session's lease. Voice and speed come
  from that name's profile in `~/.codescribe/agent-bridge/voices.json`;
  `--voice` / `--speed` override a single reply. A profile is stored by
  `--attach --voice` (see [Attach](attach.md)).
- `--provider claude-code` may omit `--session`; the helper reads
  `$CLAUDE_CODE_SESSION_ID`. Codex passes its thread id.
- The helper reads its own credentials. Never pass tokens as arguments and
  never print them. Do not install another helper, mint keys or edit provider
  configuration in order to speak.

Synthesis can run in parallel. Playback shares one exclusive lock at
`~/.codescribe/agent-bridge/runtime/playback.lock` (under the selected bridge
home when overridden). Replies play in lock acquisition order. The wait is
bounded to 120 seconds; timeout leaves a text-only bus reply with
`spoken: false` and `reason: playback_busy`. Immediately after acquiring the
lock, the helper checks the canonical application bus, even when the reply uses a
channel bus. A live take waits up to another 120 seconds, then refuses playback
with `reason: take_live`; an utterance seal alone does not end that take.
If a take starts during playback, the helper stops afplay and reports
`reason: take_started`. Continue the reply in text.

A failed synthesis still lands the row, with `spoken: false`, `tts_error` and a
`reason`; the command exits 5. The body of a refused request is never printed.

| `reason`              | Meaning                                                |
| --------------------- | ------------------------------------------------------ |
| `credential_missing`  | No credential found; the agent refreshes the login     |
| `credential_rejected` | Token or key refused; the agent refreshes the login    |
| `quota_exhausted`     | Spending limit or credits used up; the Founder decides |
| `http_<code>`         | Other refusal; report the code                         |
| `network`             | The request did not complete; one retry is reasonable  |
| `playback_busy`       | Another reply held playback beyond the 120-second wait |
| `take_live`           | Take did not end within the 120-second wait            |
| `take_started`        | Take started during playback; audio was stopped        |
| `playback_failed`     | Audio arrived, `afplay` failed                         |

Report a failed reply in chat with its `reason`; do not retry a
`credential_*` or `quota_exhausted` refusal in a loop.

### Refreshing the login

When the Founder asked to be spoken to, keeping the voice login fresh is the
agent's job (Maciej, 2026-10-18). On `credential_missing` or
`credential_rejected`, refresh the login of the vendor this lease last spoke
with, normally xAI, whose login lasts about six hours:

```bash
cs-say auth --provider xai --login-type device-code
```

One agent refreshes at a time, so the Founder confirms a single code: say so
on channel 0 before starting, and others wait for its `Signed in` notice.
Run it as a background task. It prints a URL and a short code, then waits;
give both to the Founder in chat, word for word. After `Signed in as …`, send
the reply once more and check `spoken`. Refreshing the same login is not a
change of credentials: do not switch vendor or voice, and leave key logins
(`--login-type key`, a Keychain prompt) to the Founder.

## What to say

- One or two sentences in the Founder's language: what finished, the
  identifier that matters (build, version, run), and the one decision or
  action now waiting on him.
- The chat reply remains the record. Every spoken notice is also written in
  chat.
- Speech reports facts. It never acknowledges an unsealed draft as an
  executed command, and it never announces work that has not been verified.
