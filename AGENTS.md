# Codescribe Local Agent Contract

The Vetcoders Global Agent Charter is authoritative in this repository. This
file adds only Codescribe-specific runtime laws and pointers; it does not create
a second workflow, dispatch plane, or worktree policy.

## Runtime truth

- One shared checkout is the Living Tree. Do not create implementation
  worktrees for this repository. Re-read touched files and preserve concurrent
  work.
- `RecordingController` is the single in-app microphone owner. Dictation,
  Agent, and Assistive gestures may choose different downstream consumers, but
  they must not create parallel recorders.
- `PresentationEmitter` is the transcript reducer of record. The clean
  Transcript Bus observes committed reducer events; UI previews, raw engine
  text, and a second transcription pass are not transcript authority.
- Apple is the first observer, not immutable text authority. A later observer
  may repair wording only when session, capture epoch, and PCM span identity
  prove that it owns the same occurrence.
- Intentional repetition is never duplicate text. Deduplication may reject a
  replayed observation identity; it must not collapse equal words merely
  because their strings match.
- The engine has four machine layers: Apple, Whisper, Lexicon + Light+, and the
  existing Responses formatter. Silero is VAD/time evidence, and
  `SessionFinalised` is lifecycle rather than a Final BAM producer.
- Delivery follows explicit operator intent. OS focus is not a substitute for
  an Agent, canvas, clipboard, or paste route.
- Swift recording state is derived from controller lifecycle. A terminal event
  must always release mic state, UI phase, and Agent-thread ownership.
- An Agent voice capture belongs to the thread selected when capture starts.
  Browsing another thread must not steal the in-flight transcript.
- Diagnostic and CLI consumers follow the Transcript Bus. They must not open a
  competing microphone merely to observe Codescribe.app.

## Canonical contracts

- `docs/STT_CONTRACT.md` — engine and adjudication truth.
- `docs/TRANSCRIPT_BUS.md` — clean event schema, privacy, and path resolution.
- `docs/HOTKEYS_CONTRACT.md` — gesture, ownership, and mode routing.
- `docs/DELIVERY_ROUTE.md` — destination selection when present on the active
  stack.
- `docs/ENV_REGISTRY.toml` — every supported environment variable.

When prose conflicts with executable behavior, establish runtime truth first,
then update both the code and the relevant contract in the same cut.

## Working rules

- Use Loctree before structural edits; use literal search only as the local
  detail lens or explicit fallback.
- Never revert unfamiliar dirty changes. Isolate responsibilities, verify each
  coherent cut, and stage only the files that belong to its checkpoint.
- Local implementation turns end in a scoped commit. Push that work to the
  active branch. Do not merge to trunk, tag, or publish a GitHub Release
  unless the operator asked. The daily notarized DMG below is the one
  release artifact that does not wait for a second ask.
- Generated UniFFI Swift bindings must match the Rust bridge. Run
  `make app-bindings` after bridge API changes.

## Daily install cadence

Operator agreement 2026-08-19.

- After each coherent cut that changes the app, run `make install-if-idle`
  (or `make install-app` after a live-recording check). That is the daily
  laptop binary. Do not wait to be asked.
- **Refuse the install** when a Codescribe take is in flight. Authority is
  the Transcript Bus: last session has `session_started` and no later
  `transcript_sealed`. Never tear down `/Applications/Codescribe.app` mid-take.
- A **notarized slim DMG for the secondary release operator is once per
  calendar day**, not every
  commit and not "after a batch of key fixes". When the bus is idle, cut
  `make release-standard` (sign + notarize + `verify-dmg`). One artifact
  per day is enough; do not recut for later same-day commits unless the
  operator asks. Say the path and staple result in the turn. That is a
  local release artifact; still not a silent merge to trunk, tag, or
  GitHub Release.
- An ad-hoc `/Applications` build from `install-app` is never the distribution
  DMG.

## Verification

- `make check` — static formatting, Clippy, Semgrep, env registry, gate ledger.
- `make verify` — hermetic Rust tests and doctests; this is the CI contract.
- `make test-swift` — regenerate the ignored Xcode project, run phrase-restart
  lockstep, then execute the Swift suite.
- Host-only corpus, real-API, loopback, and parity targets are bench evidence,
  not implicit merge gates. State explicitly which ones ran and which were not
  available.
- Production DMGs use the repository release contract, Developer ID signing,
  notarization, checksum, and `verify-dmg`; never describe an ad-hoc package as
  production.
