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
- `sample_rate_hz`, `capture_epoch`, `sample_start`, `sample_end`
- `audio_start_seconds`, `audio_end_seconds`
- clean draft or sealed `text`, structured `segments`, optional
  `pipeline_session_id`
- `words`: PCM-pinned spans (`text`, `session_id`, `capture_epoch`,
  `sample_start`, `sample_end`, `energy_db`, `grain`). `grain` is `word`,
  `phrase`, or `utterance` according to what the backend actually measured.
  Segment seconds are never re-labelled as word timing. Intensity is the
  overlap-weighted capture RMS in dBFS for that exact sample range. Overlay
  live text is not a span source.
- `coverage`: a falsifiable pass/fail receipt. Missing PCM identity, absent
  voiced-energy evidence, unordered/out-of-range spans, omitted anchored text,
  or an unanchored insertion leaves reducer `text` visible but publishes no
  misleading lexical spans and records the failure code.

`transcript_sealed` inherits the draft ledger's PCM identities. Its lexical
signature must match the reducer bytes; L3 punctuation/casing may update the
text of a single phrase span without changing its range. An omission or added
tail clears the spans and fails coverage. A controller seal with no prior
drafts stays honest: times and words are omitted and coverage fails explicitly.

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
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --become --drafts --follow
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
  --provider codex --session <provider-session-id> --name james --drafts --follow
```

The Setup Wizard installs that stable helper only after the operator selects
Codex, Claude Code, or both. The signed app payload is checksum-verified before
installation; runtime commands never depend on a source checkout.

Unnamed agents do not pass. The first emitted line is an attach receipt with a
provider/session lease and cursor. Preserve and poll the follower handle. When a
provider session recovers after compaction, the same provider/session/name
resumes from that cursor, including events appended during recovery, without
replaying the old command. Duplicate names in different provider sessions own
different leases. Non-stale names are exposed without touching audio:

```bash
python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py --active-names
```

Draft/revised envelopes explicitly carry `state_change_allowed: false`; only a
`transcript_sealed` envelope carries `state_change_allowed: true`. The demux
never opens audio or changes transcript text.
