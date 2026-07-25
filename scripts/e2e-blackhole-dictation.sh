#!/usr/bin/env bash
# ============================================================================
# e2e-blackhole-dictation.sh — dictation e2e through REAL CoreAudio
# ============================================================================
# Every existing STT e2e feeds a decoded WAV straight into the transcription
# session over an mpsc channel (see collect_buffered_engine_events). That proves
# the decoder and the pipeline — and skips the entire capture path: device
# selection, CoreAudio, cpal callbacks, resampling, the streaming ring buffer.
# Which is exactly where the July field failures lived (417s of speech → 234
# characters).
#
# This harness closes that gap using BlackHole as a loopback: a fixture is
# played INTO the virtual device's output, the app records FROM its input, so
# audio travels the same road a microphone takes — at real time, through
# CoreAudio, subject to the same buffer caps.
#
#   scripts/e2e-blackhole-dictation.sh [fixture.wav]
#
# Requires: BlackHole 2ch installed (brew install --cask blackhole-2ch) and
# microphone permission for whatever runs this script. Both are checked up
# front, because a silent recording that "passes" would be the worst outcome.
#
# Exit: 0 pass · 1 mismatch/empty · 2 preconditions missing.

set -uo pipefail
cd "$(dirname "$0")/.."

DEVICE="${BLACKHOLE_DEVICE:-BlackHole 2ch}"
# The Rust test carrying the capture assertions. Does not exist yet — it will be
# written once the loopback is verified to carry signal end to end; until then
# the run fails loudly rather than reporting a vacuous pass.
CAPTURE_TEST="${CAPTURE_TEST:-e2e_device_capture_dictation}"
FIXTURE="${1:-tests/assets/data_assets/02_kubernetes-wymaga-konfiguracji.wav}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cs-bh-e2e.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

fail() { printf '\033[31mFAIL\033[0m %s\n' "$1" >&2; exit "${2:-1}"; }
info() { printf '\033[36m==>\033[0m %s\n' "$1"; }

# ── Preconditions ───────────────────────────────────────────────────────────
[ -f "$FIXTURE" ] || fail "fixture not found: $FIXTURE" 2
command -v swift >/dev/null 2>&1 || fail "swift not on PATH" 2

if ! ./scripts/audio-play-to-device.swift --list x 2>/dev/null | grep -qxF "$DEVICE"; then
  fail "output device '$DEVICE' not found. Install with: brew install --cask blackhole-2ch" 2
fi

# The device must be able to BOTH play and capture — a half-configured
# aggregate device silently records nothing.
# `-list_devices` always exits non-zero (it has no input to open), so capture
# its output first rather than letting pipefail read that as "device missing".
AV_DEVICES="$(ffmpeg -hide_banner -f avfoundation -list_devices true -i "" 2>&1 || true)"
if ! printf '%s' "$AV_DEVICES" | grep -q "$DEVICE"; then
  fail "'$DEVICE' is not visible as an INPUT device to ffmpeg/avfoundation" 2
fi

REFERENCE="${FIXTURE%.wav}_human_transcription.txt"
[ -f "$REFERENCE" ] || info "no human transcription beside the fixture — running as smoke only"

DURATION="$(afinfo "$FIXTURE" 2>/dev/null |
  awk -F': ' '/estimated duration/ {printf "%d", $2 + 3}')"
[ -n "$DURATION" ] || fail "cannot read fixture duration" 2

# ── 1. Loopback sanity: does audio actually come back? ───────────────────────
# Run BEFORE the app so a dead loopback is reported as a setup problem rather
# than as an STT failure. Six seconds is enough to see non-silence.
info "loopback check on '$DEVICE'"
# HARD timeout: without microphone permission CoreAudio neither errors nor
# returns — the capture just blocks forever. A hang is the single most confusing
# failure here, so it is converted into a named, actionable one.
# -k: a capture blocked inside CoreAudio ignores SIGTERM, so escalate to KILL.
timeout -k 3 20 ffmpeg -y -hide_banner -loglevel error -f avfoundation -i ":0" \
  -t 6 -ar 16000 -ac 1 "$WORK/loopback.wav" >"$WORK/rec.log" 2>&1 &
RECORDER=$!
sleep 1
./scripts/audio-play-to-device.swift "$DEVICE" "$FIXTURE" >/dev/null 2>&1 &
PLAYER=$!
wait $RECORDER 2>/dev/null
RECORDER_STATUS=$?
kill $PLAYER 2>/dev/null

if [ $RECORDER_STATUS -eq 124 ] || [ ! -s "$WORK/loopback.wav" ]; then
  cat >&2 <<'PERMISSION'
Microphone access is missing for the process running this script.

CoreAudio blocks (rather than failing) when the TCC prompt cannot be shown, so
this looks like a hang. Grant it once:

  System Settings → Privacy & Security → Microphone → enable your terminal
  (Terminal / iTerm / the IDE running this), then re-run.

Only the operator can approve that dialog; no flag or env var substitutes.
PERMISSION
  fail "no audio captured from '$DEVICE'" 2
fi

MAXVOL="$(ffmpeg -hide_banner -i "$WORK/loopback.wav" -af volumedetect -f null - 2>&1 |
  awk -F': ' '/max_volume/ {print $2}')"
case "$MAXVOL" in
  "-91"*|"-inf"*|"") fail "loopback captured silence (max_volume=${MAXVOL:-none}). Is '$DEVICE' selected as OUTPUT for the player and INPUT for capture?" 2 ;;
esac
info "loopback carries signal (max_volume=$MAXVOL)"

# ── 2. Real dictation run through the capture path ──────────────────────────
# AUDIO_INPUT_DEVICE is the same knob the app uses at runtime (recorder.rs:360),
# so this exercises production device selection, not a test-only shortcut.
info "recording ${DURATION}s through the app's capture path"
export AUDIO_INPUT_DEVICE="$DEVICE"
export CODESCRIBE_E2E_STT=1
export CODESCRIBE_E2E_AUDIO="$FIXTURE"
export CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1

./scripts/audio-play-to-device.swift "$DEVICE" "$FIXTURE" >"$WORK/play.log" 2>&1 &
PLAYER=$!

cargo test --test e2e_overlay_delivery_parity \
  "$CAPTURE_TEST" -- --nocapture >"$WORK/test.log" 2>&1
STATUS=$?
kill $PLAYER 2>/dev/null

if [ $STATUS -ne 0 ]; then
  tail -40 "$WORK/test.log" >&2
  fail "dictation test failed (exit $STATUS); full log: $WORK/test.log"
fi

# `cargo test <name>` exits 0 when the filter matches NOTHING ("0 passed").
# Trusting the exit code alone would report a green run that executed no
# assertions at all — the exact failure this harness exists to catch.
PASSED="$(awk '/^test result:/ {for (i = 1; i <= NF; i++) if ($i ~ /^passed/) print $(i - 1)}' \
  "$WORK/test.log" | head -1)"
if [ "${PASSED:-0}" -lt 1 ]; then
  fail "no test matched '$CAPTURE_TEST' — the capture assertions do not exist yet, so this run proved nothing. Write that test (or pass CAPTURE_TEST=<name>) before trusting a pass."
fi

info "PASS — capture path transcribed the fixture ($PASSED test(s))"
grep -E "^(transcript|chars|coverage)" "$WORK/test.log" 2>/dev/null || true
