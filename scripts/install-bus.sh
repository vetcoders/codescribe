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

if [[ $# -eq 1 && "$1" == "--compile-only" ]]; then
  build_installer
  echo "install-bus: the installer source compiles on its own; nothing was installed."
  exit 0
fi
if [[ $# -ne 0 ]]; then
  echo "usage: install-bus.sh [--compile-only]" >&2
  exit 2
fi

# A helper-only update retains the manifest-verified installed Rust publisher.
# Refuse before replacing any payload if the managed artifact is unavailable.
PUBLISHER="$(python3 - "$ROOT" <<'PY'
import importlib.util
import sys
from pathlib import Path

source = Path(sys.argv[1]) / "scripts" / "bus-demux.py"
spec = importlib.util.spec_from_file_location("install_bus_publisher", source)
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
root = module.bridge_home()
if not (root / "runtime/bin/codescribe").is_file() or not (root / "runtime/manifest.json").is_file():
    raise SystemExit("install-bus: managed publisher missing; install the complete app payload first")
try:
    print(module.reply_publisher_command(root))
except (OSError, ValueError) as error:
    raise SystemExit(f"install-bus: publisher verification refused: {error}") from error
PY
)"
"$ROOT/scripts/build-app.sh" --stage-agent-bridge "$STAGE/payload" "" "$PUBLISHER"
build_installer
"$STAGE/install" "$STAGE/payload"
echo "Commands: ~/.local/bin/cs-bus and ~/.local/bin/cs-say"
echo "Existing followers stay alive. Reattach this session to adopt the updated runtime."
