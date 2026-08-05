# Data assets — local-only STT fixtures

This directory is intentionally EMPTY in the repository and ignored by git.

The engine e2e tests and `scripts/bench-stt.sh` exercise real Polish dictation
clips (`NN_slug.wav`, mono 44.1 kHz s16) paired with reference transcripts:

- `NN_slug_human_transcription.txt` — what was actually said (vocabulary
  coverage reference),
- `NN_slug_apple_live_reference.txt` — verbatim output of the SYSTEM Apple
  live dictation for the same audio (engine-parity reference).

These recordings are real operator speech. They are private data and MUST NOT
be committed — this burned us twice (deprivatize 2026-07, regression in #68).
The directory is gitignored except for this README; the pre-commit denylist is
the second fence.

## Where fixtures live

Resolution order used by the tests and the bench script:

1. `CODESCRIBE_DATA_ASSETS` — explicit fixtures directory,
2. `~/.codescribe/data_assets` — the canonical local home,
3. this directory (for ad-hoc local drops; never committed).

When no fixtures are found, fixture-driven tests SKIP with a notice — they
never fail on a clean public checkout. Detailed fixture provenance (source
recordings, ffmpeg cut windows, re-derivation method) lives next to the audio
in the local fixtures directory, not in the repository.
