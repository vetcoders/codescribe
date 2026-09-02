#!/usr/bin/env bash
# ============================================================================
# mic-bridge.sh — laptop microphone → dragon BlackHole, over Tailscale
# ============================================================================
# The Founder works on dragon over VNC. Dragon has no microphone; its only
# input device is "BlackHole 2ch" (virtual). This bridge carries the laptop
# microphone as raw PCM over UDP and plays it INTO BlackHole on dragon, so
# Codescribe.app (or any app) reading "BlackHole 2ch" hears the laptop mic at
# real time through CoreAudio — the same path a physical mic takes.
#
#   laptop:  ffmpeg avfoundation mic ──s16le/48k/mono──UDP──▶ dragon
#   dragon:  python udp ──▶ sox coreaudio "BlackHole 2ch" (0.2 s buffer) ──▶ BH input
#
# Usage
#   dragon:  scripts/mic-bridge.sh receive            # keeps running (sox → BlackHole)
#   laptop:  scripts/mic-bridge.sh send [dragon-ip]   # keeps running
#   either:  scripts/mic-bridge.sh stop
#   dragon:  scripts/mic-bridge.sh level              # 3 s RMS on BlackHole input
#   laptop:  scripts/mic-bridge.sh tone [dragon-ip]   # 4 s 440 Hz test signal
#
# TCC: the laptop sender must run from a GUI-launched process (Terminal.app).
# An ffmpeg started over ssh captures zeroed samples (-inf dB) and no error —
# measured 2026-09-02. `scripts/mic-bridge.command` is the double-click /
# `open -a Terminal` wrapper for that.
#
# Env
#   MIC_BRIDGE_PORT      default 5004
#   MIC_BRIDGE_MIC       default "MacBook Pro Microphone" (avfoundation name)
#   MIC_BRIDGE_OUT_NAME  default "BlackHole 2ch" (resolved to an audiotoolbox
#                        index by CoreAudio device order)
#   MIC_BRIDGE_RATE      default 48000
set -euo pipefail

PORT="${MIC_BRIDGE_PORT:-5004}"
MIC="${MIC_BRIDGE_MIC:-MacBook Pro Microphone}"
OUT_NAME="${MIC_BRIDGE_OUT_NAME:-BlackHole 2ch}"
RATE="${MIC_BRIDGE_RATE:-48000}"
DRAGON_DEFAULT="100.82.232.70"
PIDFILE="${TMPDIR:-/tmp}/mic-bridge.pid"

need() { command -v "$1" >/dev/null 2>&1 || { echo "mic-bridge: missing $1" >&2; exit 2; }; }

# CoreAudio device order == ffmpeg audiotoolbox -audio_device_index order.
out_index() {
  need swift
  swift - "$OUT_NAME" <<'EOF'
import CoreAudio
import Foundation
let want = CommandLine.arguments[1].lowercased()
var addr = AudioObjectPropertyAddress(
  mSelector: kAudioHardwarePropertyDevices,
  mScope: kAudioObjectPropertyScopeGlobal,
  mElement: kAudioObjectPropertyElementMain)
var size: UInt32 = 0
AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size)
var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids)
for (i, id) in ids.enumerated() {
  var nameAddr = AudioObjectPropertyAddress(
    mSelector: kAudioObjectPropertyName,
    mScope: kAudioObjectPropertyScopeGlobal,
    mElement: kAudioObjectPropertyElementMain)
  var nsize = UInt32(MemoryLayout<Unmanaged<CFString>>.size)
  var ref: Unmanaged<CFString>? = nil
  let status = withUnsafeMutablePointer(to: &ref) { ptr in
    AudioObjectGetPropertyData(id, &nameAddr, 0, nil, &nsize, ptr)
  }
  guard status == noErr, let name = ref?.takeRetainedValue() as String? else { continue }
  if name.lowercased() == want { print(i); exit(0) }
}
FileHandle.standardError.write(Data("mic-bridge: output device not found: \(want)\n".utf8))
exit(2)
EOF
}

# Pin the capture device's nominal sample rate so avfoundation's timestamps
# match what the hardware delivers.
pin_input_rate() {
  need swift
  swift - "$MIC" "$RATE" <<'SWIFT' 2>/dev/null || echo "mic-bridge: rate pin failed (continuing)" >&2
import CoreAudio
import Foundation
let want = CommandLine.arguments[1].lowercased()
let target = Float64(CommandLine.arguments[2]) ?? 48000
var addr = AudioObjectPropertyAddress(
  mSelector: kAudioHardwarePropertyDevices,
  mScope: kAudioObjectPropertyScopeGlobal,
  mElement: kAudioObjectPropertyElementMain)
var size: UInt32 = 0
AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size)
var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids)
for id in ids {
  var nameAddr = AudioObjectPropertyAddress(
    mSelector: kAudioObjectPropertyName,
    mScope: kAudioObjectPropertyScopeGlobal,
    mElement: kAudioObjectPropertyElementMain)
  var nsize = UInt32(MemoryLayout<Unmanaged<CFString>>.size)
  var ref: Unmanaged<CFString>? = nil
  _ = withUnsafeMutablePointer(to: &ref) { AudioObjectGetPropertyData(id, &nameAddr, 0, nil, &nsize, $0) }
  guard let name = ref?.takeRetainedValue() as String?, name.lowercased() == want else { continue }
  var rateAddr = AudioObjectPropertyAddress(
    mSelector: kAudioDevicePropertyNominalSampleRate,
    mScope: kAudioObjectPropertyScopeGlobal,
    mElement: kAudioObjectPropertyElementMain)
  var rate: Float64 = 0
  var rsize = UInt32(MemoryLayout<Float64>.size)
  AudioObjectGetPropertyData(id, &rateAddr, 0, nil, &rsize, &rate)
  if rate == target { print("mic-bridge: \(name) nominal rate already \(Int(target))"); exit(0) }
  var nr = target
  let st = AudioObjectSetPropertyData(id, &rateAddr, 0, nil, rsize, &nr)
  print("mic-bridge: \(name) nominal rate \(Int(rate)) -> \(Int(target)) status=\(st)")
  exit(0)
}
FileHandle.standardError.write(Data("mic-bridge: input device not found: \(want)\n".utf8))
exit(2)
SWIFT
}

stop_bridge() {
  if [[ -f "$PIDFILE" ]]; then
    kill "$(cat "$PIDFILE")" 2>/dev/null || true
    rm -f "$PIDFILE"
  fi
  pkill -f "mic-bridge-ffmpeg" 2>/dev/null || true
}

case "${1:-}" in
  receive-ffmpeg)
    # Kept for reference: audiotoolbox goes silent after the first underrun
    # on a jittery UDP stream. `receive` (sox) is the default.
    need ffmpeg
    IDX="$(out_index)"
    stop_bridge
    echo "mic-bridge: receiving udp://0.0.0.0:${PORT} → audiotoolbox[$IDX] \"$OUT_NAME\"" >&2
    # overrun_nonfatal: a burst never kills the receiver; fifo is ~0.5 s.
    exec -a mic-bridge-ffmpeg ffmpeg -nostdin -hide_banner -loglevel warning \
      -f s16le -ar "$RATE" -ac 1 \
      -i "udp://0.0.0.0:${PORT}?fifo_size=98304&overrun_nonfatal=1" \
      -ac 2 -f audiotoolbox -audio_device_index "$IDX" -
    ;;
  receive|receive-sox)
    # Alternative sink: sox plays to the CoreAudio device by NAME with an
    # explicit buffer, tolerant of UDP jitter where ffmpeg's audiotoolbox
    # AudioQueue goes silent after its first underrun (measured 2026-09-02:
    # paced tone audible at -21 dB, live mic -inf on the same receiver).
    need sox
    stop_bridge
    echo "mic-bridge: receiving udp://0.0.0.0:${PORT} → sox coreaudio \"$OUT_NAME\"" >&2
    exec -a mic-bridge-ffmpeg python3 - "$PORT" <<'PY' | AUDIODEV="$OUT_NAME" sox -q --buffer 19200 -t raw -r "$RATE" -e signed -b 16 -c 1 - -t coreaudio
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("0.0.0.0", int(sys.argv[1])))
out = sys.stdout.buffer
while True:
    d, _ = s.recvfrom(4096)
    out.write(d); out.flush()
PY
    ;;
  send)
    need ffmpeg
    DEST="${2:-$DRAGON_DEFAULT}"
    stop_bridge
    # avfoundation trusts the device's nominal rate. Measured 2026-09-02: the
    # MacBook Pro Microphone sat at 88200 Hz nominal while delivering half
    # that, so ffmpeg timestamped at 88.2k and the bridge ran at 0.49x with
    # BlackHole reading digital silence. Pin the nominal rate to $RATE first.
    pin_input_rate
    echo "mic-bridge: sending \"$MIC\" → udp://${DEST}:${PORT} (${RATE} Hz mono s16le)" >&2
    # pkt_size 960 bytes = 10 ms at 48 kHz mono s16le.
    exec -a mic-bridge-ffmpeg ffmpeg -nostdin -hide_banner -loglevel warning \
      -f avfoundation -i ":${MIC}" \
      -ac 1 -ar "$RATE" -f s16le "udp://${DEST}:${PORT}?pkt_size=960"
    ;;
  tone)
    need ffmpeg
    DEST="${2:-$DRAGON_DEFAULT}"
    echo "mic-bridge: 4 s 440 Hz → udp://${DEST}:${PORT}" >&2
    exec ffmpeg -nostdin -hide_banner -loglevel warning -re \
      -f lavfi -i "sine=frequency=440:sample_rate=${RATE}:duration=4" \
      -ac 1 -ar "$RATE" -f s16le "udp://${DEST}:${PORT}?pkt_size=960"
    ;;
  level)
    need ffmpeg
    ffmpeg -hide_banner -nostats -f avfoundation -i ":${OUT_NAME}" -t 3 \
      -af "astats=measure_overall=RMS_level+Peak_level:measure_perchannel=none" \
      -f null - 2>&1 | grep -E "RMS level|Peak level"
    ;;
  stop)
    stop_bridge
    echo "mic-bridge: stopped" >&2
    ;;
  *)
    sed -n '2,30p' "$0"
    exit 1
    ;;
esac
