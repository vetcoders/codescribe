#!/usr/bin/env bash
# ============================================================================
# receive-to-blackhole.sh — dragon-side receiver of a live PCM mic stream
# ============================================================================
# Reads raw PCM (s16le, 48 kHz, mono) from stdin and plays it into the
# BlackHole virtual output device. Whatever plays INTO BlackHole appears on
# the "BlackHole 2ch" INPUT — so any app capturing from that device (e.g.
# Codescribe with input device set to "BlackHole 2ch") hears the remote mic
# live, as if it were plugged into this machine.
#
# Meant to be invoked over ssh by silver-mic-to-dragon.command; can also be
# fed by anything that emits s16le/48k/mono PCM on stdout.
#
# The audiotoolbox device index is resolved by NAME at startup because
# CoreAudio indices shift whenever devices come and go.
#
# Env:
#   BLACKHOLE_DEVICE   device name to target (default "BlackHole 2ch")
#
# This plays into a named device only — the system default output is never
# touched (harness restore-defaults rule; incident 2026-07-26: BlackHole
# left as the system input after a restart silenced daily dictation).
# ============================================================================
set -euo pipefail

BLACKHOLE_DEVICE="${BLACKHOLE_DEVICE:-BlackHole 2ch}"

command -v ffmpeg >/dev/null || { echo "ffmpeg missing (brew install ffmpeg)" >&2; exit 3; }

# Resolve the audiotoolbox output index for the named device.
device_index="$(
  ffmpeg -hide_banner -f lavfi -i anullsrc=r=48000:cl=mono -t 0.05 \
         -f audiotoolbox -list_devices true - 2>&1 |
  sed -n "s/^\[AudioToolbox[^]]*\] \[\([0-9][0-9]*\)\][[:space:]]*${BLACKHOLE_DEVICE},.*/\1/p" |
  head -1
)"

if [[ -z "${device_index}" ]]; then
  echo "device '${BLACKHOLE_DEVICE}' not found in audiotoolbox outputs" >&2
  echo "install: brew install --cask blackhole-2ch" >&2
  exit 4
fi

echo "playing stdin PCM into '${BLACKHOLE_DEVICE}' (audiotoolbox index ${device_index})" >&2

exec ffmpeg -hide_banner -loglevel warning \
  -fflags nobuffer -flags low_delay -probesize 32 \
  -f s16le -ar 48000 -ac 1 -i - \
  -ac 2 -f audiotoolbox -audio_device_index "${device_index}" -
