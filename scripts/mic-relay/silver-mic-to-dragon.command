#!/usr/bin/env bash
# ============================================================================
# silver-mic-to-dragon.command — laptop-side sender of a live mic stream
# ============================================================================
# Captures the LOCAL default microphone and streams raw PCM (s16le, 48 kHz,
# mono) over ssh to the remote host, where receive-to-blackhole.sh plays it
# into the "BlackHole 2ch" virtual device. Codescribe on the remote machine
# then dictates from the laptop's microphone as if it were local.
#
# Run this ON THE LAPTOP (double-click in Finder, or `open` it): the .command
# extension launches it inside Terminal.app, which is what holds the
# microphone TCC grant. First run pops the macOS microphone permission
# dialog for Terminal — approve it once.
#
# Env:
#   DRAGON_HOST     ssh destination            (default "dragon")
#   RELAY_SCRIPT    receiver path on the host  (default: this repo's copy)
#   MIC_DEVICE      avfoundation input spec    (default ":default" = system mic)
#
# Bandwidth: ~94 KiB/s (48 kHz * 16 bit * mono). Works fine over Tailscale.
# Stop with Ctrl-C — both ends shut down with the pipe.
# ============================================================================
set -euo pipefail

DRAGON_HOST="${DRAGON_HOST:-dragon}"
RELAY_SCRIPT="${RELAY_SCRIPT:-/Volumes/vc-workspace/vetcoders/codescribe/scripts/mic-relay/receive-to-blackhole.sh}"
MIC_DEVICE="${MIC_DEVICE:-:default}"

command -v ffmpeg >/dev/null || { echo "ffmpeg missing on this laptop (brew install ffmpeg)" >&2; exit 3; }

echo "streaming local mic (${MIC_DEVICE}) -> ${DRAGON_HOST} -> BlackHole 2ch"
echo "leave this window open while dictating; Ctrl-C to stop"

ffmpeg -hide_banner -loglevel warning \
  -f avfoundation -i "${MIC_DEVICE}" \
  -ar 48000 -ac 1 -f s16le - |
  ssh "${DRAGON_HOST}" "bash '${RELAY_SCRIPT}'"
