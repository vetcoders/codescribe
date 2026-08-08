# Apple On-Device STT Bridge

This directory contains the Apple STT backend for codescribe:

- `mod.rs` — Rust `TranscriptionAdapter` + subprocess bridge client + final-pass verdict helper.
- `codescribe-stt-bridge.swift` — Swift bridge executable with per-locale backend selection.

## Backend selection (per locale)

| Priority | Backend                                | When                                                                               |
| -------- | -------------------------------------- | ---------------------------------------------------------------------------------- |
| 1        | `SpeechTranscriber` (SpeechAnalyzer)   | Locale is in ST supported+installed catalog                                        |
| 2        | `DictationTranscriber` (SpeechAnalyzer) | **Opt-in only** — `CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1` and locale supported+installed |
| 3        | `SFSpeechRecognizer` on-device         | ST lacks the locale **and** `supportsOnDeviceRecognition` is true (e.g. **pl-PL**) |
| 4        | Error                                  | No backend can serve the locale                                                    |

SFSpeechRecognizer is the current public dictation-class API and the product's
foundation for Polish — **not** a "legacy" path. Whisper remains the fallback
engine, tail-patch donor, and quality second opinion when Apple fails.

### DictationTranscriber lane (W4-A PoC — off by default)

`DictationTranscriber` is the SpeechAnalyzer module backing the SYSTEM dictation,
and unlike `SpeechTranscriber` its catalog **includes pl-PL**. Measured on
macOS 27.0 / SDK 26.5 with the frozen `05_apple-live-parity` fixture (140.85 s):

| Lane                                | vs Apple live ref | vs human | word ratio | wall  | repeatable |
| ----------------------------------- | ----------------- | -------- | ---------- | ----- | ---------- |
| SYSTEM Apple live dictation (frozen) | 1.000 (identity)  | 0.805    | 0.88       | —     | frozen     |
| `DictationTranscriber`               | **0.947**         | 0.810    | 0.99 / 0.87 | ~2.4 s | byte-identical over 3 runs |
| SFSpeech streaming (shipped, W0-A)   | 0.898–0.931       | —        | —          | —     | no (spread 0.033) |

Two things the lane must not be read as: it is **not** enabled, and the numbers
above are a single-fixture PoC. Harder vocabulary clips (01–04) score 0.47–0.80
against the human transcript — the loss there is word-level recognition on
deliberately hard terms plus digit-vs-word normalisation, not truncation (head
and tail are both present). Any product default change is an operator decision.

Arming it also requires `AssetInventory.reserve(locale:)`, which the bridge does
for you: an unreserved locale reports `AssetInventory.status == .supported` and
the analyzer then yields **zero results with no error**, even though
`DictationTranscriber.installedLocales` lists it.

## Why Subprocess Bridge

Apple speech APIs are Swift-first. Keeping Swift in a separate executable gives:

- fast integration with low Rust-side risk
- clear failure boundaries and easy fallback to Candle Whisper
- no Rust FFI surface to maintain across Apple SDK changes

## Build Bridge

Pin the host triple so binaries do not inherit the builder's macOS version
(cross-machine drift). The Makefile default is `ENGINE_BRIDGE_TARGET=arm64-apple-macos26.0`.

```bash
# preferred: Makefile recipe (Info.plist section + codesign + target pin)
make target/release/codescribe-stt-bridge

# manual (same -target as Makefile / scripts/build-app.sh)
swiftc -O -target arm64-apple-macos26.0 \
  -o codescribe-stt-bridge core/stt/apple_stt/codescribe-stt-bridge.swift
```

`make app` / `scripts/build-app.sh` builds this helper and bundles it in:

```text
Codescribe.app/Contents/MacOS/codescribe-stt-bridge
```

For local bridge development without rebuilding the app, set:

```bash
export CODESCRIBE_APPLE_STT_BRIDGE=/absolute/path/to/codescribe-stt-bridge
```

When the override is unset, the resolver checks the bundled helper beside the
current `.app` executable first, then falls back to `codescribe-stt-bridge` on
`PATH`.

## Runtime Env

- `CODESCRIBE_STT_ENGINE=auto` uses Apple on-device on supported macOS and falls back to Candle Whisper when unavailable.
- `CODESCRIBE_STT_ENGINE=apple` forces the Apple path while preserving runtime fallback to Candle.
- `CODESCRIBE_APPLE_STT_BRIDGE=/absolute/path/to/codescribe-stt-bridge` (optional dev override; wins over bundled helper and `PATH`)
- `CODESCRIBE_APPLE_STT_LOCALE=pl-PL` (optional; defaults to `pl-PL`)
- `CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD=1` (allow SpeechTranscriber asset install via `AssetInventory`)
- `CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1` (arm the DT PoC lane; default off).
  Read by both sides — the bridge child inherits it, so one key arms the whole path.

On unsupported hosts (non-macOS or macOS < 26), Codescribe logs a warning and falls back to Candle Whisper.

## Bridge protocol

JSON stdin request / JSON stdout response, `protocol_version: 1`.

Commands:

| command           | Engine surface                                                                                                                 | Use                                              |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------ |
| `probe`           | ST → SF capability                                                                                                             | locale readiness + honest `speech_auth`          |
| `request_auth`    | `SFSpeechRecognizer.requestAuthorization`                                                                                      | force TCC dialog / inspect auth                  |
| `transcribe_live` | **Virtual mic**: `AVAudioEngine` → mixer tap → `SFSpeechAudioBufferRecognitionRequest` (multi-phrase accumulate, >55s chunked) | **Live dictation** (product + Teacher live leg)  |
| `transcribe`      | `SFSpeechURLRecognitionRequest`                                                                                                | File final-pass only (known collapse on long pl) |

Additive fields (no wire version bump):

- `backend`: `speech_transcriber` | `dictation_transcriber` | `sf_speech_recognizer` (probe + transcribe)
- `speech_auth`: `not_determined` | `denied` | `restricted` | `authorized`

### File feeder truncation (fixed 2026-08-08)

The analyzer file feeder (`streamAudio`, ST lane only) handed
`AVAudioConverter` one source buffer per `convert` call and answered
`.endOfStream` to every further pull. With a sample-rate conversion in play the
converter therefore ended the whole stream on the second outer iteration:
**1 buffer / 1486 frames** of a 140.85 s fixture reached the analyzer instead of
2 253 600 — a ~0.09 s window, which is why ST `transcribe` returned empty text
and zero segments. The feeder now pulls from the file inside the input block and
treats `framePosition >= length` as EOF (reading past it throws a bare ObjC
failure with no `NSError`, which the naive shape reports as a transcription
error). Measured after the fix: coverage 1.0000, last segment ends at 140.82 s.

### Why buffer API (not file URL) for live

Apple's **file** engine (`SFSpeechURLRecognitionRequest`) under-generates / returns
empty on long Polish fixtures. Product live must exercise the **buffer** engine
(`SFSpeechAudioBufferRecognitionRequest`) — the same request type hardware mic
taps use. Fixture audio plays through a muted `AVAudioEngine` with a mixer-bus
tap appending PCM into that request. Multi-phrase `isFinal` results are
**accumulated** (never settle on the first phrase — that was the live under-gen
bug). Windows >~12s are chunked with a short overlap (on-device buffer
hypothesis collapses on longer continuous dumps). No BlackHole required for
e2e/Teacher.

Rust live path (`transcribe_via_bridge` / chunk commits) calls `transcribe_live`.

**File final-pass does not use Apple.** `stt::transcribe_file_verdict` always
routes full-WAV adjudication to Whisper when a final re-pass is needed. Apple
`SFSpeechURL` on long Polish fixtures returns a tail fragment (0–66 chars) that
can still beat a broken live stream on pure length — that is a product bug, not
a second opinion. Teacher may still invoke bridge `transcribe` (URL) offline to
contrast engines; the controller/router must not.

## Backend order (supported **and** installed)

1. **SpeechTranscriber** — only when the locale is in the ST catalog **and**
   the model assets are installed (optional download when
   `CODESCRIBE_APPLE_STT_ALLOW_DOWNLOAD=1`).
2. **DictationTranscriber** — only when armed *and* supported+installed. Unarmed,
   this rung does not exist and the order below is byte-for-byte the shipped one.
3. **SFSpeechRecognizer on-device** — when the analyzer lanes lack the locale, or
   are in the catalog but assets are missing, and SF supports on-device
   recognition for that locale (notably **pl-PL**).
4. Honest error — only when no backend can serve the locale.

The pure fall-through table is modelled in Rust
(`probe_backend_fallthrough` + `DictationLane` in `mod.rs`) so the ordering is
testable without a bridge process.

A stalled SFSpeech callback is cancelled after ~2.5 s
(`CODESCRIBE_SFSPEECH_DEADLINE_SECS` override) so Whisper fallback is not
blocked for the full 30 s bridge timeout.
