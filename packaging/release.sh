#!/bin/zsh
# release.sh — build wrapper .app, build DMG, optional codesign + notarization
#
# Usage examples:
#   packaging/release.sh                                  # build only
#   SIGN_IDENTITY="Developer ID Application: ..." packaging/release.sh
#   NOTARY_PROFILE=VSNotary packaging/release.sh          # uses stored profile
#   DMG_UNQUARANTINE=1 packaging/release.sh               # clears quarantine (local)

set -euo pipefail

NO_NOTARY=0
for arg in "$@"; do
  if [[ "$arg" == "--no-notary" ]]; then
    NO_NOTARY=1
  fi
done

ROOT_DIR="$(cd -- "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/packaging/dist"

echo "🏗️  Building wrapper app…"
"$ROOT_DIR/packaging/appwrap/build_wrapper_app.sh"

echo "📦 Building DMG…"
"$ROOT_DIR/packaging/dmg/build_dmg.sh"

DMG_PATH=$(ls -1t "$ROOT_DIR/packaging/dmg"/VistaScribe-*.dmg | head -n1)
APP_PATH="$ROOT_DIR/packaging/dist/VistaScribe.app"
echo "➡️  App: $APP_PATH"
echo "➡️  DMG: $DMG_PATH"

if [[ -n "${SIGN_IDENTITY:-}" ]] || [[ -n "${NOTARY_PROFILE:-}" ]]; then
  echo "✍️  Codesign + (optional) Notary…"
  ARGS=(--app "$APP_PATH" --dmg "$DMG_PATH")
  [[ -n "${SIGN_IDENTITY:-}" ]] && ARGS+=(--identity "$SIGN_IDENTITY")
  [[ -n "${NOTARY_PROFILE:-}" ]] && ARGS+=(--profile "$NOTARY_PROFILE")
  [[ "$NO_NOTARY" -eq 1 ]] && ARGS+=(--no-notary)
  "$ROOT_DIR/packaging/scripts/notary_quick.sh" "${ARGS[@]}"
else
  echo "(skip) Signing/Notary — no SIGN_IDENTITY/NOTARY_PROFILE provided"
fi

echo "✅ Release ready: $DMG_PATH"
