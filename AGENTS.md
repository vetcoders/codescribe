# Codescribe Local Agent Contract

The Vetcoders Global Agent Charter is authoritative. This file adds only
Codescribe-specific runtime laws, release cadence, and canonical pointers.

## Runtime authority

- `RecordingController` is the only in-app microphone owner. Dictation, Agent,
  and Assistive may route differently downstream; none may create a recorder.
- `PresentationEmitter` is the transcript reducer of record. The Transcript Bus
  observes committed reducer events; previews and raw engine text are not truth.
- Only occurrence-authenticated ledger receipts may create committed Bus
  projections. Raw final/correction/patch events are telemetry or diagnostics.
- Preview is ephemeral overlay paint. It never writes delivery or Bus state.
- A terminal ledger seal closes committed Bus truth; no arbitrary text seal,
  draft publication API, or raw-event delta reducer exists.
- Apple is the first observer, not immutable text authority. A later observer
  may repair the same occurrence only with matching session, epoch, and PCM span.
- Equal words may be intentional repetition. Reject replayed observation
  identity, never text merely because its string repeats.
- Machine layers are Apple, Whisper, Lexicon + Light+, and the Responses
  formatter. Silero supplies VAD/time evidence; `SessionFinalised` is lifecycle.
- Delivery follows explicit operator intent, never OS focus alone.
- Terminal events release microphone state, UI phase, and Agent-thread ownership.
- Agent capture stays bound to the thread selected when capture starts.
- Diagnostic and CLI consumers observe the Transcript Bus; they never open a
  competing microphone.

## Canonical contracts

- `docs/STT_CONTRACT.md` — engines and adjudication.
- `docs/TRANSCRIPT_BUS.md` — clean events, privacy, and path resolution.
- `docs/HOTKEYS_CONTRACT.md` — gestures, ownership, and mode routing.
- `docs/DELIVERY_ROUTE.md` — destination selection.
- `docs/ENV_REGISTRY.toml` — supported environment variables.

When prose conflicts with executable behavior, establish runtime truth and
update both code and the relevant contract in the same cut. After Rust bridge
API changes, regenerate UniFFI Swift bindings with `make app-bindings`.

## Daily app and release cadence

- After a coherent app-changing cut, run `make install-if-idle` (or
  `make install-app` after a live-recording check).
- Refuse installation while the Transcript Bus shows `session_started` without
  a later `session_ended` (the controller's text-free lifecycle terminal;
  `transcript_sealed` is honoured for legacy buses); never tear down the app
  mid-take.
- Cut at most one `make release-standard` notarized slim DMG per calendar day
  when the bus is idle. Recut only when the operator asks.
- An ad-hoc `/Applications` build is not a distribution DMG. Production DMGs
  require signing, notarization, checksum, stapling, and `verify-dmg`.

## Verification

- `make check` — formatting, Clippy, Semgrep, env registry, and gate ledger.
- `make verify` — hermetic Rust tests and doctests; this is the CI contract.
- `make test-swift` — bindings/project regeneration plus the Swift suite.
- Report host-only corpus, real-API, loopback, and parity evidence explicitly;
  these are bench evidence, not implicit merge gates.
