#!/usr/bin/env bash
# Install-lane single-instance stamp (scripts/lib/stamp-single-instance.sh).
#
# Regression: cba560c1 put LSMultipleInstancesProhibited in macos/project.yml,
# so the CodescribeTests TEST_HOST (same target, same bundle id) carried it and
# LaunchServices refused to launch it while the flagged installed app ran.
# This proves, on a fixture bundle and without touching /Applications:
#   - project.yml no longer carries the flag;
#   - the stamp sets it as a bool, leaves CFBundleIdentifier and every other
#     key untouched, and is idempotent (also over a string-typed stale key);
#   - build-app.sh stamps before its codesign stage and verifies the seal after,
#     only under CODESCRIBE_INSTALL_LANE=1, which install-app and build-dmg set;
#   - a stamp after signing breaks the seal (why the order is load-bearing).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAMP="$ROOT/scripts/lib/stamp-single-instance.sh"
PB=/usr/libexec/PlistBuddy
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

fail() { echo "single-instance-stamp: FAIL: $*" >&2; exit 1; }

make_fixture() {
  local app="$1"
  mkdir -p "$app/Contents/MacOS"
  cp /usr/bin/true "$app/Contents/MacOS/Fixture"
  chmod 755 "$app/Contents/MacOS/Fixture"
  cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Fixture</string>
  <key>CFBundleIdentifier</key><string>com.vetcoders.codescribe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1547</string>
  <key>CSBuildCommit</key><string>e66caf1a</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
PLIST
}

plist_json() { plutil -convert json -o - "$1/Contents/Info.plist"; }

# 1. The test host source no longer carries the flag.
if python3 - "$ROOT/macos/project.yml" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
sys.exit(0 if re.search(r"^\s*LSMultipleInstancesProhibited\s*:", text, re.M) else 1)
PY
then
  fail "macos/project.yml sets LSMultipleInstancesProhibited; the XCTest host inherits it"
fi

# 2. Stamp sets a bool true and changes nothing else.
APP="$WORKDIR/Fixture.app"
make_fixture "$APP"
before="$(plist_json "$APP")"
"$STAMP" "$APP" >/dev/null
[[ "$("$PB" -c 'Print :LSMultipleInstancesProhibited' "$APP/Contents/Info.plist")" == "true" ]] \
  || fail "flag not set to true"
[[ "$("$PB" -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")" == "com.vetcoders.codescribe" ]] \
  || fail "CFBundleIdentifier changed"
after="$(plist_json "$APP")"
python3 - "$before" "$after" <<'PY' || fail "stamp touched keys other than LSMultipleInstancesProhibited"
import json, sys
before, after = json.loads(sys.argv[1]), json.loads(sys.argv[2])
assert after.pop("LSMultipleInstancesProhibited") is True
assert before == after, (before, after)
PY

# 3. Idempotent: a second run is byte-for-byte a no-op on the plist semantics.
"$STAMP" "$APP" >/dev/null
[[ "$(plist_json "$APP")" == "$after" ]] || fail "second stamp changed the plist"

# 4. A stale string-typed key is replaced by a real bool.
"$PB" -c 'Delete :LSMultipleInstancesProhibited' "$APP/Contents/Info.plist"
"$PB" -c 'Add :LSMultipleInstancesProhibited string NO' "$APP/Contents/Info.plist"
"$STAMP" "$APP" >/dev/null
python3 - "$(plist_json "$APP")" <<'PY' || fail "stale string key was not replaced by bool true"
import json, sys
assert json.loads(sys.argv[1])["LSMultipleInstancesProhibited"] is True
PY

# 5. Missing Info.plist fails closed.
mkdir -p "$WORKDIR/Empty.app/Contents"
if "$STAMP" "$WORKDIR/Empty.app" >/dev/null 2>&1; then
  fail "stamp succeeded without an Info.plist"
fi

# 6. Order is load-bearing: stamp-then-sign verifies strictly;
#    sign-then-stamp breaks the seal.
SIGNED="$WORKDIR/Signed.app"
make_fixture "$SIGNED"
"$STAMP" "$SIGNED" >/dev/null
codesign --force --deep --sign - --identifier com.vetcoders.codescribe "$SIGNED" 2>/dev/null
codesign --verify --deep --strict "$SIGNED" 2>/dev/null || fail "stamp-then-sign did not verify"
LATE="$WORKDIR/Late.app"
make_fixture "$LATE"
codesign --force --deep --sign - --identifier com.vetcoders.codescribe "$LATE" 2>/dev/null
"$STAMP" "$LATE" >/dev/null
if codesign --verify --deep --strict "$LATE" 2>/dev/null; then
  fail "a post-signature plist edit still verified; the order check proves nothing"
fi

# 7. build-app.sh: stamp gated on the lane, before stage 7, seal verified after;
#    the install and DMG lanes set the lane knob.
python3 - "$ROOT" <<'PY' || fail "build-app.sh / install lanes do not wire the stamp before codesign"
import re, sys
from pathlib import Path
root = Path(sys.argv[1])
build = (root / "scripts/build-app.sh").read_text(encoding="utf-8")
stamp = build.index('"$REPO_ROOT/scripts/lib/stamp-single-instance.sh" "$APP"')
gate = build.rindex('if [ "$INSTALL_LANE" = "1" ]; then', 0, stamp)
assert 'INSTALL_LANE="${CODESCRIBE_INSTALL_LANE:-0}"' in build[:gate]
first_sign = build.index('codesign --force --deep --sign')
assert stamp < first_sign, "stamp must precede the stage-7 codesign"
verify = build.index('codesign --verify --deep --strict "$APP"')
assert verify > build.rindex('codesign --force --deep --sign'), "seal check must follow signing"
makefile = (root / "Makefile").read_text(encoding="utf-8")
recipe = makefile[makefile.index("\ninstall-app:"):]
recipe = recipe[: recipe.index("\n\n")]
assert "CODESCRIBE_INSTALL_LANE=1" in recipe
assert re.search(r"codesign --verify --deep --strict /Applications/", recipe)
dmg = (root / "scripts/build-dmg.sh").read_text(encoding="utf-8")
assert dmg.index("BUILD_ENV+=(CODESCRIBE_INSTALL_LANE=1)") < dmg.index("make app PROFILE=release")
PY

echo "single-instance-stamp: ok"
