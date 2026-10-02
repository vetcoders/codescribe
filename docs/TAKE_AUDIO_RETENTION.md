# Take audio retention

Complete captured PCM is preserved regardless of recognizer, formatter, seal,
delivery or UI outcome. Only an explicitly persisted audio retention choice can
expire a completed owned take. This source contract does not certify an
installed build. Recorder completion/coverage remains the recorder's authority;
occurrence identity and seal remain the acoustic ledger's authority.

## Settings and the immutable take policy

Settings > Audio > Audio retention uses a picker with these exact labels:

| Label | Persisted `audio.retention` | Completed audio policy |
| --- | --- | --- |
| Forever | `forever` | Preserve without an age limit (default) |
| 30 days | `30_days` | Expire after 2,592,000 seconds |
| 7 days | `7_days` | Expire after 604,800 seconds |
| 24h | `24h` | Expire after 86,400 seconds |
| Off | `off` | Discard future owned captures after processing/readers settle |

The existing settings schema (version 3, also readable as version 2) contains
`audio.retention`. The in-memory `UserSettings.audio_retention` projects into
`Config.audio_retention` through the sole snapshot loader. `AUDIO_RETENTION` is
only a settings write-router identifier; neither process environment nor `.env`
can override or seed it. Missing/null/unknown strings resolve to Forever.
Explicit malformed writes are rejected without changing the saved choice.
There is no separate retention configuration store.

The existing `CsSettings.audio_retention` DTO reports the effective next-take
choice. Settings writes through `update_config`, reloads the snapshot on success,
and shows the prior effective choice plus the existing error surface on failure.
Root must regenerate Swift bindings for the new DTO field (`make app-bindings`).

Hold, toggle and independent live Agent channels obtain their storage lease from
the same immutable runtime snapshot before microphone admission. Attached-only
channels reuse their owner's capture and do not invent another archive/policy.
A choice changed during a take cannot discard that in-flight capture. Off never
purges captures that started under another choice. A later finite choice applies
to receipted completed audio, using its declared completion time; changing to
Forever stops age expiration. No transcription mode or quality setting changes.

## Existing audio locations

```
Recorder full native PCM
  ~/.codescribe/takes/codescribe_recording_<epoch_ms>.wav
      └─ controller retention, independent of transcript success
           ├─ sessions/<session_id>.wav (hardlink; pinned copy if links fail)
           ├─ last_session.wav (relative symlink to latest spoken session)
           └─ transcriptions/YYYY-MM-DD/HHMMSS_slug_kind.m4a or .wav
```

Daily transcript `.txt` and truth sidecars remain independent. Expiration never
removes transcript text, drafts, diagnostic Bus rows, settings or user-selected
external source files. `BUS_EVIDENCE_RETENTION_DAYS` still governs Bus evidence
only. The existing registered `CODESCRIBE_DATA_DIR` selects the application root.
No new archive, environment variable or dependency is introduced.

Recorder segment/calibration snapshots are not admitted as completed full takes.
CLI `--bus` copies retain their existing semantics and acquire no application
capture completion receipt merely because a file was decoded. External CLI
sources are never deletion candidates. Historic files without trustworthy
completion/ownership metadata remain preserved; filename or mtime is not proof
that a take completed. There is no automatic sweep of unreceipted files.

## Lifecycle and protection

`core/state/history.rs` owns the bounded `audio_retention` module:

- `begin_capture(root, session_id, policy)` runs on a blocking worker before
  recorder start. It holds a shared advisory lock on the pinned data-root inode,
  which excludes expiration across cooperating processes. A refused lease stops
  microphone admission instead of silently dropping the storage guarantee.
- Controller archival runs on `spawn_blocking` and is awaited. Full capture,
  session link and daily audio are registered by object identity after the
  existing publication path runs. A delivery error is propagated after archival.
  Storage/capture failure protects recovery evidence and logs an error.
- `finish_capture` is called by the existing terminal reset, failed-start unwind
  and live-channel close. A detached stop tail holds its own `Arc<CaptureLease>`
  across drain/retention after the successor resets the original session slot.
  No archive/processing worker can be outrun by terminal reset.
- Only the last lease's drop schedules completion metadata on a blocking worker.
  The shared root lock remains held until publication finishes. The timestamp is
  Unix seconds declared after capture/processing settlement. Unknown/zero/future
  timestamps do not expire.
- `AudioReadLease::acquire(root)` is the same shared root lock for owned audio
  readers/retries. Acquire on a blocking worker **before** resolving/opening owned
  audio, and hold through all decoding/processing/path-based retries. An open
  file descriptor preserves its bytes even if a name is removed; that alone does
  not protect a future path-based retry.
- `start_maintenance` is invoked at controller startup and schedules a bounded
  pass every 60 seconds on the blocking pool. Completion also schedules a pass.
  Maintenance takes an exclusive nonblocking root lock and defers during any
  active capture, processing or admitted read lease. It runs no directory walk
  or conversion on an audio callback or the main UI.

Completion metadata lives beside the existing session WAV as
`sessions/<session_id>.audio-retention.json` (`codescribe.audio-retention.v1`).
It is an ownership/completion receipt, never another audio archive or settings
owner. It records the root inode, frozen capture choice, completion timestamp,
retry protection and each published source/session/daily file's relative path,
parent inode, file inode/device, byte length and modification timestamp.
If publication, clock acquisition or ownership admission fails, audio stays.

## Expiration admission and failed deletion

Each pass visits at most 128 session-directory entries, keeping a pinned stream
cursor between passes; it eventually rewinds. It never scans every take/daily
file, and it runs no per-block scan. Reads of receipts are capped at 64 KiB and
16 owned members. The current snapshot's finite choice expires eligible
completed audio; prospective Off is determined by that capture's frozen choice.
Capture/storage recovery protection prevents automatic deletion, including Off.
Unresolved recovery protection is conservative and requires explicit repair;
it is not silently cleared by a successful UI delivery.

Every path must be a relative normal-component path under one of:
`takes/codescribe_recording_<digits>.wav`, `sessions/<same session_id>.wav`, or a
known daily `transcriptions/YYYY-MM-DD/HHMMSS_*.m4a|wav` publication. All directory
walks use pinned descriptors and `O_NOFOLLOW`; regular file type, parent identity,
file identity, length and modification time must still match. Root/ancestor
replacement cannot redirect a held descriptor. No recursive deletion occurs.

All members are validated before removal. The latest alias is removed only if it
is the known relative symlink to this session; a successor's alias stays. Each
admitted name moves to a deterministic `.expiry-<name>.tmp` in its pinned parent;
the moved inode is checked before unlink. A substitution remains preserved and
reported rather than deleted. A refused unlink keeps that pending name and the
completion receipt. Later passes verify/retry pending names. Every admitted
source, session hardlink and daily copy must disappear before the receipt is
removed; deleting a single hardlink is not certified as full expiration.

Failures are retained in `MaintenanceReport.failures` and logged at error level;
lease/refusal errors log preservation. Parent/entry substitution, clock doubt,
corrupt metadata and incomplete ownership are conservative refusals. The
cooperative root lock is not an access-control boundary against a same-user
process deliberately editing the data directory. Automatic expiration is not
secure erasure of storage blocks, snapshots or user-created external copies.

## Integrator admission boundaries

This worker changes only source and static evidence. Build, tests, binding
regeneration, installation and real stores are NOT_ASSESSED.

Before accepting the complete reader contract, the integrator must wire
`AudioReadLease` into owned-audio consumers outside this worker's source fence:
CLI file/retry processing, the agent `transcribe_audio` tool, and bridge session
retranscription/word-clip reads. Those unleased path-based operations are not
certified protected by this source checkpoint. Full recorder retention failures
belong to paired worker E; its WAV path and typed failure contracts stay intact.

Integrator regression obligations include all five setting round-trips;
unknown/missing/environment input; policy changes during a take; recognition,
seal and paste failure; long full PCM coverage; all hardlinks/daily/alias copies;
active capture/read/retry leases; bounded maintenance progress; future/unknown
clocks; corrupt receipts; directory/file/symlink substitution; interrupted and
failed unlink retries; preservation of text and external CLI sources. Use only
isolated synthetic stores. No live store cleanup is authorized by this document.
