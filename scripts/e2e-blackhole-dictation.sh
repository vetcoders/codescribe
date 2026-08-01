#!/usr/bin/env bash
# ============================================================================
# e2e-blackhole-dictation.sh — dictation e2e through REAL CoreAudio
# ============================================================================
# SEPARATION LAW (operator 2026-07-27, non-negotiable):
#
#   DAILY product path  ≠  ENGINE harness path
#
#   Daily (Codescribe.app, teacher, mic, speakers):
#     System Sound Input  = real mic (MacBook Pro Microphone / …)
#     System Sound Output = real speakers / headphones
#     Never leave BlackHole as a system default.
#
#   Harness (this script / make test-engine-apple ENGINE_ALL_CLIPS=1):
#     Plays INTO BlackHole by device *name* (audio-play-to-device.swift)
#     Captures FROM BlackHole by device *name* / resolved avfoundation index
#     Does NOT require BlackHole to be the system default I/O
#     ALWAYS saves system defaults at start and restores them on EXIT
#
# Every existing STT e2e feeds a decoded WAV straight into the transcription
# session over an mpsc channel (see collect_buffered_engine_events). That proves
# the decoder and the pipeline — and skips the entire capture path. This harness
# closes that gap with BlackHole loopback at real time through CoreAudio.
#
#   scripts/e2e-blackhole-dictation.sh [fixture.wav]
#
# Requires: BlackHole 2ch installed (brew install --cask blackhole-2ch) and
# microphone permission for the *terminal* running this script.
#
# Optional env:
#   BLACKHOLE_DEVICE          default "BlackHole 2ch"
#   DAILY_INPUT_DEVICE        preferred restore if saved default was BH
#   DAILY_OUTPUT_DEVICE       preferred restore if saved default was BH
#   FORCE_LEAVE_BH_DEFAULTS=1 skip forced un-hijack of BH-as-default (debug only)
#
# Exit: 0 pass · 1 mismatch/empty · 2 preconditions missing.

set -uo pipefail
cd "$(dirname "$0")/.."

DEVICE="${BLACKHOLE_DEVICE:-BlackHole 2ch}"
DAILY_INPUT_DEVICE="${DAILY_INPUT_DEVICE:-MacBook Pro Microphone}"
DAILY_OUTPUT_DEVICE="${DAILY_OUTPUT_DEVICE:-MacBook Pro Speakers}"
# The Rust test carrying the capture assertions. Does not exist yet — it will be
# written once the loopback is verified to carry signal end to end; until then
# the run fails loudly rather than reporting a vacuous pass.
CAPTURE_TEST="${CAPTURE_TEST:-e2e_device_capture_dictation}"
FIXTURE="${1:-tests/assets/data_assets/02_kubernetes-wymaga-konfiguracji.wav}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/cs-bh-e2e.XXXXXX")"
DEFAULTS_FILE="$WORK/audio-defaults.env"

restore_daily_audio() {
  # Always attempt restore — even mid-fail — so a harness crash cannot leave
  # the operator's daily mic/speakers stuck on BlackHole.
  if [ -x ./scripts/audio-default-devices.swift ] || [ -f ./scripts/audio-default-devices.swift ]; then
    if [ -f "$DEFAULTS_FILE" ]; then
      ./scripts/audio-default-devices.swift restore "$DEFAULTS_FILE" 2>/dev/null || true
    fi
    # If defaults were already BH when we started (or restore failed), force
    # the daily mic/speakers so Codescribe is usable after the test.
    if [ "${FORCE_LEAVE_BH_DEFAULTS:-0}" != "1" ]; then
      local cur_in cur_out
      cur_in="$(./scripts/audio-default-devices.swift get-input 2>/dev/null || true)"
      cur_out="$(./scripts/audio-default-devices.swift get-output 2>/dev/null || true)"
      case "$cur_in" in
        *BlackHole*|*blackhole*)
          ./scripts/audio-default-devices.swift set-input "$DAILY_INPUT_DEVICE" 2>/dev/null || true
          ;;
      esac
      case "$cur_out" in
        *BlackHole*|*blackhole*)
          ./scripts/audio-default-devices.swift set-output "$DAILY_OUTPUT_DEVICE" 2>/dev/null || true
          ;;
      esac
    fi
  fi
  rm -rf "$WORK"
}

trap 'restore_daily_audio' EXIT

fail() { printf '\033[31mFAIL\033[0m %s\n' "$1" >&2; exit "${2:-1}"; }
info() { printf '\033[36m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[33mWARN\033[0m %s\n' "$1" >&2; }

# ── Preconditions ───────────────────────────────────────────────────────────
[ -f "$FIXTURE" ] || fail "fixture not found: $FIXTURE" 2
command -v swift >/dev/null 2>&1 || fail "swift not on PATH" 2
[ -f ./scripts/audio-default-devices.swift ] ||
  fail "missing scripts/audio-default-devices.swift (daily/harness separation helper)" 2

# Snapshot system defaults FIRST — restore runs on every EXIT.
./scripts/audio-default-devices.swift save "$DEFAULTS_FILE" ||
  fail "cannot snapshot system default input/output" 2
info "daily audio snapshot: $(tr '\n' ' ' <"$DEFAULTS_FILE")"

# If the operator already hijacked daily I/O onto BlackHole (Sound panel),
# say so loudly — harness will still run, then force daily devices back.
CUR_IN="$(./scripts/audio-default-devices.swift get-input 2>/dev/null || true)"
CUR_OUT="$(./scripts/audio-default-devices.swift get-output 2>/dev/null || true)"
case "$CUR_IN$CUR_OUT" in
  *BlackHole*|*blackhole*)
    warn "system default I/O is on BlackHole (input='$CUR_IN' output='$CUR_OUT')."
    warn "That is the HARNESS device, not the daily product path."
    warn "After this run we restore to: input='$DAILY_INPUT_DEVICE' output='$DAILY_OUTPUT_DEVICE' (override via env)."
    ;;
esac

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

# ── Loopback device mixer preflight ─────────────────────────────────────────
# Pressing the system mute key while BlackHole is the default output writes a
# PERSISTENT device-level mute into the driver — on BOTH scopes (measured
# 2026-07-25: output AND input mute stuck at 1, loopback at -91 dB while the
# player verifiably rendered peak 0.106). AppleScript cannot clear it — only a
# direct kAudioDevicePropertyMute write can — so this preflight repairs the
# device instead of telling the operator to click things. Volumes are pushed
# to 1.0 too (a 50% slider costs ~15 dB per side).
#
# NOTE: this only mutes/volume on the BlackHole *device* — it does NOT change
# which device is the system default. Daily mic/speakers stay put.
swift scripts/audio-device-unmute.swift "$DEVICE" ||
  fail "cannot clear device mute/volume on '$DEVICE' — the loopback would carry silence" 2

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
#
# NEVER hardcode avfoundation index `:0` — device order is machine-local and
# changes with Zoom/NoMachine/iPhone mics. On div0 (2026-07-27) `:0` was
# ZoomAudioDevice while BlackHole sat at `:2`, so the preflight "passed"
# mute-clear then captured pure silence (-91 dB) forever. Prefer the device
# *name* (`none:BlackHole 2ch`); fall back to a parsed index if the name form
# is rejected by an older ffmpeg.
AV_IN="none:${DEVICE}"
if ! ffmpeg -hide_banner -f avfoundation -list_devices true -i "" 2>&1 |
  grep -F "AVFoundation audio devices:" >/dev/null; then
  :
fi
# Resolve numeric index as fallback (and for diagnostic messages).
AV_IDX="$(
  ffmpeg -hide_banner -f avfoundation -list_devices true -i "" 2>&1 |
    awk -v want="$DEVICE" '
      /AVFoundation audio devices:/ { audio=1; next }
      audio && /AVFoundation video devices:/ { exit }
      audio && match($0, /\[[0-9]+\]/) {
        idx = substr($0, RSTART+1, RLENGTH-2)
        # line looks like: [AVFoundation indev @ 0x…] [2] BlackHole 2ch
        name = $0
        sub(/^.*\[[0-9]+\][[:space:]]*/, "", name)
        if (name == want) { print idx; exit }
      }
    '
)"
if [ -z "${AV_IDX}" ]; then
  fail "cannot resolve avfoundation audio index for '$DEVICE' (is it listed under Audio devices?)" 2
fi
info "avfoundation input: name='$DEVICE' index=$AV_IDX (using -i none:NAME, not :0)"

timeout -k 3 20 ffmpeg -y -hide_banner -loglevel error -f avfoundation \
  -i "$AV_IN" -t 6 -ar 16000 -ac 1 "$WORK/loopback.wav" >"$WORK/rec.log" 2>&1 &
RECORDER=$!
sleep 1
./scripts/audio-play-to-device.swift "$DEVICE" "$FIXTURE" >/dev/null 2>&1 &
PLAYER=$!
wait $RECORDER 2>/dev/null
RECORDER_STATUS=$?
# If name form failed to open, retry once with the resolved index.
if [ $RECORDER_STATUS -ne 0 ] || [ ! -s "$WORK/loopback.wav" ]; then
  info "name capture failed (rc=$RECORDER_STATUS) — retrying -i :$AV_IDX"
  kill $PLAYER 2>/dev/null || true
  timeout -k 3 20 ffmpeg -y -hide_banner -loglevel error -f avfoundation \
    -i ":$AV_IDX" -t 6 -ar 16000 -ac 1 "$WORK/loopback.wav" >"$WORK/rec.log" 2>&1 &
  RECORDER=$!
  sleep 1
  ./scripts/audio-play-to-device.swift "$DEVICE" "$FIXTURE" >/dev/null 2>&1 &
  PLAYER=$!
  wait $RECORDER 2>/dev/null
  RECORDER_STATUS=$?
fi
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
  fail "no audio captured from '$DEVICE' (avfoundation index was $AV_IDX, not :0)" 2
fi

MAXVOL="$(ffmpeg -hide_banner -i "$WORK/loopback.wav" -af volumedetect -f null - 2>&1 |
  awk -F': ' '/max_volume/ {print $2}')"
case "$MAXVOL" in
  "-91"*|"-inf"*|"")
    cat >&2 <<HINT
Loopback still silent after targeting '$DEVICE' (avfoundation index $AV_IDX).
Daily mic/speakers are NOT involved — do not set Sound defaults to BlackHole.

Checklist (harness-only):
  1. Terminal has Microphone TCC (Privacy → Microphone → your terminal).
  2. Player log should show "rendered peak … into 'BlackHole 2ch'" — if not,
     audio-play-to-device failed.
  3. First BlackHole install: reboot once if capture stays digital silence.
  4. Unmute already ran on the BH device itself (not system mute).
  5. For engine proof WITHOUT BlackHole use the channel path:
       make test-engine-apple-channel
     (no loopback device; decoder+pipeline only).
HINT
    fail "loopback captured silence (max_volume=${MAXVOL:-none}) on '$DEVICE' (index $AV_IDX)" 2
    ;;
esac
info "loopback carries signal (max_volume=$MAXVOL, device='$DEVICE' index=$AV_IDX)"
info "system defaults left alone for daily use; will restore snapshot on exit"

# ── 2. Real dictation run through the capture path ──────────────────────────
# AUDIO_INPUT_DEVICE is the same knob the app uses at runtime (recorder.rs:360),
# so this exercises production device selection, not a test-only shortcut.
#
# The TEST owns the player lifecycle (it arms capture first, then spawns the
# player), so nothing here may play audio — a second player would double-feed
# the loopback. This layer only prepares the engine and runs cargo.
info "recording ~${DURATION}s through the app's capture path"
export AUDIO_INPUT_DEVICE="$DEVICE"
export CODESCRIBE_E2E_STT=1
export CODESCRIBE_E2E_AUDIO="$FIXTURE"
export CODESCRIBE_E2E_CAPTURE_VIA_DEVICE=1

# Engine: Apple live is the product case (same setup as make test-engine-apple).
# ENGINE=candle skips the bridge and lets the default engine run.
if [ "${ENGINE:-apple}" = "apple" ]; then
  BRIDGE="target/release/codescribe-stt-bridge"
  make "$BRIDGE" >/dev/null 2>&1 || fail "cannot build Apple STT bridge" 2
  export CODESCRIBE_STT_ENGINE=apple
  export CODESCRIBE_APPLE_STT_BRIDGE="$PWD/$BRIDGE"
  # Outside Codescribe.app the terminal is TCC's responsible process and the
  # bridge's own Speech grant is invisible. Disclaim makes the bridge
  # self-responsible (see respawnSelfResponsibleIfRequested + make engine-auth).
  export CODESCRIBE_BRIDGE_DISCLAIM=1
fi

# Pre-build so compile time cannot eat into anything timing-sensitive.
cargo test --test e2e_overlay_delivery_parity --no-run >"$WORK/build.log" 2>&1 ||
  { tail -20 "$WORK/build.log" >&2; fail "test build failed" 2; }

# --ignored: the capture tests are `#[ignore]` so a plain `cargo test` cannot
# report them as passing without a loopback (review P1-01).
cargo test --test e2e_overlay_delivery_parity \
  "$CAPTURE_TEST" -- --ignored --nocapture >"$WORK/test.log" 2>&1
STATUS=$?

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
