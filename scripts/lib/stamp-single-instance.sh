#!/usr/bin/env bash
# Install-lane Info.plist stamp: LSMultipleInstancesProhibited = true.
#
# The installed app refuses a second generation (cut L, cba560c1), but the flag
# must not live in macos/project.yml: the app target is also the TEST_HOST of
# CodescribeTests, so every DerivedData test host carried it with the installed
# app's bundle id. With a flagged installed build running (build 1547,
# 2026-09-30 17:14) LaunchServices refused to launch the test host and
# `make test-swift` died before its first test ("Could not launch
# CodescribeTests … The LaunchServices launcher has returned an error").
#
# scripts/build-app.sh runs this only when CODESCRIBE_INSTALL_LANE=1 (set by
# `make install-app` and scripts/build-dmg.sh), after the bundle is complete
# and BEFORE its codesign stage: an Info.plist edit after signing breaks the
# seal. Idempotent; it never touches any other key.
set -euo pipefail

APP="${1:?usage: stamp-single-instance.sh <App.app>}"
PLIST="$APP/Contents/Info.plist"
PLISTBUDDY="${PLISTBUDDY:-/usr/libexec/PlistBuddy}"

if [[ ! -f "$PLIST" ]]; then
  echo "stamp-single-instance: no Info.plist at $PLIST" >&2
  exit 2
fi

# Delete-then-add pins the value type to bool even when an older build wrote
# the key as a string; a missing key makes Delete a no-op.
"$PLISTBUDDY" -c 'Delete :LSMultipleInstancesProhibited' "$PLIST" >/dev/null 2>&1 || true
"$PLISTBUDDY" -c 'Add :LSMultipleInstancesProhibited bool true' "$PLIST"

if [[ "$("$PLISTBUDDY" -c 'Print :LSMultipleInstancesProhibited' "$PLIST")" != "true" ]]; then
  echo "stamp-single-instance: LSMultipleInstancesProhibited did not read back true in $PLIST" >&2
  exit 1
fi
echo "    single-instance: LSMultipleInstancesProhibited=true stamped into $PLIST"
