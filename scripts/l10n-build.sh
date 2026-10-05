#!/usr/bin/env bash
# Compile the existing app target for localization, without a Rust build,
# executable link, signing, installation or tests. Contract: docs/LOCALIZATION.md.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# A separate DerivedData tree: the archive is extraction evidence, not an app.
# Export the same L10N_DERIVED for subsequent l10n-sync / verify-l10n-sync calls.
DERIVED="${L10N_DERIVED:-macos/build/l10n}"
if [ "${L10N_CONFIG:-Debug}" != Debug ]; then
  echo "l10n-build: Debug is required to include #if DEBUG copy." >&2
  exit 2
fi
python3 - "$DERIVED" <<'PY'
from pathlib import Path
import sys

if Path(sys.argv[1]).resolve() == Path("macos/build").resolve():
    raise SystemExit("l10n-build: use separate DerivedData; refusing to replace the normal app build with an archive.")
PY

( cd macos && xcodegen generate )
xcodebuild -quiet build \
  -project macos/Codescribe.xcodeproj -scheme Codescribe -configuration Debug \
  -derivedDataPath "$DERIVED" -destination 'generic/platform=macOS' \
  ARCHS="$(uname -m)" ONLY_ACTIVE_ARCH=YES \
  SWIFT_EMIT_LOC_STRINGS=YES \
  CODE_SIGNING_ALLOWED=NO \
  MACH_O_TYPE=staticlib OTHER_LDFLAGS= ENABLE_DEBUG_DYLIB=NO

echo "l10n-build: Debug compiler extraction ready under $DERIVED."
echo "l10n-build: use L10N_DERIVED=\"$DERIVED\" make verify-l10n-sync (or make l10n-sync to update)."
