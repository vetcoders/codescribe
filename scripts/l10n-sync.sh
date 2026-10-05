#!/usr/bin/env bash
# l10n-sync.sh - fold compiler-extracted strings into Localizable.xcstrings
#
# The Xcode project is generated and every build runs from the command line, so
# nothing keeps the String Catalog in step with the Swift sources: Xcode's
# "sync on build" is an IDE feature. What a CLI build does leave behind is one
# .stringsdata file per Swift source (SWIFT_EMIT_LOC_STRINGS in
# macos/project.yml). This script merges those into the catalog with
# `xcstringstool sync` — the same tool the IDE drives.
#
# It is authoritative only for the build it reads. Extraction data describes
# the last compile of each file, not the working tree, so the script refuses to
# run when a Swift source is newer than its last compile or was never compiled:
# a sync from an old build would drop keys that still exist.
#
# Usage:
#   scripts/l10n-sync.sh           # update the catalog in place
#   scripts/l10n-sync.sh --check   # exit 1 if the catalog differs; writes nothing
#
# Env:
#   L10N_DERIVED   derived-data root of the build to read (default: macos/build)
#   L10N_CONFIG    Xcode configuration to read (default: Debug — a superset of
#                  Release, because it also compiles `#if DEBUG` code)
#
# Exit codes:
#   0 - catalog is in step with the build (or was updated)
#   1 - --check found drift
#   2 - no usable extraction data (no build, or the build is older than sources)
#
# Contract: docs/LOCALIZATION.md §2.
#
# Created by Vetcoders (c)2026

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CHECK=0
case "${1:-}" in
  "") ;;
  --check) CHECK=1 ;;
  *)
    echo "usage: scripts/l10n-sync.sh [--check]" >&2
    exit 2
    ;;
esac

SOURCE_ROOT="macos/Codescribe"
CATALOG="$SOURCE_ROOT/Resources/Localization/Localizable.xcstrings"
DERIVED="${L10N_DERIVED:-macos/build}"
CONFIG="${L10N_CONFIG:-Debug}"
OBJECTS="$DERIVED/Build/Intermediates.noindex/Codescribe.build/$CONFIG/Codescribe.build/Objects-normal"

if [ ! -f "$CATALOG" ]; then
  echo "l10n-sync: catalog not found: $CATALOG" >&2
  exit 2
fi
if ! xcrun --find xcstringstool >/dev/null 2>&1; then
  echo "l10n-sync: xcstringstool is required (ships with Xcode 15 or newer)." >&2
  exit 2
fi
if [ ! -d "$OBJECTS" ]; then
  echo "l10n-sync: no $CONFIG build under $DERIVED — run 'make app' (or 'make test-swift') first." >&2
  exit 2
fi

WORK="$(mktemp -d)"
trap 'rm -r "$WORK"' EXIT

# Select the extraction files that describe the current sources, and prove they
# are current. Each .stringsdata names the source it was compiled from.
if ! python3 - "$REPO_ROOT" "$SOURCE_ROOT" "$OBJECTS" "$WORK/inputs" <<'PY'; then
import json
import os
import sys
from pathlib import Path

repo, source_root, objects, out = sys.argv[1:5]
repo = Path(repo).resolve()
sources = {p.resolve() for p in (repo / source_root).rglob("*.swift")}



def compiled_at(data):
    # The compiler leaves a .stringsdata file untouched when a recompile
    # extracts the same strings, so its own mtime can predate the source. The
    # object file beside it is rewritten by every compile of that source.
    stamps = [data.stat().st_mtime]
    for suffix in (".o", ".swiftdeps"):
        sibling = data.with_suffix(suffix)
        if sibling.exists():
            stamps.append(sibling.stat().st_mtime)
    return max(stamps)


extracted = {}
for data in Path(objects).rglob("*.stringsdata"):
    try:
        source = Path(json.loads(data.read_text())["source"]).resolve()
    except (OSError, ValueError, KeyError):
        continue
    # Intermediates can outlive a deleted or renamed source; those rows no
    # longer describe anything in the tree.
    if source not in sources:
        continue
    previous = extracted.get(source)
    if previous is None or compiled_at(data) > compiled_at(previous):
        extracted[source] = data

missing = sorted(str(p.relative_to(repo)) for p in sources - extracted.keys())
outdated = sorted(
    str(source.relative_to(repo))
    for source, data in extracted.items()
    if source.stat().st_mtime > compiled_at(data)
)
if missing or outdated:
    for label, paths in (("never compiled", missing), ("edited since the build", outdated)):
        if paths:
            print(f"l10n-sync: {len(paths)} source(s) {label}:", file=sys.stderr)
            for path in paths[:10]:
                print(f"  {path}", file=sys.stderr)
            if len(paths) > 10:
                print(f"  … and {len(paths) - 10} more", file=sys.stderr)
    print("l10n-sync: the extraction data is older than the sources — rebuild first.", file=sys.stderr)
    raise SystemExit(1)

with open(out, "wb") as handle:
    for data in sorted(extracted.values()):
        handle.write(os.fsencode(str(data)) + b"\0")
print(f"l10n-sync: {len(extracted)} extraction files read from the {Path(objects).parent.parent.name} build")
PY
  exit 2
fi

# xcstringstool derives the table name from the file name, so the working copy
# must keep it.
mkdir "$WORK/catalog"
cp "$CATALOG" "$WORK/catalog/Localizable.xcstrings"
xargs -0 xcrun xcstringstool sync "$WORK/catalog/Localizable.xcstrings" --stringsdata <"$WORK/inputs"

# Summarise by key, so a drift report reads as copy changes rather than JSON.
python3 - "$CATALOG" "$WORK/catalog/Localizable.xcstrings" <<'PY'
import json
import sys

before = json.load(open(sys.argv[1]))["strings"]
after = json.load(open(sys.argv[2]))["strings"]
added = sorted(after.keys() - before.keys())
removed = sorted(before.keys() - after.keys())
stale = sorted(
    key
    for key, entry in after.items()
    if entry.get("extractionState") == "stale" and before.get(key, {}).get("extractionState") != "stale"
)
print(f"l10n-sync: {len(after)} keys; +{len(added)} new, -{len(removed)} removed, {len(stale)} newly stale")
for label, keys in (("+", added), ("-", removed), ("stale", stale)):
    for key in keys[:40]:
        print(f"  {label} {key!r}")
    if len(keys) > 40:
        print(f"  … and {len(keys) - 40} more")
PY

# Compare the whole catalog semantically. xcstringstool omits the final newline,
# while repository hooks add it; JSON layout alone must not make a required
# check red. Keys, source values, comments and all translation metadata remain
# part of the comparison.
if python3 - "$CATALOG" "$WORK/catalog/Localizable.xcstrings" <<'PY'
import json
import sys

with open(sys.argv[1]) as before, open(sys.argv[2]) as after:
    raise SystemExit(0 if json.load(before) == json.load(after) else 1)
PY
then
  echo "l10n-sync: catalog is in step with the build."
  exit 0
fi
if [ "$CHECK" = "1" ]; then
  echo "l10n-sync: catalog differs from the build — build Debug, run 'make l10n-sync', run 'make verify-l10n-catalog', and commit the catalog with the Swift changes (see docs/LOCALIZATION.md)." >&2
  exit 1
fi
cp "$WORK/catalog/Localizable.xcstrings" "$CATALOG"
echo "l10n-sync: updated $CATALOG"
