#!/usr/bin/env bash
# make install-bus: helpers and selected managed skills, no app build.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
"$ROOT/scripts/build-app.sh" --stage-agent-bridge "$STAGE/payload"
xcrun swiftc -swift-version 6 -warnings-as-errors -D CODESCRIBE_AGENT_BRIDGE_STANDALONE \
  "$ROOT/macos/Codescribe/Services/AgentBridgeInstaller.swift" \
  "$ROOT/scripts/agent-bridge-install.swift" -o "$STAGE/install"
"$STAGE/install" "$STAGE/payload"
echo "Commands: ~/.local/bin/cs-bus and ~/.local/bin/cs-say"
echo "Existing followers stay alive. Reattach this session to adopt the updated runtime."
