# W-0b offline measurement protocol and W-5a facts

This protocol does not open a microphone, change settings, install an app or
run a CPU stress workload. A short saved-WAV probe cannot certify live thermals.

## Preserve comparable inputs

Before full comparison, the integrator preserves the baseline executable,
code-signing/version/build/source identity and SHA-256, the candidate executable,
settings snapshot/digest, WAV hash, sample rate, annotated speech duration and
mode. Source baseline is `d2ea631a10f1c13f3c5be3773eb25c1d1f62f9a8`; a source SHA
alone does not preserve or identify a built binary. L1 did not build/preserve an
installed baseline app. Keep authorized measurements under the run artifacts,
not the Founder's settings/session directory.

Record host/model/OS, physical/logical core count, power source, initial thermal
state, foreground/background workload and measurement duration. Use the same
WAV, mode and immutable settings for each side. Measure cold and warm runs
separately. Isolate cloud, on-device and Max provider costs. Full acceptance needs
Dragon and MacBook absolute CPU/RSS/thermal ceilings chosen from measurements;
no invented ceiling is entered here.

The full authorized profile includes idle, continuous dictation, Stop, repeated
takes and post-take idle. Offline replay tests only the available file/replay
corridor; it cannot stand in for microphone admission or sustained live comfort.

## Instrumentation boundaries

Use a monotonic clock throughout each measured process. Attach session/capture
identity, task generation, source revision and applied revision to timing samples.
Do not subtract clocks from different processes without a measured clock mapping.

| Metric                 | Start → end / units                                                                                                                                                              |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Main-thread projection | Dispatch onto main thread → completed projection/paint handling; ms; record every sample and stalls ≥100 ms                                                                      |
| Engine → visible       | Engine revision accepted → matching visible projection; ms; includes queue, reducer, bridge and UI                                                                               |
| Final Light+ tick      | Scheduled on one frozen Raw revision → complete accepted projection; ms; includes queue, execution and commit                                                                    |
| Stop → delivery        | Actual Stop event → accepted destination handoff; ms; record timeout/partial outcomes, not just success                                                                          |
| CPU                    | `%cpu` / 100 = logical cores; 100% means one logical core. Sample process plus its identified helper processes; average, p95, p99 and maxima                                     |
| RSS                    | Bytes per identified process; on macOS `ps rss` is KiB, multiply by 1024. Report samples, p95, peak and slope after repeated takes. Summed helpers may double-count shared pages |
| Trace                  | File bytes / independently annotated speech minutes; goal ≤1 MiB/min. WAV wall duration including silence is not the speech denominator                                          |
| Thermals               | Measured sensor values and OS thermal state; missing sensor = unavailable. Nominal state alone is not proof of no heating                                                        |
| Classifier             | Cold model initialization, first result, complete file analysis, CPU seconds, average cores, RSS and available accelerator metrics                                               |

For short commands `/usr/bin/time -l COMMAND` supplies process CPU seconds and
peak RSS; on macOS its resident size is bytes. Exclude compiler startup when
measuring the already-built artifact. For longer authorized runs collect
`ps -p PID -o %cpu=,rss=` externally and store identified process samples, with
elapsed monotonic seconds. Capture helper identities separately. No burner is
needed. Record power/thermal data separately; unsupported metrics remain null.

Feed collected JSON into the summarizer:

```json
{
  "metadata": {
    "provenance": "measured",
    "baseline_or_candidate": "candidate",
    "build_sha256": "REQUIRED",
    "settings_digest": "REQUIRED",
    "host": "REQUIRED",
    "mode": "REQUIRED"
  },
  "samples": [
    {
      "elapsed_s": 0,
      "cpu_percent": 100,
      "rss_bytes": 1048576,
      "projection_main_ms": 4,
      "engine_to_visible_ms": 12,
      "light_plus_total_ms": 20,
      "stop_to_delivery_ms": 100
    }
  ]
}
```

These values illustrate the input format, not an observed product measurement.
`w0-measure.py` reports nearest-rank p95/p99, sample count, mean/max and a
least-squares RSS trend. Missing values remain null, with zero samples. It
rejects negative/nonfinite measurements and never infers a successful tick.

```sh
python3 scripts/w0-measure.py FIXTURE.wav --telemetry samples.json \
  --trail SESSION.trail.jsonl --speech-seconds ANNOTATED_SECONDS
python3 scripts/w0-measure.py tests/fixtures/p0_b_five_iwo.wav --dry-run
python3 -m unittest discover -s scripts/tests -p test_w0_measure.py
```

The dry run reads a 2.35-second repository WAV and uses five synthetic timing
samples solely to verify the summarizer. It performs no STT/model work. CPU,
RSS and tick metrics stay unmeasured. The three tests check normalization,
percentiles/RSS trend, missing data and invalid samples.

Compare absolute values first, relative change second. The proposed p95 main
thread ≤16.7 ms and engine-to-visible p95 ≤100 ms / p99 ≤250 ms need actual
samples. The Founder's final Light+ tick limit is ≤2 s for its complete corridor.
The trail's current null job handoff times cannot certify that limit or N2.

## W-5a: observed SoundAnalysis facts

Apple documents the built-in classifier and its queried label list in
[SNClassifySoundRequest](https://developer.apple.com/documentation/soundanalysis/snclassifysoundrequest),
and result/error/completion callbacks in
[SNResultsObserving](https://developer.apple.com/documentation/soundanalysis/snresultsobserving).
The local SDK headers and an actual offline run confirm this host's surface.

Build the standalone Objective-C probe (no Swift app/bridge changes):

```sh
xcrun clang -fobjc-arc -Wall -Wextra -Werror \
  -framework Foundation -framework AVFoundation -framework SoundAnalysis \
  -framework CoreMedia scripts/sound-analysis-spike.m -o /tmp/sound-analysis-spike
/tmp/sound-analysis-spike tests/assets/synthetic_speech_tts.wav
```

The executable rejects unreadable input and files longer than 15 seconds.
There is no microphone path. The captured JSON is
[W0_SOUND_ANALYSIS_FACTS.json](W0_SOUND_ANALYSIS_FACTS.json), including all 303
actual labels, request settings and window outputs.

Observed on macOS 27.2 (26B5091g), Apple Swift/Clang toolchain from Xcode:

- `SNClassifierIdentifierVersion1` constructed and completed successfully.
- The query returned **303** classes. Requested coverage: `laughter`,
  `baby_laughter`, `belly_laugh`, `cough`, `sneeze`. No `fart`/`flatulence` label
  was present; there is no exact class for the Founder's requested fart event.
- Default analysis window: **3 s**, overlap **0.5**. The first 2.35-second fixture
  completed with no result windows. A completion callback alone therefore does
  not prove detection. This discovery justified the second, 10.84025-second
  repository fixture.
- On the longer fixture: first result **21.656 ms** after `analyze`; analysis
  **41.771 ms**; model request initialization **123.150 ms**; whole measured
  interval **191.123 ms**. This was a warmed system/model run after the initial
  capability probe; these numbers are not cold-start or live-stream latency.
- Process CPU: **0.109922 s**, mean **0.5751 logical cores** over the complete
  measured interval. Peak RSS: **62,390,272 bytes**. OS thermal state: nominal
  (`0`) in this one short sample. Accelerator cost was unavailable, not zero.
- Six windows were returned, with `speech` first in all six. This is a TTS
  speech fixture, not an annotated laughter/cough/sneeze/overlap corpus. It
  proves invocation, callback timing and cost; it does not establish per-class
  precision/recall or false-positive rates. W-5b remains gated on such data.

No producer, microphone or model was added to the application's active pipeline.
