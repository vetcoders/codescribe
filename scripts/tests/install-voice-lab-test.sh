#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALL="$ROOT/scripts/install-voice-lab.sh"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

export HOME="$WORKDIR/home"
mkdir -p "$HOME"

fake_git="$WORKDIR/bin"
mkdir -p "$fake_git"
cat >"$fake_git/git" <<'EOF'
#!/bin/sh
echo "unexpected git $*" >&2
exit 99
EOF
chmod +x "$fake_git/git"
export PATH="$fake_git:$PATH"

ABSENT="$WORKDIR/absent"

# No checkout, ls-remote fails → fail-closed. Force a clone path so a
# sibling voice-lab checkout on the operator machine cannot satisfy this.
cat >"$fake_git/git" <<'EOF'
#!/bin/sh
if [ "$1" = "ls-remote" ]; then
  echo "Permission denied" >&2
  exit 128
fi
echo "unexpected git $*" >&2
exit 99
EOF
set +e
out="$(
  CODESCRIBE_VOICE_LAB_SRC="$ABSENT" \
  VOICE_LAB_REPO_URL="git@github.com:vetcoders/voice-lab.git" \
    "$INSTALL" 2>&1
)"
status=$?
set -e
[[ "$status" -ne 0 ]] || { echo "expected fail without repo access, got: $out" >&2; exit 1; }
[[ "$out" == *org-closed* ]] || { echo "expected org-closed message, got: $out" >&2; exit 1; }

# Wrong repo URL → fail before clone.
set +e
out="$(
  CODESCRIBE_VOICE_LAB_SRC="$ABSENT" \
  VOICE_LAB_REPO_URL="https://github.com/octocat/Hello-World.git" \
    "$INSTALL" 2>&1
)"
status=$?
set -e
[[ "$status" -ne 0 ]] || { echo "expected fail on non-voice-lab URL, got: $out" >&2; exit 1; }
[[ "$out" == *"voice-lab repo"* ]] || { echo "expected repo-name check, got: $out" >&2; exit 1; }

# Sibling-shaped checkout via CODESCRIBE_VOICE_LAB_SRC runs setup.sh.
pack="$WORKDIR/pack"
mkdir -p "$pack/examples/monika/keys"
printf 'x' >"$pack/server.py"
cat >"$pack/examples/monika/settings.json" <<'JSON'
{
  "schema_version": 3,
  "speech": {
    "engine": {
      "cloud_transcription_endpoint": "wss://api.libraxis.cloud/v1/audio/transcribe",
      "asr_mode": "local_power"
    }
  }
}
JSON
cat >"$pack/setup.sh" <<EOF
#!/bin/sh
set -eu
mkdir -p "\$HOME/.codescribe/voice-lab" "\$HOME/.codescribe/bin" "\$HOME/.vibecrafted/secrets/codescribe"
echo setup-ok > "\$HOME/.codescribe/voice-lab/server.py"
echo '#!/bin/sh' > "\$HOME/.codescribe/bin/voice-lab"
chmod 755 "\$HOME/.codescribe/bin/voice-lab"
echo sparkle > "\$HOME/.vibecrafted/secrets/codescribe/sparkle-public.b64"
echo license > "\$HOME/.vibecrafted/secrets/codescribe/license-public.hex"
echo ran-setup
EOF
chmod +x "$pack/setup.sh"

out="$(CODESCRIBE_VOICE_LAB_SRC="$pack" "$INSTALL" 2>&1)"
[[ -f "$HOME/.codescribe/voice-lab/server.py" ]] || { echo "runtime not installed" >&2; exit 1; }
[[ -x "$HOME/.codescribe/bin/voice-lab" ]] || { echo "launcher missing" >&2; exit 1; }
[[ "$out" == *ran-setup* ]] || { echo "setup.sh did not run: $out" >&2; exit 1; }
[[ "$out" == *seeded\ app\ settings* ]] || { echo "expected seed on missing settings: $out" >&2; exit 1; }
[[ "$out" == *endpoint=wss://api.libraxis.cloud/v1/audio/transcribe* ]] || {
  echo "expected libraxis guarantee, got: $out" >&2
  exit 1
}

# Existing loopback endpoint must not be overwritten.
export HOME="$WORKDIR/home-keep"
mkdir -p "$HOME/Library/Application Support/Codescribe"
cat >"$HOME/Library/Application Support/Codescribe/settings.json" <<'JSON'
{
  "speech": {
    "engine": {
      "cloud_transcription_endpoint": "ws://127.0.0.1:8446/v1/audio/transcribe",
      "asr_mode": "local_power"
    }
  }
}
JSON
out="$(CODESCRIBE_VOICE_LAB_SRC="$pack" "$INSTALL" 2>&1)"
[[ "$out" == *app\ settings\ kept* ]] || { echo "expected keep existing endpoint: $out" >&2; exit 1; }
[[ "$out" == *endpoint=ws://127.0.0.1:8446/v1/audio/transcribe* ]] || {
  echo "loopback must survive seed, got: $out" >&2
  exit 1
}

# Empty engine keys get the pack values without replacing the file wholesale.
export HOME="$WORKDIR/home-fill"
mkdir -p "$HOME/Library/Application Support/Codescribe"
printf '%s\n' '{"schema_version":3,"speech":{"engine":{}}}' \
  >"$HOME/Library/Application Support/Codescribe/settings.json"
out="$(CODESCRIBE_VOICE_LAB_SRC="$pack" "$INSTALL" 2>&1)"
[[ "$out" == *filled\ empty\ engine\ keys* ]] || { echo "expected fill: $out" >&2; exit 1; }
[[ "$out" == *asr_mode=local_power* ]] || { echo "expected mode fill: $out" >&2; exit 1; }

echo "install-voice-lab: ok"
