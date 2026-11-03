# Codescribe CLI

The `codescribe` terminal command is a transcription and diagnostic utility.
The native macOS app owns live microphone capture; launch that app through
Finder, Spotlight or LaunchServices. A CLI invocation is not a second app or
microphone session.

## Install and identify the CLI

For a source installation, `make install` installs the CLI and quality tools.
This builds Rust binaries; it is separate from installing the native app and
from updating a Codescribe client plugin. See [INSTALLATION.md](INSTALLATION.md).

```sh
codescribe --version
codescribe --help
codescribe transcribe --help
```

The CLI and the app can be different generations. Record which executable you
used when reporting a result; a CLI version is not proof of the loaded app's
version.

## Transcribe an audio file

```sh
codescribe transcribe recording.wav --no-bus --no-truth
codescribe transcribe recording.wav --apple --no-bus --no-truth
codescribe transcribe recording.wav --whisper --no-bus --no-truth
```

The examples deliberately avoid publishing onto the transcript bus and avoid
writing an observer sidecar beside the input. Without these flags, those side
effects can be part of the file-transcription path. Do not treat a diagnostic
comparison as a new user delivery.

`--apple` forces Apple's file recognizer without a Whisper fallback;
`--whisper` forces the local Whisper final pass. They cannot be combined.
File recognition is a separate execution from a live recording and needs the
appropriate model or permission. A file comparison does not reproduce every
capture or live-preview condition.

Useful options:

| Option             | Meaning                                                                               |
| ------------------ | ------------------------------------------------------------------------------------- |
| `-l`, `--language` | Supply the file language                                                              |
| `--stream`         | Print admitted segments as decoding progresses, without printing the final text again |
| `--raw`            | Skip Light+ sentence shaping for literal-word inspection                              |
| `--inspect`        | Print the time-axis evidence view on stderr                                           |
| `--no-bus`         | Do not publish this file verdict onto the transcript bus                              |
| `--no-truth`       | Do not write the `<file>.truth.json` observer sidecar                                 |

Several file arguments are processed sequentially. Transcript text goes to
stdout; per-file headings, provenance and warnings go to stderr. Preserve and
inspect warnings: an exit status or a completed output alone does not establish
lexical accuracy or full coverage.

## Follow the running app

```sh
codescribe transcribe live
codescribe transcribe live --json
```

This follows the clean transcript bus owned by the app. It does not open a
microphone. The human view appends drafts, marks reducer revisions and closes a
take at its terminal seal. `--json` exposes the projection JSONL for a machine
consumer. App settings own live capture; a CLI language argument does not
reconfigure the running recording.

`codescribe transcribe last` prints the last completed transcript from the bus
without starting recognition or opening the microphone.

A live transcript observer is not a named agent mailbox. For delivery ownership,
ACKs, replies and provider wakeup use the appropriate
[client integration](../README.md#connect-an-external-agent), not reconstructed
UI text or a second transcript reducer.

## Other command families

Use each command's `--help` before running it:

- `trace`: inspect a session's per-occurrence decision evidence.
- `replay`: replay a recorded take with explicit provenance.
- `lexicon`: inspect or deliberately change pronunciation mappings.
- `report`, `daemon`, `teach`, `corpus`: quality and development workflows.
- `bus`: inspect and maintain bus data; mutation is not ordinary transcript
  viewing and needs the corresponding ownership and idle conditions.

Commands that replay, apply, compact, stage or recover data can write files or
run recognition. Do not run them as a generic “health check”. Keep original
recordings and receipts when making comparisons, and follow
[Privacy & Security](guide/privacy.md) before sharing outputs.
