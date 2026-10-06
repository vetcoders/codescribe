#!/usr/bin/env bash
# make install-bus: helpers and selected managed skills, no app build.
# --compile-only builds the installer and stops before staging or installing
# anything; that is `make verify-install-bus`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# The installer is these two files and nothing else from the app target.
build_installer() {
  xcrun swiftc -swift-version 6 -warnings-as-errors \
    "$ROOT/macos/Codescribe/Services/AgentBridgeInstaller.swift" \
    "$ROOT/scripts/agent-bridge-install.swift" -o "$STAGE/install"
}

if [[ "${1:-}" == "--compile-only" ]]; then
  build_installer
  echo "install-bus: the installer source compiles on its own; nothing was installed."
  exit 0
fi
if [[ $# -ne 0 ]]; then
  echo "usage: install-bus.sh [--compile-only]" >&2
  exit 2
fi

"$ROOT/scripts/build-app.sh" --stage-agent-bridge "$STAGE/payload"
build_installer
"$STAGE/install" "$STAGE/payload"
echo "Commands: ~/.local/bin/cs-bus and ~/.local/bin/cs-say"
echo "Existing followers stay alive. Reattach this session to adopt the updated runtime."
