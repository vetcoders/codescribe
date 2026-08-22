#!/usr/bin/env bash
# ============================================================================
# smoke-macos27.sh — per-beta host smoke for the macOS surfaces Codescribe owns
# ============================================================================
# WHY
# ---
# Pensieve lost days on macOS 27 because a beta host IS the shipping host and
# nobody had a repeatable way to ask "did AppKit move under us?". Codescribe
# dodged that specific bullet (zero NSToolbar surface) but caught the same
# generation's other edge: AppKit 27 fires notifications from inside window
# operations, which froze the thread rail until d79781b1.
#
# This script is the standing answer. Run it after every OS/Xcode bump, before
# every release, and whenever a hotkey/overlay/permission report smells like
# drift. It proves what can be proven without a human at the keyboard, and it
# SKIPs the rest honestly instead of pretending.
#
# WHAT IT WILL NEVER DO
# ---------------------
# Post synthetic events, raise a TCC dialog, read pasteboard content, or touch
# operator state. Rows that need any of those are operator-gated by design —
# see the install cadence and safety boundaries in AGENTS.md.
#
# USAGE
#   scripts/smoke-macos27.sh [--out FILE] [--with-inference] [--clipboard-content]
#
#   --out FILE            also write a markdown checklist table to FILE
#   --with-inference      run the Metal/candle cold-vs-warm row (needs models +
#                         fixtures; minutes, not seconds)
#   --clipboard-content   perform ONE pasteboard content read so an operator
#                         watching the screen can report whether macOS 27 shows
#                         a read notice (S-6 canary)
#
# EXIT: 0 when no row FAILed · 1 when at least one row FAILed.
#       SKIP never fails the run — an un-runnable row is not a broken one.
#
# Created by Vetcoders (c)2026

set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

OUT_FILE=""
WITH_INFERENCE=0
CLIPBOARD_CONTENT=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT_FILE="${2:?--out needs a path}"; shift 2 ;;
    --with-inference) WITH_INFERENCE=1; shift ;;
    --clipboard-content) CLIPBOARD_CONTENT=1; shift ;;
    -h|--help) sed -n '1,40p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# ---------------------------------------------------------------------------
# Row bookkeeping
# ---------------------------------------------------------------------------

ROWS=()
FAILED=0

record() {
  local row="$1" status="$2" evidence="$3"
  if [[ "$status" == "FAIL" ]]; then FAILED=1; fi
  ROWS+=("${row}|${status}|${evidence}")
  printf '%-24s %-5s %s\n' "$row" "$status" "$evidence"
}

# Swift probes emit `row<TAB>status<TAB>evidence`; funnel them through record.
record_probe_lines() {
  while IFS=$'\t' read -r row status evidence; do
    [[ -z "${row:-}" ]] && continue
    record "$row" "$status" "$evidence"
  done
}

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

# The bridge is pinned to this target (Makefile) so the probes are built with
# the same language/runtime semantics the product ships with.
SWIFT_TARGET="arm64-apple-macos26.0"

echo "== Codescribe · macOS host smoke =="
printf '%-24s %-5s %s\n' "ROW" "STATE" "EVIDENCE"
echo "--------------------------------------------------------------------------"

# ---------------------------------------------------------------------------
# Host identity — every later row is only meaningful against this
# ---------------------------------------------------------------------------

OS_VER="$(sw_vers -productVersion)"
OS_BUILD="$(sw_vers -buildVersion)"
XCODE_VER="$(xcodebuild -version 2>/dev/null | tr '\n' ' ' | sed 's/  */ /g' || echo 'xcodebuild unavailable')"
SDK_VER="$(xcrun --sdk macosx --show-sdk-version 2>/dev/null || echo 'unknown')"
GIT_HEAD="$(git rev-parse --short HEAD 2>/dev/null || echo 'no-git')"
GIT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo 'no-git')"
record "host-os" "INFO" "macOS ${OS_VER} (${OS_BUILD}) · ${XCODE_VER}· SDK ${SDK_VER}"
record "host-repo" "INFO" "${GIT_BRANCH}@${GIT_HEAD}"

APP_BUNDLE=""
for candidate in "/Applications/Codescribe.app" "$HOME/Applications/Codescribe.app"; do
  [[ -d "$candidate" ]] && { APP_BUNDLE="$candidate"; break; }
done
if [[ -n "$APP_BUNDLE" ]]; then
  APP_SHORT="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || echo '?')"
  APP_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || echo '?')"
  APP_RUNNING="$(pgrep -f "$APP_BUNDLE/Contents/MacOS/Codescribe" >/dev/null 2>&1 && echo 'running' || echo 'not running')"
  record "host-app" "INFO" "${APP_BUNDLE} ${APP_SHORT} (${APP_BUILD}), ${APP_RUNNING}"
else
  record "host-app" "SKIP" "no installed Codescribe.app found; UI-dependent rows cannot be judged against a build id"
fi

# ---------------------------------------------------------------------------
# Permissionless API-drift probe (CoreGraphics table, TCC matrix, tap re-arm,
# disclaim symbol, pasteboard)
# ---------------------------------------------------------------------------

PROBE_BIN="$TMP_DIR/macos27_probe"
if swiftc -parse-as-library -target "$SWIFT_TARGET" -O \
    scripts/smoke/macos27_probe.swift -o "$PROBE_BIN" 2>"$TMP_DIR/probe_build.log"; then
  PROBE_ARGS=()
  [[ "$CLIPBOARD_CONTENT" == "1" ]] && PROBE_ARGS+=(--clipboard-content)
  # The probe exits non-zero on its own FAIL rows; record() already owns the
  # verdict, so do not let set -e swallow the output.
  "$PROBE_BIN" "${PROBE_ARGS[@]}" > "$TMP_DIR/probe.tsv" 2>"$TMP_DIR/probe.err" || true
  record_probe_lines < "$TMP_DIR/probe.tsv"
else
  record "macos27-probe" "FAIL" "probe failed to compile: $(tail -3 "$TMP_DIR/probe_build.log" | tr '\n' ' ')"
fi

# ---------------------------------------------------------------------------
# NSPanel overlay placement — compiled against the production source, no
# XCTest host (the host boots AppModel and hangs while the app is running)
# ---------------------------------------------------------------------------

OVERLAY_BIN="$TMP_DIR/overlay_probe"
if swiftc -target "$SWIFT_TARGET" -O \
    macos/Codescribe/Screens/Overlay/OverlayPlacement.swift \
    scripts/smoke/overlay_placement_probe.swift \
    -o "$OVERLAY_BIN" 2>"$TMP_DIR/overlay_build.log"; then
  "$OVERLAY_BIN" > "$TMP_DIR/overlay.tsv" 2>&1 || true
  record_probe_lines < "$TMP_DIR/overlay.tsv"
else
  record "overlay-placement" "FAIL" "probe failed to compile against OverlayPlacement.swift: $(tail -3 "$TMP_DIR/overlay_build.log" | tr '\n' ' ')"
fi

# ---------------------------------------------------------------------------
# AppKit observer census — the executable form of the C2 review rule
# ---------------------------------------------------------------------------

ALLOW_FILE="scripts/smoke/appkit-observers.allow"
if [[ ! -f "$ALLOW_FILE" ]]; then
  record "appkit-observer-census" "FAIL" "missing pin file $ALLOW_FILE"
else
  # Live census: AppKit notifications are Apple-namespaced (NSWindow., NSApp.,
  # NSScrollView., ...); our own buses are ConfigChangeBus./ThreadsChangeBus.,
  # so the NS-prefix is a clean discriminator. Tests are out of scope: they do
  # not run inside a user's window server session.
  grep -rn --include='*.swift' 'forName:[[:space:]]*NS[A-Za-z]*\.' macos/Codescribe \
    | sed -E 's/^([^:]+):[0-9]+:.*forName:[[:space:]]*(NS[A-Za-z]+\.[A-Za-z]+).*/\1 | \2/' \
    | sort -u > "$TMP_DIR/observers.live" || true

  sed -E 's/[[:space:]]*#.*$//' "$ALLOW_FILE" \
    | grep -v '^[[:space:]]*$' \
    | awk -F'|' 'NF>=2 { gsub(/^[ \t]+|[ \t]+$/, "", $1); gsub(/^[ \t]+|[ \t]+$/, "", $2); print $1 " | " $2 }' \
    | sort -u > "$TMP_DIR/observers.pinned" || true

  # Anything registered ObjC-style bypasses the forName scan entirely. `grep`
  # exits 1 on "no matches", which is the GOOD outcome here — swallow it so
  # pipefail does not read a clean repo as a broken run.
  SELECTOR_SITES="$( { grep -rn --include='*.swift' 'addObserver([[:space:]]*[A-Za-z]*,[[:space:]]*selector:' macos/Codescribe || true; } | wc -l | tr -d ' ')"

  if ! DIFF="$(diff "$TMP_DIR/observers.pinned" "$TMP_DIR/observers.live")"; then
    record "appkit-observer-census" "FAIL" \
      "live AppKit observers diverge from the pin — review each against $ALLOW_FILE, then update it: $(echo "$DIFF" | tr '\n' ' ' | cut -c1-320)"
  elif [[ "$SELECTOR_SITES" != "0" ]]; then
    record "appkit-observer-census" "FAIL" \
      "${SELECTOR_SITES} selector-style addObserver site(s) bypass the forName census; pin them or migrate to the block form"
  else
    record "appkit-observer-census" "PASS" \
      "$(wc -l < "$TMP_DIR/observers.live" | tr -d ' ') AppKit-notification observer(s) match the pinned census; 0 selector-style sites"
  fi
fi

# ---------------------------------------------------------------------------
# CGEventTap disable-sentinel unit contract (pure logic, no permissions)
# ---------------------------------------------------------------------------

if cargo test -p codescribe --quiet is_tap_disabled_event_detects_disabled_sentinels \
    > "$TMP_DIR/tap_unit.log" 2>&1; then
  record "eventtap-sentinel-unit" "PASS" \
    "is_tap_disabled_event_detects_disabled_sentinels green (app/os/hotkeys/platform.rs)"
else
  record "eventtap-sentinel-unit" "FAIL" \
    "$(tail -5 "$TMP_DIR/tap_unit.log" | tr '\n' ' ' | cut -c1-300)"
fi

# ---------------------------------------------------------------------------
# Sparkle
# ---------------------------------------------------------------------------

if [[ -z "$APP_BUNDLE" ]]; then
  record "sparkle" "SKIP" "no installed app bundle to inspect"
else
  SPARKLE_LINKED="$( { otool -L "$APP_BUNDLE/Contents/MacOS/Codescribe" 2>/dev/null || true; } | { grep -c 'Sparkle' || true; } )"
  FEED="$(/usr/libexec/PlistBuddy -c 'Print :SUFeedURL' "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || echo '')"
  EDKEY="$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || echo '')"
  if [[ "$SPARKLE_LINKED" == "0" ]]; then
    record "sparkle" "FAIL" "Sparkle.framework is not linked into the shipped binary"
  elif [[ -z "$FEED" ]]; then
    record "sparkle" "FAIL" "SUFeedURL is empty — the updater has nowhere to look"
  elif [[ -z "$EDKEY" ]]; then
    # A local build-app.sh build has no SPARKLE_ED_PUBLIC_KEY injected. That is
    # fail-closed by design (UpdaterService refuses), and the release payload
    # gate lives in scripts/verify-dmg-payload.sh — so this is a note about
    # THIS bundle, not a release defect.
    record "sparkle" "INFO" \
      "framework linked, feed ${FEED}, SUPublicEDKey EMPTY → updater fail-closed. Expected for a local build; release payloads are gated by scripts/verify-dmg-payload.sh"
  else
    record "sparkle" "PASS" "framework linked, feed ${FEED}, SUPublicEDKey present"
  fi

  if [[ -n "$FEED" ]]; then
    FEED_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "$FEED" || echo '000')"
    if [[ "$FEED_CODE" == "200" ]]; then
      record "sparkle-feed" "PASS" "appcast reachable over TLS (HTTP ${FEED_CODE})"
    else
      record "sparkle-feed" "FAIL" "appcast fetch returned HTTP ${FEED_CODE}"
    fi
  fi
fi

# ---------------------------------------------------------------------------
# Metal / candle cold vs warm
# ---------------------------------------------------------------------------

if [[ "$WITH_INFERENCE" != "1" ]]; then
  record "metal-cold-warm" "SKIP" \
    "needs --with-inference (release build + model load; minutes). An OS upgrade invalidates the Metal shader cache, so the first run after a bump pays a one-off compile — support noise, not a regression"
else
  FIXTURE=""
  if FIXTURE="$(scripts/lib/data-assets.sh resolve 01_no-to-dobra.wav 2>/dev/null)"; then :; else FIXTURE=""; fi
  if [[ -z "$FIXTURE" ]]; then
    record "metal-cold-warm" "SKIP" "fixture 01_no-to-dobra.wav not resolvable through the three-tier order"
  # Two deliberate pins, both borrowed from how the repo already builds locally:
  #  * `--profile local-release` + `CODESCRIBE_LOCAL_INSTALL=1` — the exact
  #    pairing scripts/build-app.sh:100 uses: release optimizations with the
  #    checked-in development license verifier. The literal `release` profile
  #    fails closed without the operator-owned licence key, which is a
  #    distribution contract, not something a latency probe should satisfy.
  #  * `CODESCRIBE_STT_ENGINE=candle` — ~/.codescribe/.env is loaded by every
  #    process that boots core, so without pinning, an operator's power-user
  #    setting would silently decide which lane this row measures.
  elif env -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX \
        CODESCRIBE_LOCAL_INSTALL=1 CODESCRIBE_STT_ENGINE=candle \
        cargo run --profile local-release --quiet \
        --example final_pass_latency_baseline -- \
        "$FIXTURE" "$FIXTURE" > "$TMP_DIR/latency.log" 2>"$TMP_DIR/latency.err"; then
    SUMMARY="$(grep -E '^[0-9]+ \|' "$TMP_DIR/latency.log" | tr '\n' ' ' | cut -c1-300)"
    record "metal-cold-warm" "PASS" "cold→warm on $(basename "$FIXTURE"): ${SUMMARY}"
  else
    record "metal-cold-warm" "FAIL" "$(tail -5 "$TMP_DIR/latency.err" | tr '\n' ' ' | cut -c1-300)"
  fi
fi

# ---------------------------------------------------------------------------
# The review rule has to be written down where agents actually read it
# ---------------------------------------------------------------------------

if grep -q 'AppKit notification observers' AGENTS.md 2>/dev/null; then
  record "review-rule-recorded" "PASS" "AGENTS.md carries the AppKit-observer coalescing rule"
else
  record "review-rule-recorded" "FAIL" \
    "AGENTS.md does not state the AppKit-observer coalescing rule; the pin in $ALLOW_FILE has no doctrine behind it"
fi

# ---------------------------------------------------------------------------
# Operator-gated rows. These need eyes on a screen and hands on a keyboard.
# They are SKIP, never silent.
# ---------------------------------------------------------------------------

record "overlay-space-visual" "SKIP" \
  "operator row: show the overlay on a normal Space and inside a fullscreen Space, confirm it stays on top and fully on-screen. Geometry is proven headless by overlay-placement; this row is the compositor half"
record "popover-key-storm" "SKIP" \
  "operator row: open the tray popover with the Agent window open, close it repeatedly, confirm no beachball (the d79781b1 freeze). Needs a live window server session"
record "eventtap-forced-timeout" "SKIP" \
  "operator row: with the app running, stall the tap (heavy UI storm) until macOS posts kCGEventTapDisabledByTimeout, then confirm hotkeys keep working without a restart. eventtap-rearm proves the API half; this row proves it end to end under a real storm"
record "sparkle-canary" "SKIP" \
  "operator row: signed N-1 → N update check against the live appcast. Requires a release-signed build with SUPublicEDKey injected"

# ---------------------------------------------------------------------------
# Summary + optional markdown artifact
# ---------------------------------------------------------------------------

echo "--------------------------------------------------------------------------"
pass_n=0; fail_n=0; skip_n=0; info_n=0
for entry in "${ROWS[@]}"; do
  case "${entry#*|}" in
    PASS*) pass_n=$((pass_n + 1)) ;;
    FAIL*) fail_n=$((fail_n + 1)) ;;
    SKIP*) skip_n=$((skip_n + 1)) ;;
    INFO*) info_n=$((info_n + 1)) ;;
  esac
done
echo "PASS ${pass_n} · FAIL ${fail_n} · SKIP ${skip_n} · INFO ${info_n}"

if [[ -n "$OUT_FILE" ]]; then
  mkdir -p "$(dirname "$OUT_FILE")"
  {
    echo "| Row | State | Evidence |"
    echo "|---|---|---|"
    for entry in "${ROWS[@]}"; do
      row="${entry%%|*}"
      rest="${entry#*|}"
      status="${rest%%|*}"
      evidence="${rest#*|}"
      printf '| `%s` | %s | %s |\n' "$row" "$status" "${evidence//|/\\|}"
    done
    echo
    echo "_host macOS ${OS_VER} (${OS_BUILD}) · ${GIT_BRANCH}@${GIT_HEAD} · PASS ${pass_n} / FAIL ${fail_n} / SKIP ${skip_n} / INFO ${info_n}_"
  } > "$OUT_FILE"
  echo "checklist table written to ${OUT_FILE}"
fi

exit "$FAILED"
