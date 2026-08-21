# Clean Transcript Bus

Codescribe publishes one private, append-only NDJSON stream containing mutable
live transcript drafts and one immutable product seal. Dictation, Agent, and
Assistive share the same capture, VAD, STT, correction, and publication path;
mode changes only the downstream paste/format/Agent-delivery consumer.

This bus is an observer. It does not open a microphone, scrape SwiftUI, or
re-transcribe saved audio.

## Path contract

Resolution order:

1. `CODESCRIBE_TRANSCRIPT_BUS_PATH`
2. `$XDG_STATE_HOME/codescribe/transcript-events.jsonl`
3. `$CODESCRIBE_DATA_DIR/transcript-events.jsonl`, with the normal default of
   `~/.codescribe/transcript-events.jsonl`

The parent directory is created when needed. On Unix the file is forced to
mode `0600`. A control-plane bridge can consume it with an ordinary follow/tail
reader; no host, date, room, or control-plane path is embedded in Codescribe.

## `codescribe.transcript.v1`

Each line is one JSON object with:

- `sequence`, `session_id`, `mode`, `utterance_id`, `emitted_at`, `status`
- `sample_rate_hz`, `sample_start`, `sample_end`
- `audio_start_seconds`, `audio_end_seconds`
- clean draft or sealed `text`, structured `segments`, optional
  `pipeline_session_id`
- `words`: PCM-pinned spans (`text`, `sample_start`, `sample_end`, optional
  `energy_db`, `grain`). `grain` is `word` when the engine supplied pins and
  `utterance` when the span is the commit-to-commit window. Intensity is the
  overlap-weighted capture RMS in dBFS for that sample range. Overlay live
  text is not a word source.

`transcript_sealed` inherits the draft ledger's sample range, segments, and
words. A controller seal with no prior drafts stays honest: times and words
are omitted.

Statuses are `session_started`, `utterance_draft`, `utterance_revised`, and
`transcript_sealed`. A revision keeps the original utterance identity.
`UtteranceFinal` and engine `SessionFinalised` are working boundaries, not
product truth: Smart/Always final pass, adjudication, dictionary cleanup, and
formatting can still change the entire result. Only the controller output used
for history and delivery becomes `transcript_sealed`; the bus rejects all later
machine writes. `raw_text` and unstable character-by-character UI previews never
cross this boundary. Every line is flushed before publication returns, so live
consumers can observe drafts while recording is active.

Example consumer:

```bash
tail -F "$HOME/.codescribe/transcript-events.jsonl"
```

That command is the non-XDG default. With `XDG_STATE_HOME` set, follow
`$XDG_STATE_HOME/codescribe/transcript-events.jsonl`; an explicit bus-path
override wins over both.

Named external agents (same file, not a second microphone):

```bash
python3 scripts/bus-demux.py --become --follow
python3 scripts/bus-demux.py --name james --follow
```

Unnamed agents do not pass. The demux never opens audio.

The Codex voice bridge composes that named mailbox with a local Codex App
Server client and cancellable macOS TTS. It submits only seals and uses live
addressed drafts only for barge-in. See [`CODEX_VOICE_BRIDGE.md`](CODEX_VOICE_BRIDGE.md).
