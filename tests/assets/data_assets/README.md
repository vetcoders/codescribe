# Data assets — STT fixtures

Polish dictation clips (`NN_slug.wav`, mono 44.1 kHz s16) paired with
`NN_slug_human_transcription.txt` — what was actually SAID, used as the
vocabulary-coverage reference by the engine e2e tests.

## 05_apple-live-parity — the reversed-TDD parity spec

Source: `Screen_Recording_2026-07-25_at_01.09.14.mov` (operator's evidence
recording; audio stream 0:1, aac 48 kHz stereo). Cut `[133.15 s – 274.0 s]`:

```
ffmpeg -ss 133.15 -to 274.0 -i Screen_Recording_2026-07-25_at_01.09.14.mov \
  -vn -ac 1 -ar 44100 -af "afade=t=in:st=0:d=0.15,afade=t=out:st=140.25:d=0.6" \
  -c:a pcm_s16le 05_apple-live-parity.wav
```

The window is exactly the span heard by the SYSTEM Apple live dictation
running in the left editor window: it was armed mid-sentence — its transcript
opens on "I z filmem równie…" — and 274.0 s is the screenshot moment
(`05_bug_04-34.jpg`, cursor after "Apple"). Trimming to this window is
load-bearing: audio outside it was never seen by the reference engine, so a
wider cut hands us words the reference cannot contain and makes exact parity
unreachable.

The 133.15 s start is MEASURED, not guessed. A wider first cut (124.3 s) was
run through the streaming bridge, whose phrase finals carry timestamps: the
"i" of "nagrywaniem i z filmem" lands at ~9.0 s into that cut
(124.3 + 9.0 ≈ 133.3), with 0.15 s of headroom so the word survives the
fade-in. Re-derive the same way if the reference is ever re-cut.

Two references, two different truths:

- `05_apple-live-parity_apple_live_reference.txt` — VERBATIM what the system
  Apple live engine wrote in the left window (transcribed letter-for-letter
  from the hi-res screenshot, including its own artefacts: "równie",
  "prze zajebisty", "beznadz"). This is the **parity bar** for
  `e2e_apple_live_parity`: our capture path through BlackHole must reproduce
  what the same neural engine produced from the same audio. Do not "fix" this
  file — its errors are part of the spec.
- `05_apple-live-parity_human_transcription.txt` — what was actually SAID
  (derived from the screenscribe timestamped transcript of the recording).
  Used for coverage-style reporting and the before/after confrontation
  report, not as the parity gate.

Confrontation counterpart (what our pipeline produced from this same speech
that night, machine-local, not committed):
`~/.codescribe/transcriptions/2026-07-25/015148_zrobic-teraz-zrobic_raw.txt`
— 234 characters out of 417 s of speech.
