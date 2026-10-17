# Explicit CLI transcript consumption

The CLI consumes the product pipeline; it does not require another microphone
for these operations.

| Request                   | Command                                             |
| ------------------------- | --------------------------------------------------- |
| Watch the bus             | `codescribe transcribe live`                        |
| Last completed transcript | `codescribe transcribe last`                        |
| Transcribe an audio file  | `codescribe transcribe /absolute/path/to/audio.wav` |

A file transcription can publish a new `cli_file_verdict` session on the bus.
Run it when requested, not as a read-only probe of live recognition.
`transcribe last` emits text without a trailing newline to avoid turning
insertion into Enter. Confirm current CLI help if an option is unavailable.

For shell-line insertion, the checkout provides
`scripts/codescribe.zsh` (Ctrl-X Ctrl-V). Source it only in the shell the
user intends to configure. For an explicit clipboard request:

```bash
codescribe transcribe last | pbcopy
```

Clipboard writes and terminal insertion require the corresponding user intent.
Do not infer them from a request to listen. Watching CLI stdout does not wake
an agent; use [Monitor](monitor.md) for conversational delivery.

## Typed text with a pasted file

`cs-bus --send-text --channel 0 --attach-file /abs/path` (repeatable; text on
stdin, may be empty) publishes a pointer to a file the app already stored,
typically `~/.codescribe/assets/inline_<sha256>.png`. The helper validates the
path (absolute, under `$HOME`, regular file, 0 < size ≤ 50 MiB), computes the
sha256 and never copies the file. Readers get `attachments` in the envelope and
the bell, plus the `[attachment]` line in the text.
