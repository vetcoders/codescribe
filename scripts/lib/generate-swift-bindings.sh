#!/usr/bin/env bash
# Generate and normalize UniFFI outputs before publishing byte changes to Bridge.
# Identical Swift/C/modulemap files retain their existing mtimes for Xcode reuse
# and l10n-sync freshness validation. Called by scripts/build-app.sh stage 3.
set -euo pipefail

BINDGEN="${1:?usage: generate-swift-bindings.sh <bindgen> <library> <bridge-dir>}"
DYLIB="${2:?missing library path}"
BINDINGS_DIR="${3:?missing Bridge directory}"

mkdir -p "$BINDINGS_DIR"
# Same-filesystem staging makes each changed-file replacement an atomic rename.
# mktemp owns this unique directory; cleanup never removes destination files.
BINDINGS_STAGE="$(mktemp -d "$BINDINGS_DIR/.uniffi.XXXXXX")"
trap 'rm -r -- "$BINDINGS_STAGE"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

"$BINDGEN" generate --library "$DYLIB" --language swift --out-dir "$BINDINGS_STAGE"
# Retain the existing Swift/C normalization, entirely inside the stage.
find "$BINDINGS_STAGE" \( -name '*.swift' -o -name '*.h' \) \
  -exec sed -i '' -E 's/[[:space:]]+$//' {} +
python3 - "$BINDINGS_STAGE" "$BINDINGS_DIR" <<'PY'
import sys
from pathlib import Path

stage, destination = map(Path, sys.argv[1:])
generated = sorted(stage.iterdir())
if not generated or any(p.is_symlink() or not p.is_file() for p in generated):
    raise SystemExit("error: UniFFI stage must contain regular generated files")

# Collapse blank EOF lines to one POSIX newline, exactly as build-app did.
# Modulemaps and any other generated files keep their original bytes.
for source in generated:
    if source.suffix in (".swift", ".h"):
        data = source.read_bytes()
        if data:
            source.write_bytes(data.rstrip(b"\n") + b"\n")

# Finish normalization and all byte comparisons before changing any live file.
changed = []
for source in generated:
    target = destination / source.name
    if not target.is_file() or target.read_bytes() != source.read_bytes():
        changed.append((source, target))
for source, target in changed:
    source.replace(target)
PY
