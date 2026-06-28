#!/usr/bin/env bash
set -euo pipefail

# Safe cleaner for development artifacts. Default is dry-run.
# Usage:
#   scripts/clean.sh            # dry run (prints what would be removed)
#   scripts/clean.sh --apply    # actually remove

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ ! -f pyproject.toml ]]; then
  echo "[!] Not at repo root (pyproject.toml missing). Abort." >&2
  exit 1
fi

APPLY=false
if [[ "${1:-}" == "--apply" || "${1:-}" == "-y" ]]; then
  APPLY=true
fi

targets=(
  "logs/*"
  "outputs/*"
  "user-messages/*"
  "Codescribe.log"
  ".codescribe.lock"
  ".pytest_cache"
  "__pycache__"
  "bundle"
  "Codescribe_*.dmg"
  "dist"
  "build"
)

echo "==> Cleaning targets (dry-run=${APPLY=false})"
for t in "${targets[@]}"; do
  matches=( $(eval echo "$t") ) || true
  if [[ ${#matches[@]} -gt 0 ]]; then
    for m in "${matches[@]}"; do
      if [[ -e "$m" ]]; then
        echo "- $m"
        if $APPLY; then
          rm -rf "$m"
        fi
      fi
    done
  fi
done

if $APPLY; then
  echo "Done."
else
  echo "(dry-run) Nothing removed. Re-run with --apply to clean."
fi
