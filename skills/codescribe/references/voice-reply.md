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

## Helper

Speak through the host's text-to-speech helper; do not synthesize ad hoc.

- On the Founder's machines the helper is `speak-xai "text"`. It uses xAI TTS
  in the same speech lane Codescribe's own agent uses (PCM 24 kHz, speed
  1.25), with voice `leo`, authenticated by the local `grok login` session.
  `speak-xai --dry "text"` synthesizes without playing, for checks.
- If no helper exists, say so in chat. Do not install a helper, mint keys or
  edit provider configuration in order to speak.
- The helper reads its own credentials. Never pass tokens as arguments and
  never print them.

## What to say

- One or two sentences in the Founder's language: what finished, the
  identifier that matters (build, version, run), and the one decision or
  action now waiting on him.
- The chat reply remains the record. Every spoken notice is also written in
  chat.
- Speech reports facts. It never acknowledges an unsealed draft as an
  executed command, and it never announces work that has not been verified.
