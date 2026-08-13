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

Resolution order used by the tests, the bench script, the engine harness and
the `ENGINE_*` Makefile targets:

1. `CODESCRIBE_DATA_ASSETS` — explicit fixtures directory,
2. `~/.codescribe/data_assets` — the canonical local home,
3. this directory (for ad-hoc local drops; never committed).

Shell consumers share one implementation — `scripts/lib/data-assets.sh`
(`dir` / `resolve <fixture>`), pinned by `tests/data_assets_resolution.rs`.
Rust tests carry the same order in their own `data_assets_dir()` helpers.
Hardcoding tier 3 is what left `make test-engine-parity` reporting
`fixture not found` on hosts whose home corpus held the clip — the tier
gitignore keeps empty by design is the one that got baked into the harness.

When no fixtures are found, fixture-driven tests SKIP with a notice — they
never fail on a clean public checkout. The engine harness instead exits 2 and
prints every path it checked, so a cold worker never has to hunt.

W13-0 golden replay WAVs (operator voice, 2026-08-13 takes 171939 / 191351 /
193523) live in `w13/` under this same resolution order. The repo commits
only `tests/fixtures/w13_golden_manifest.json` (relative path + sha256).
`cargo test --test w13_clock_falsification w13_golden_fixture_manifest_loads`
is hermetic; the histogram test measures when the WAVs are present.

## Cold worker: point a run at the corpus

No copying, no committing — name the directory that holds it:

```bash
CODESCRIBE_DATA_ASSETS=/path/to/corpus make test-engine-parity
```

Verify resolution alone (no audio hardware touched):

```bash
./scripts/lib/data-assets.sh resolve 05_apple-live-parity.wav
```

Detailed fixture provenance (source recordings, ffmpeg cut windows,
re-derivation method) lives next to the audio in the local fixtures directory,
not in the repository.
