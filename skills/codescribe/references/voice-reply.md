# Voice reply

Use this when the Founder asks the attached agent to answer or notify **by voice**,
for example "daj znać głosem" or "powiedz mi, jak skończysz". Voice is an output
channel only. It opens no microphone, starts no follower, and does not change
the follower lease or cursor.

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
| `credential_missing`  | No credential found; `grok login` or the Keychain item |
| `credential_rejected` | Token or key refused; the Founder re-authenticates     |
| `quota_exhausted`     | Spending limit or credits used up; the Founder decides |
| `http_<code>`         | Other refusal; report the code                         |
| `network`             | The request did not complete; one retry is reasonable  |
| `playback_busy`       | Another reply held playback beyond the 120-second wait |
| `take_live`           | Take did not end within the 120-second wait            |
| `take_started`        | Take started during playback; audio was stopped        |
| `playback_failed`     | Audio arrived, `afplay` failed                         |

Report a failed reply in chat with its `reason`; do not retry a
`credential_*` or `quota_exhausted` refusal in a loop.

## What to say

- One or two sentences in the Founder's language: what finished, the
  identifier that matters (build, version, run), and the one decision or
  action now waiting on him.
- The chat reply remains the record. Every spoken notice is also written in
  chat.
- Speech reports facts. It never acknowledges an unsealed draft as an
  executed command, and it never announces work that has not been verified.
