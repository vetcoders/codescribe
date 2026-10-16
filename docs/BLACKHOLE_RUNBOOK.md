# BlackHole: calibration and native app acceptance

## Recovered historical entry point (2026-10-14)

AICX session `01a12678-3636-75b3-8ab7-26912ba5c434` records the October 10–11
native comparison. Its archived Swift player is byte-identical to
`scripts/audio-play-to-device.swift` (SHA256
`372ba216ad70831cfdf0466d06ec626b35699ac31cb20e0347c3210c54d04b32`).
Use that device-bound player. The older `play-preserve-gain.swift` is a different
experiment, not a replacement inferred from its filename. Voice Lab displays
the resulting reports; its currently installed directory contains no Swift player.

Keep system output at its saved pre-test device; select BlackHole in Codescribe.
Changing system output to BlackHole was a deviation in the recent raw-afplay
series. Do not repeat that deviation for this protocol. Decode transcript-bus
storage records through `scripts/bus-demux.py` `ChunkDecoder`; raw JSONL rows
alone can omit chunked events. If capture ends before playback, classify the
trial as interrupted and never blindly click a toggle that could start a new take.

Use this entry point instead of reconstructing past agent sessions. Commands
below are run from the repository root on the Mac running Codescribe, not on
the VNC client. No physical microphone or speakers are needed.

## Ownership and evidence

One integrator owns BlackHole playback and capture at a time. Record host,
session, installed build/commit, resident process start time, source WAV SHA256,
player command/gain, selected input and calibration profile in a private run
directory. An installed bundle does not identify an older resident process.
Do not restart the app, stop a Founder's take, or start another player to obtain
a free device. STT workers must not run this procedure under compile embargo.

There are three distinct operations:

- **Calibration handoff:** the agent plays a file; the Founder clicks calibration
  in Settings. Do not click native Start or Stop on their behalf.
- **Native acceptance:** Codescribe.app Start → file playback → Stop → terminal
  event and actual delivery. This is the required app test.
- **Engine harness:** `scripts/e2e-blackhole-dictation.sh` runs Cargo capture tests.
  It does not prove the installed app, overlay or native delivery works.

## Choose and preserve the playback protocol

The repository's device-bound player targets BlackHole directly, without
changing the system output:

```sh
WAV='/absolute/path/to/fixture.wav'
shasum -a 256 "$WAV"
afinfo "$WAV"
swift scripts/audio-play-to-device.swift --list x
swift scripts/audio-device-unmute.swift 'BlackHole 2ch'
swift scripts/audio-play-to-device.swift 'BlackHole 2ch' "$WAV"
```

**This player normalizes toward −1 dBFS with a gain cap of ×32.** Save both its
normalization and rendered-peak output. It currently has no raw-gain switch.
Its mixer measurement proves rendering, not capture from BlackHole.

The October 13 build-941 native series instead used raw `/usr/bin/afplay`.
To reproduce that series, snapshot defaults, select BlackHole output, verify
the selection, play, and restore the snapshot after the handoff/test:

```sh
RUN=$(mktemp -d /tmp/codescribe-blackhole.XXXXXX)
swift scripts/audio-default-devices.swift save "$RUN/audio-defaults.env"
swift scripts/audio-default-devices.swift set-output 'BlackHole 2ch'
swift scripts/audio-default-devices.swift get-output
swift scripts/audio-device-unmute.swift 'BlackHole 2ch'
# Continue only if get-output returned BlackHole 2ch.
/usr/bin/afplay "$WAV"
# After the Founder has finished calibration or the test has ended:
swift scripts/audio-default-devices.swift restore "$RUN/audio-defaults.env"
```

Use an EXIT trap for restoration in an automated wrapper. Restore the exact
snapshot, including intentional BlackHole defaults; do not guess a physical
device. Keep the snapshot outside a temporary directory scheduled for removal.
Do not switch between raw and normalized playback within a comparison or
between calibration and its subsequent test. Source bytes alone do not define
the signal reaching the app.

## Prove the loopback before judging transcription

List the current audio devices. Their numeric indices are host-local and can
change; never assume `:0`:

```sh
ffmpeg -hide_banner -f avfoundation -list_devices true -i ''
```

The listing command normally exits nonzero. In a second terminal, start a
bounded capture **before** playback of a known speech-bearing passage:

```sh
timeout -k 3 20 ffmpeg -y -hide_banner -f avfoundation \
  -i 'none:BlackHole 2ch' -t 10 -ar 48000 -ac 2 "$RUN/loopback.wav"
ffmpeg -hide_banner -i "$RUN/loopback.wav" \
  -af astats=metadata=0:reset=0 -f null -
```

If name selection is rejected, use the BlackHole index from that fresh listing.
`timeout` must be available before starting the probe. Preserve the capture and
logs. Confirm nonzero samples during source speech, and compare channel levels,
duration and speech/silence placement with the source. Noise alone is not proof
that the intended fixture arrived. A short capture inside a source pause proves
nothing about the rest of the route.

If capture is zero or stalls, resolve routing, BlackHole device mute/volume,
and Microphone permission for the actual capture process first. Permission for
Codescribe and permission for Terminal/agent capture are separate. A player PID,
exit zero or rendered peak is not sufficient. Do not report an STT regression
from an unverified loopback.

## Calibrate, then run the native application

1. In Codescribe Settings → Audio, verify selected and actually used input are
   BlackHole 2ch. Preserve the previous input and calibration evidence.
2. For a Founder calibration handoff, play speech using the chosen protocol and
   announce that playback has started. The Founder owns the calibration click.
   Confirm the new attempt succeeded; a pre-existing green Ready label is not
   proof. `calibration_refused: no active speech was measured` is a failed attempt.
3. For native acceptance, choose the intended delivery destination and observe
   a new `session_started` from the app **before** starting the fixture. Retain
   that session ID. Do not feed a second engine harness simultaneously.
4. Let playback finish. Issue native Stop only for the test-owned active take;
   retain the `session_ended` receipt, including `sealed` and refusal reason.
   Use a bounded wait and report timeout separately from success.
5. Preserve source hash, captured WAV, transcript events, ledger/qualification
   traces, final rendered text and actual destination text. Compare with the
   unchanged human reference, including repetitions, head, tail and silence.
6. Restore test-owned audio settings after calibration/test ownership ends.

For AX automation, inspect `AXIdentifier` before clicking:
`overlay-start-recording`, `overlay-stop-recording`, `overlay-tools-handle`.
Window/group indices change when Settings opens. Never use a blind toggle or
assume window 1 is the overlay. Computer Use is unnecessary for this protocol.

## Existing engine harness caveats

`scripts/e2e-blackhole-dictation.sh "$WAV"` owns an exclusive harness lock,
preflight and player lifecycle. Its capture test starts its own player; do not
start another one. The script compiles and executes tests, so only the assigned
integrator may run it. Direct Cargo uses the shared main-checkout target.

Its default exit cleanup can force BlackHole defaults back to named physical
devices. For intentional BlackHole defaults use its documented
`FORCE_LEAVE_BH_DEFAULTS=1` setting to retain exact snapshot restoration. Review
the script before execution; this runbook does not authorize an app restart.

## October 14 handoff: unresolved, not a known-good recipe

The last raw-playback attempt produced a three-second all-zero loopback probe
and a Founder screenshot showing calibration refusal. Whether that probe
coincided with source silence remains unresolved. The transport/calibration
must pass the preflight above before using that attempt to bisect app builds.
The earlier normalized player and raw afplay runs are different experimental
conditions and must not be pooled.

Private original regression filenames under `~/.codescribe/sessions/` on Dragon:

- `agent-channel-3-749e7099-1dd6-4557-a612-a1679473f7e4.wav` — about 25 s.
- `agent-channel-0-c05faaa9-294e-4413-a01c-340a8460ba33.wav` — about 42 s.
- `agent-channel-3-6ec55f2c-7117-4555-abac-2b878ac24ea6.wav` — about 146 s.

Historical native trial artifacts live under
`~/.vibecrafted/artifacts/vetcoders/codescribe/2026_1013/release-smoke-941/`.
Its `run.py` is historical evidence, not an accepted reusable runner: it lacks
the validated loopback gate above. Private audio and transcripts stay outside Git.
