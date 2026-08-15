# Clean Transcript Bus

Codescribe publishes one private, append-only NDJSON stream from the committed
`PresentationEmitter` reducer. Dictation, Agent, and Assistive share the same
capture, VAD, STT, correction, and publication path; mode changes only the
downstream paste/format/Agent-delivery consumer.

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
- committed `text`, structured `segments`, optional `pipeline_session_id`

Statuses are `session_started`, `utterance_committed`,
`utterance_revised`, and `session_finalized`. A revision keeps the original
utterance identity. `raw_text` and unstable UI previews never cross this
boundary. Every line is flushed before publication returns, so live consumers
can observe committed speech while recording is still active.

Example consumer:

```bash
tail -F "$HOME/.codescribe/transcript-events.jsonl"
```

That command is the non-XDG default. With `XDG_STATE_HOME` set, follow
`$XDG_STATE_HOME/codescribe/transcript-events.jsonl`; an explicit bus-path
override wins over both.
