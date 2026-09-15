# Take audio retention

Where a take's PCM lives after the microphone closes. Identity is still the
acoustic ledger (`OccurrenceIdentity`); these paths are storage, not identity
keys. Silero alone decides speech vs silence.

## Four locations

```
mic ─► Recorder
        ├─ spill / stop dump / segment snapshot
        │     ~/.codescribe/takes/codescribe_recording_<ms>.wav
        │     ~/.codescribe/takes/codescribe_segment_<ms>.wav
        │
        └─ retain_session_audio_at  (copy, never removes the source)
              ├─ ~/.codescribe/sessions/<session_id>.wav
              ├─ ~/.codescribe/last_session.wav
              └─ ~/.codescribe/transcriptions/YYYY-MM-DD/*.m4a   (daily archive)
```

Override the root with `CODESCRIBE_DATA_DIR` (already registered). No new env
var.

### 1. `takes/` — scratch (this cut)

`SpillSink` (streaming spill opened at start), the RAM-buffer dump at `stop`,
and `snapshot_wav` all write under `Config::config_dir().join("takes")`.
Filenames are unchanged: `codescribe_recording_<epoch_ms>.wav` and
`codescribe_segment_<epoch_ms>.wav`. The directory is created on demand.

This is the address that used to be `std::env::temp_dir()` (`/var/folders/.../T`
on macOS). The OS no longer purges scratch on its own schedule.

### 2. `sessions/<session_id>.wav` — Bus copy

`retain_session_audio_at` (`app/controller/mod.rs`) copies the scratch file to
`sessions/<session_id>.wav`. Bus-demux identity for a live take is that path.
The source under `takes/` is **not** removed (contract with tests at
`app/controller/mod.rs:7178–7377`).

### 3. `last_session.wav` — latest-take alias

The same controller copy also refreshes `last_session.wav` as a latest-app-take
alias for overlay / `codescribe transcribe last`. Third copy of the same bytes.

### 4. Daily m4a archive

`core/state/history.rs` archives a take into
`~/.codescribe/transcriptions/YYYY-MM-DD/` as paired `HHMMSS_slug_kind.m4a` +
`.txt`. Unchanged by this cut.

## CLI lane (`--bus`) identity

`codescribe transcribe <file>` with the bus on (the default) retains the source
as `sessions/<session_id>.wav`. Demux identity is that path. The CLI lane must
not write `last_session.wav` — a file re-decode is not the last live take.
Finder Quick Action uses `--no-bus` and is exempt.

## Growth until Phase 2

Because retention copies and never deletes the scratch file, `takes/` grows by
roughly what `/var/folders` used to hold. There is no janitor in this cut.

## Phase 2 — open Founder decision ⛔

Not in this dispatch. Claude recommends; the Founder decides:

- retention **moves** the take from `takes/` into `sessions/<session_id>.wav`
  (`rename`, same volume) after all destinations succeeded, and keeps the
  source only on failure;
- `last_session.wav` becomes a **hardlink** to `sessions/<session_id>.wav`
  instead of a third copy.

Both change the "never remove the source" contract of `retain_session_audio_at`.

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_
