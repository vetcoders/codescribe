#!/bin/zsh
# CodeScribe installer (macOS) — curl | sh friendly
#
# Supports two sources:
#  1) Direct DMG URL via --url
#  2) GitHub Releases via --repo owner/name [--tag vX] or --latest (needs GH_TOKEN for private)
# Installs app to /Applications, optionally sets Start at Login.
#
# Examples:
#  curl -fsSL https://raw.githubusercontent.com/Loctree/CodeScribe/develop/packaging/scripts/install.sh | zsh -s -- --url "https://example.com/CodeScribe-0.1.0.dmg"
#  GH_TOKEN=xxxx curl -H "Authorization: token $GH_TOKEN" -fsSL \
#    https://raw.githubusercontent.com/Loctree/CodeScribe/develop/packaging/scripts/install.sh | zsh -s -- --repo Loctree/CodeScribe --latest

set -euo pipefail

[[ "$(uname -s)" == "Darwin" ]] || { echo "❌ macOS only" >&2; exit 1; }

URL=""
REPO=""
TAG=""
LATEST=0
LOGIN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --url) URL="$2"; shift 2;;
    --repo) REPO="$2"; shift 2;;
    --tag) TAG="$2"; shift 2;;
    --latest) LATEST=1; shift;;
    --login) LOGIN=1; shift;;
    -h|--help) echo "Usage: install.sh [--url <DMG>] | [--repo owner/name --latest|--tag <tag>] [--login]"; exit 0;;
    *) echo "Unknown arg: $1" >&2; exit 2;;
  esac
done

TMPDIR="${TMPDIR:-/tmp}"
DMG_PATH=""

fetch_dmg() {
  local out="$TMPDIR/CodeScribe.$$.$RANDOM.dmg"
  if [[ -n "$URL" ]]; then
    echo "⬇ Downloading DMG…"
    curl -fL --retry 3 --output "$out" "$URL"
    echo "🔐 SHA256: $(shasum -a 256 "$out" | awk '{print $1}')"
    echo "ℹ️  Verify this hash against the publisher's release notes before continuing."
    echo "$out"; return 0
  fi
  if [[ -n "$REPO" ]]; then
    local api="https://api.github.com/repos/$REPO/releases"
    local hdrs=()
    [[ -n "${GH_TOKEN:-}" ]] && hdrs+=( -H "Authorization: token $GH_TOKEN" )
    local json
    if [[ $LATEST -eq 1 ]]; then
      json=$(curl -fsSL "${hdrs[@]}" "$api/latest")
    else
      json=$(curl -fsSL "${hdrs[@]}" "$api/tags/${TAG}" || curl -fsSL "${hdrs[@]}" "$api")
    fi
    local dmg_url
    dmg_url=$(echo "$json" | awk -F '"' '/browser_download_url/ && /\.dmg"/{print $4; exit}')
    [[ -n "$dmg_url" ]] || { echo "❌ No DMG asset found in releases" >&2; exit 3; }
    echo "⬇ Downloading DMG from GitHub Releases…"
    curl -fL --retry 3 ${GH_TOKEN:+-H "Authorization: token $GH_TOKEN"} -o "$out" "$dmg_url"
    echo "🔐 SHA256: $(shasum -a 256 "$out" | awk '{print $1}')"
    echo "ℹ️  Verify this hash against the release announcement or signature."
    echo "$out"; return 0
  fi
  echo "❌ Provide --url or --repo/--latest" >&2; exit 2
}

install_app() {
  local dmg="$1"
  echo "🗂️  Mounting DMG…"
  local mp
  mp=$(hdiutil attach -nobrowse -quiet "$dmg" | awk '{print $3}' | tail -n1)
  [[ -d "$mp" ]] || { echo "❌ Failed to mount DMG" >&2; exit 4; }
  trap 'hdiutil detach -quiet "$mp" || true' EXIT
  local src
  src=$(find "$mp" -maxdepth 1 -name "CodeScribe.app" -or -name "Vista Scribe.app" | head -n1)
  [[ -d "$src" ]] || { echo "❌ App not found in DMG" >&2; exit 5; }
  echo "📥 Copying to /Applications…"
  rsync -a --delete "$src/" "/Applications/CodeScribe.app/"
  echo "✅ Installed: /Applications/CodeScribe.app"
}

setup_login() {
  local plist="$HOME/Library/LaunchAgents/com.codescribe.tray.plist"
  mkdir -p "$(dirname "$plist")"
  /usr/bin/plutil -convert xml1 -o "$plist" - <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.codescribe.tray</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/zsh</string>
    <string>-lc</string>
    <string>cd "/Applications/CodeScribe.app/Contents/Resources/Repo" && ./scripts/quickstart_mac.sh --mode both --daemon --log "$HOME/Library/Logs/CodeScribe.app.log"</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
  <key>StandardOutPath</key><string>$HOME/Library/Logs/CodeScribe.launchd.out.log</string>
  <key>StandardErrorPath</key><string>$HOME/Library/Logs/CodeScribe.launchd.err.log</string>
</dict></plist>
PLIST
  launchctl unload -w "$plist" >/dev/null 2>&1 || true
  launchctl load -w "$plist" || true
  echo "🔁 Start at Login enabled"
}

main() {
  DMG_PATH=$(fetch_dmg)
  install_app "$DMG_PATH"
  [[ $LOGIN -eq 1 ]] && setup_login
  echo "🚀 Launching app…"
  open -a "/Applications/CodeScribe.app" || true
  echo "Done. Logs: ~/Library/Logs/CodeScribe.app.log"
}

main "$@"
