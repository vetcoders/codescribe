#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/x9bus-attachment-regression.XXXXXX")
trap 'rm -rf "$TEMP_DIR"' EXIT HUP INT TERM

swiftc -swift-version 6 -parse-as-library \
  "$ROOT/macos/Codescribe/Screens/Overlay/ConversationClipboardAttachment.swift" \
  "$ROOT/scripts/tests/overlay-attachment-runtime-regression.swift" \
  -o "$TEMP_DIR/overlay-attachment-runtime-regression"
"$TEMP_DIR/overlay-attachment-runtime-regression"
python3 "$ROOT/scripts/tests/overlay-attachment-regression-contract.py"
