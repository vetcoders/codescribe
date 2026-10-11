# ElevenLabs arm for preserved native BlackHole takes

`scripts/compare-native-elevenlabs.py` adds a cloud file-decode comparison to a
preserved **native application** series. It never starts recording, changes
audio devices, launches the app, installs a build, or plays a fixture. Collect
native Start → realtime BlackHole → Stop takes first, under the integrator's
existing protocol. Do not use the CoreAudio engine harness as native app evidence.

The cloud result is another hypothesis. Both arms use the same unchanged human
reference and the supplied `SCORER-SOURCE.py`; ElevenLabs is never ground truth.
WER remains a diagnostic metric alongside the native seal, coverage, occurrence
and delivery evidence, rather than a replacement for the
[quality report contract](quality-reports/CONTRACT.md).

## One explicit upload

```sh
python3 scripts/compare-native-elevenlabs.py transcribe \
  --wav /absolute/path/to/input.wav \
  --out /absolute/path/to/new-eleven-run/01 \
  --api-key-file /absolute/path/to/private/key-file
```

The private file may contain only the key or an `ELEVENLABS_API_KEY=...` entry.
It is read without shell evaluation. Neither the key nor request headers are
written to artifacts or command output. No credential or private recording
belongs in Git.

The defaults are `scribe_v2`, Polish, word timestamps, temperature zero,
diarization off, audio-event tags off and `no_verbatim=false`. No keyterms or
transcript editing are requested. The API contract is documented by
[ElevenLabs](https://elevenlabs.io/docs/api-reference/speech-to-text/convert).
The hosted model is identified by its public model ID and request time; a pinned
weight hash is unavailable. This is whole-file cloud decoding, with different
context and finalization from realtime native Codescribe.

Each invocation is one billable request, without automatic retry. A pre-existing
output directory is refused. Failed attempts retain the input, request metadata,
response when available and failure receipt. Retrying requires a new directory;
keep the unsuccessful attempt. The script does not upload the rest of a corpus
automatically.

## Score the same preserved corpus

After explicitly obtaining an ElevenLabs result for each clip:

```sh
python3 scripts/compare-native-elevenlabs.py compare \
  --native-run /absolute/path/to/native-run \
  --eleven-run /absolute/path/to/eleven-run \
  --scorer /absolute/path/to/SCORER-SOURCE.py \
  --out /absolute/path/to/new-comparison
```

`--prior-run /absolute/path/to/older-native-run` can be repeated when an actual
older native series exists for the same source WAV and human reference bytes.
It does not install or run an older application.

The native-run directory contains `MANIFEST.json`, `PROVENANCE.json` and numbered
clip directories. Each clip contains `input.wav`, `reference.txt` when referenced,
`transcript.txt` from the reducer and its original `receipt.json` with `terminal`.
The manifest's `clips` entries carry `id`, `sha256`, `human_reference` and
`reference_sha256`. The ElevenLabs run has matching numbered result directories
produced by `transcribe`. Input WAV hashes, reference hashes and raw cloud
response/text identity are validated before any score is published.

Outputs include per-arm ordered alignments, S/D/I/N/WER, corpus totals, totals
without clip10 and clip10 separately, plus ordered numeric matches. The supplied
scorer preserves repetition and does not split glued digits. Clip09 has no full
human reference and receives no WER. Native generation, terminal seal/refusal and
external delivery status remain distinct from cloud text. Saved engine words or
reducer text alone do not prove visible or external delivery. A report over old
takes is not fresh native acceptance of today's installed build.

Voice Lab discovers closed `RESULTS.json` comparisons in its Reports workspace.
The comparison directory retains reference and per-arm text, ordered alignments,
SHA256 bindings, source generation and optional stage/UI/delivery observations.
The viewer validates these files on each open; it neither recomputes WER nor
uploads audio. A separate Founder-confirmed paste observation supplements the
original receipt without rewriting its original `NOT_PROVEN` status. Generate a
new comparison directory for each later generation; refreshing Reports discovers
it without a manual HTML export.

Offline verification (fake credentials and HTTP responses only):

```sh
python3 scripts/tests/test_native_elevenlabs_comparison.py -v
```
