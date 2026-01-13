#!/bin/zsh
# Quick Action backend client for Finder (Q2 variant)
# Usage: Called by an Automator "Run Shell Script" action with input as arguments.
# For each selected audio file, it sends to the local backend /stt_and_format,
# saves a sibling .transkrypcja.txt, copies to clipboard, and shows a notification.

set -euo pipefail

BACKEND_URL=${BACKEND_URL:-"http://127.0.0.1:8237"}
LANG=${WHISPER_LANG:-"pl"}

for f in "$@"; do
  if [ ! -f "$f" ]; then
    continue
  fi
  dir=$(dirname "$f")
  base=$(basename "$f")
  stem="${base%.*}"
  echo "Transcribing via backend: $f"
  # Send file to /stt_and_format
  # Note: instruction is optional; leave empty for default SYSTEM_PROMPT
  resp=$(curl -sS -f -X POST \
    -F "audio=@$f" \
    -F "instruction=" \
    "$BACKEND_URL/stt_and_format")
  # Extract text (simple jq-less approach)
  text=$(python3 -c 'import sys,json; print(json.load(sys.stdin).get("text",""))' <<< "$resp")
  outpath="$dir/${stem}.transkrypcja.txt"
  print -r -- "$text" > "$outpath"
  # Copy to clipboard
  if command -v pbcopy >/dev/null 2>&1; then
    print -r -- "$text" | pbcopy
  fi
  # Notification
  if command -v osascript >/dev/null 2>&1; then
    osascript -e 'display notification "Transkrypcja gotowa i w schowku" with title "CodeScribe" subtitle '"$stem"''
  fi
  echo "OK -> $outpath"
done
