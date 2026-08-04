#!/usr/bin/env bash
#
# Generate a signed Sparkle 2 appcast for Codescribe DMG artifacts.
#
# Thin wrapper around Sparkle's `generate_appcast` (the prebuilt tool that ships
# inside the SPM checkout — no extra install step). Signs every enclosure with
# the EdDSA private key taken from the environment, never from a file in the
# repo.
#
# Usage:
#   SPARKLE_ED_PRIVATE_KEY=<base64 ed25519 private key> \
#     scripts/generate-appcast.sh <dmg-dir> [-o <appcast.xml>] \
#       [--download-url-prefix <url>]
#
#   <dmg-dir>               directory containing the release DMG(s)
#   -o <path>               output appcast path (default: <dmg-dir>/appcast.xml)
#   --download-url-prefix   URL prefix for enclosure links, e.g.
#                           https://github.com/vetcoders/codescribe/releases/download/v0.14.0/
#
# Env:
#   SPARKLE_ED_PRIVATE_KEY  REQUIRED. EdDSA private key (the base64 line printed
#                           by Sparkle's `generate_keys -x`). Fed to the tool on
#                           stdin — it never touches the filesystem.
#   SPARKLE_BIN             optional dir holding generate_appcast; otherwise the
#                           SPM artifact under macos/build is auto-discovered.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() { sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2; }

DMG_DIR=""
OUT=""
URL_PREFIX=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    --download-url-prefix) URL_PREFIX="$2"; shift 2 ;;
    -h|--help) usage ;;
    *)
      [ -z "$DMG_DIR" ] || { echo "error: unexpected argument: $1" >&2; usage; }
      DMG_DIR="$1"; shift ;;
  esac
done

[ -n "$DMG_DIR" ] || usage
[ -d "$DMG_DIR" ] || { echo "error: not a directory: $DMG_DIR" >&2; exit 1; }
[ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ] || {
  echo "error: SPARKLE_ED_PRIVATE_KEY is empty — refusing to emit an unsigned appcast" >&2
  exit 1
}

# ── Locate generate_appcast: explicit SPARKLE_BIN, then the SPM artifact the
# app build materializes under macos/build (see scripts/build-app.sh).
GENERATE_APPCAST=""
if [ -n "${SPARKLE_BIN:-}" ] && [ -x "$SPARKLE_BIN/generate_appcast" ]; then
  GENERATE_APPCAST="$SPARKLE_BIN/generate_appcast"
else
  GENERATE_APPCAST="$(
    find "$REPO_ROOT/macos/build/SourcePackages/artifacts" \
      -type f -name generate_appcast -perm +111 2>/dev/null | head -1 || true
  )"
fi
[ -n "$GENERATE_APPCAST" ] || {
  echo "error: generate_appcast not found. Build the app once (scripts/build-app.sh)" >&2
  echo "       so SPM materializes the Sparkle artifact, or set SPARKLE_BIN." >&2
  exit 1
}

OUT="${OUT:-$DMG_DIR/appcast.xml}"

ARGS=(--ed-key-file - -o "$OUT")
[ -n "$URL_PREFIX" ] && ARGS+=(--download-url-prefix "$URL_PREFIX")

echo "==> generate_appcast: $GENERATE_APPCAST"
echo "==> signing enclosures in $DMG_DIR -> $OUT"
printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$GENERATE_APPCAST" "${ARGS[@]}" "$DMG_DIR"
echo "==> appcast written: $OUT"
