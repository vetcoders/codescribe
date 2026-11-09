#!/bin/bash
# Serves live window captures for AgentContextEvidenceTests.
#
# Usage: scripts/agent-window-capture.sh <live-dir>
#
# The test writes "<windowNumber> <name>" to <live-dir>/request and waits for
# <live-dir>/<name>. This loop answers each new request with
# `screencapture -l`, which records the compositor (Liquid Glass, vibrancy,
# Dark-mode glass) that an offscreen bitmap cannot. Run it from a process that
# holds Screen Recording permission, with the screen unlocked; stop it with
# Ctrl-C when the test finishes. screencapture refuses names that start with a
# dot, so it writes tmp_<name> and renames it once the file is complete.
set -u
dir="${1:?usage: $0 <live-dir>}"
mkdir -p "$dir"
last=""
while true; do
  if [ -f "$dir/request" ]; then
    req="$(cat "$dir/request")"
    if [ -n "$req" ] && [ "$req" != "$last" ]; then
      read -r window name <<<"$req"
      screencapture -x -o -t png -l "$window" "$dir/tmp_$name" 2>>"$dir/capturer.err" \
        && mv "$dir/tmp_$name" "$dir/$name"
      last="$req"
    fi
  fi
  sleep 0.2
done
