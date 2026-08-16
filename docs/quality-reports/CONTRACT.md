# Quality-report HTML contract

Source of truth: `core/quality/engine_contract.rs`.
Visual gold: [`seal-atlas.take01.html`](seal-atlas.take01.html)
(take 01 „no to dobra” — the Claude artifact, locked).

`codescribe-corpus` writes these HTML files. Voice Lab only lists what
passes this handshake. A WER table is a footnote, never the throne.

## Identity

| | |
| --- | --- |
| surface | `seal-atlas` |
| engine | `the-engine/v1` |
| primary key | `pcm_time` |
| Voice Lab kind | `seal_atlas` |

Required `<head>`:

```html
<meta name="engine-contract" content="the-engine/v1"/>
<meta name="quality-report-surface" content="seal-atlas"/>
<title>Seal Atlas — …</title>
```

Title **must** contain `Seal Atlas` (or `seal-atlas`).  
`Codescribe Quality Report` is the retired Qube page. It may still be
written as `quality/qube.{profile}.html`. It is not the report.

## Fact cards

Voice Lab reads:

```html
<div class="stat"><b>VALUE</b><span>LABEL</span></div>
```

Take 01 cards (labels may be PL or EN):

| value | meaning |
| --- | --- |
| `20/20` | word-grain words with ≥75% speech |
| `11` | sealed spans |
| `2` | spans with per-word pins |
| `1` | clock-lie (span 2) |
| `0.5` | Silero threshold |

A corpus atlas without a dump still emits these five `.stat` cards
(values may be `n/a`). Missing the class is a handshake miss.

## Lanes (one take, one clock)

X-axis is the capture PCM sample counter. Required legend tokens:

- `Silero` / `p(mowa)` — production VAD, 32 ms, default threshold
- `word-grain` — SFSpeech actually returned per-word pins
- `utterance-grain` — one segment over the Apple commit-to-commit window
- `clock-lie` / `kłamstwo zegarowe` — first-class finding
- `whisper_words` — Whisper mapped **back** onto the same clock

Forbidden as the live picture: reconstructed timelines, letter timings
presented as measurement, Whisper drawn into Silero silence.

## Required body copy

The HTML must say, in some language:

1. Words come from `SealedSpan.words` / the live dump — not from the final string.
2. Per-word pins are real where they exist and **not guaranteed**.
3. Letter ticks inside a word are even interpolation, not measurement.
4. Clock-lie: too many characters for the claimed PCM duration
   (`CLOCK_LIE_CHARS_PER_SEC = 30`). Take 01 span 2 is the canonical example.

## Files corpus must write

| path | kind | role |
| --- | --- | --- |
| `quality/seal-atlas.{profile}.html` | `seal_atlas` | **the** quality report |
| `docs/quality-reports/seal-atlas.take01.html` | `seal_atlas` | gold take, never regenerated from WER |
| `quality/qube.{profile}.html` | `quality_report` | retired scores surface, footnote |
| `docs/THE_ENGINE_CONTRACT.md` + engine plate | `quality_contract` | doctrine, not a take |

Destination the operator sees: `~/.vibecrafted/artifacts/vetcoders/codescribe`
(or `$CODESCRIBE_ARTIFACTS_DIR`). If the atlas does not land there, Voice Lab
never shows it.

## Forbidden in a Seal Atlas HTML

- title `Codescribe Quality Report` as the H1
- `Avg WER` as a hero / first stat
- treating HQ / Cloud / AI as the live engine
- omitting `engine-contract` or `quality-report-surface` meta
