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

# Substring "voice-lab" is not enough — attacker-controlled fork is rejected.
set +e
out="$(
  CODESCRIBE_VOICE_LAB_SRC="$ABSENT" \
  VOICE_LAB_REPO_URL="https://github.com/octocat/voice-lab.git" \
    "$INSTALL" 2>&1
)"
status=$?
set -e
[[ "$status" -ne 0 ]] || { echo "expected fail on non-org voice-lab URL, got: $out" >&2; exit 1; }
[[ "$out" == *"voice-lab repo"* ]] || { echo "expected org lock, got: $out" >&2; exit 1; }

# Unset URL: HTTPS first when gh git_protocol=https, then SSH.
probe_log="$WORKDIR/probes.log"
: >"$probe_log"
cat >"$fake_git/gh" <<'EOF'
#!/bin/sh
if [ "$1" = "config" ] && [ "$2" = "get" ] && [ "$3" = "git_protocol" ]; then
  echo https
  exit 0
fi
exit 1
EOF
chmod +x "$fake_git/gh"
cat >"$fake_git/git" <<EOF
#!/bin/sh
if [ "\$1" = "ls-remote" ]; then
  echo "\$2" >> "$probe_log"
  echo "Permission denied" >&2
  exit 128
fi
echo "unexpected git \$*" >&2
exit 99
EOF
set +e
out="$(
  CODESCRIBE_VOICE_LAB_SRC="$ABSENT" \
    "$INSTALL" 2>&1
)"
status=$?
set -e
[[ "$status" -ne 0 ]] || { echo "expected fail after both probes, got: $out" >&2; exit 1; }
[[ "$out" == *org-closed* ]] || { echo "expected org-closed after dual probe, got: $out" >&2; exit 1; }
probe_n="$(wc -l <"$probe_log" | tr -d ' ')"
https_probe="$(sed -n '1p' "$probe_log")"
ssh_probe="$(sed -n '2p' "$probe_log")"
[[ "$probe_n" == "2" ]] || { echo "expected 2 probes, got ${probe_n}: $(cat "$probe_log")" >&2; exit 1; }
[[ "$https_probe" == "https://github.com/vetcoders/voice-lab.git" ]] || {
  echo "https protocol must probe HTTPS first, got ${https_probe}" >&2
  exit 1
}
[[ "$ssh_probe" == "git@github.com:vetcoders/voice-lab.git" ]] || {
  echo "second probe must be SSH fallback, got ${ssh_probe}" >&2
  exit 1
}

# An SSH-configured host probes SSH first.
: >"$probe_log"
cat >"$fake_git/gh" <<'EOF'
#!/bin/sh
if [ "$1" = "config" ] && [ "$2" = "get" ] && [ "$3" = "git_protocol" ]; then
  echo ssh
  exit 0
fi
exit 1
EOF
set +e
out="$(
  CODESCRIBE_VOICE_LAB_SRC="$ABSENT" \
    "$INSTALL" 2>&1
)"
status=$?
set -e
[[ "$status" -ne 0 ]] || { echo "expected fail after ssh-first probes, got: $out" >&2; exit 1; }
ssh_first="$(sed -n '1p' "$probe_log")"
https_second="$(sed -n '2p' "$probe_log")"
[[ "$ssh_first" == "git@github.com:vetcoders/voice-lab.git" ]] || {
  echo "ssh protocol must probe SSH first, got ${ssh_first}" >&2
  exit 1
}
[[ "$https_second" == "https://github.com/vetcoders/voice-lab.git" ]] || {
  echo "ssh protocol second probe must be HTTPS, got ${https_second}" >&2
  exit 1
}

# Restore the fail-closed git stub used by later cases.
cat >"$fake_git/git" <<'EOF'
#!/bin/sh
if [ "$1" = "ls-remote" ]; then
  echo "Permission denied" >&2
  exit 128
fi
echo "unexpected git $*" >&2
exit 99
EOF
rm -f "$fake_git/gh"

# A checkout is accepted only as a Git root with the org origin and live
# access. This stub answers those probes for every path, like an operator
# machine with access.
cat >"$fake_git/git" <<EOF
#!/bin/sh
dir="\$PWD"
if [ "\$1" = "-C" ]; then dir="\$2"; shift 2; fi
case "\$1 \${2:-}" in
  "rev-parse --show-toplevel") (cd "\$dir" && pwd) ;;
  "remote get-url") echo "https://github.com/vetcoders/voice-lab.git" ;;
  "ls-remote "*) exit 0 ;;
  "fetch "*|"checkout "*|"merge "*) echo "git \$* in \$dir" >> "$WORKDIR/git.log" ;;
  *) echo "unexpected git \$*" >&2; exit 99 ;;
esac
EOF

make_pack() {
  local pack="$1" label="$2"
  mkdir -p "$pack/examples/operator/keys" "$pack/.git"
  printf 'x' >"$pack/server.py"
  printf '%s\n' '{"speech":{"engine":{"asr_mode":"local_power"}}}' \
    >"$pack/examples/operator/settings.json"
  cat >"$pack/setup.sh" <<EOF
#!/bin/sh
set -eu
mkdir -p "\$HOME/.codescribe/voice-lab" "\$HOME/.codescribe/bin" "\$HOME/.vibecrafted/secrets/codescribe"
echo ${label} > "\$HOME/.codescribe/voice-lab/server.py"
echo '#!/bin/sh' > "\$HOME/.codescribe/bin/voice-lab"
chmod 755 "\$HOME/.codescribe/bin/voice-lab"
echo sparkle > "\$HOME/.vibecrafted/secrets/codescribe/sparkle-public.b64"
echo license > "\$HOME/.vibecrafted/secrets/codescribe/license-public.hex"
echo ran-setup-${label}
EOF
  chmod +x "$pack/setup.sh"
}

# A named checkout via CODESCRIBE_VOICE_LAB_SRC runs its setup.sh. App settings
# are merged by Codescribe at launch, so the installer never writes them.
pack="$WORKDIR/pack"
make_pack "$pack" named
out="$(CODESCRIBE_VOICE_LAB_SRC="$pack" "$INSTALL" 2>&1)"
[[ "$(cat "$HOME/.codescribe/voice-lab/server.py")" == named ]] || { echo "runtime not installed: $out" >&2; exit 1; }
[[ -x "$HOME/.codescribe/bin/voice-lab" ]] || { echo "launcher missing" >&2; exit 1; }
[[ "$out" == *ran-setup-named* ]] || { echo "setup.sh did not run: $out" >&2; exit 1; }
[[ ! -e "$HOME/Library/Application Support/Codescribe/settings.json" ]] || {
  echo "installer must not write app settings" >&2
  exit 1
}

# Without a named checkout the Lab comes from the cache on main, even when a
# sibling checkout exists on another branch (2026-10-17: the Silero take chart
# vanished because install-app copied the sibling's feature branch).
export HOME="$WORKDIR/home-cache"
tree="$WORKDIR/tree"
mkdir -p "$tree/codescribe/scripts"
cp "$INSTALL" "$tree/codescribe/scripts/install-voice-lab.sh"
make_pack "$tree/voice-lab" sibling
make_pack "$HOME/.codescribe/src/voice-lab" cache
: >"$WORKDIR/git.log"
out="$("$tree/codescribe/scripts/install-voice-lab.sh" 2>&1)"
[[ "$out" == *ran-setup-cache* && "$out" != *ran-setup-sibling* ]] || {
  echo "expected the cache on main, got: $out" >&2
  exit 1
}
[[ "$out" == *"$tree/voice-lab not installed"* ]] || { echo "expected the sibling note: $out" >&2; exit 1; }
[[ "$(cat "$WORKDIR/git.log")" == *"merge --ff-only origin/main in $HOME/.codescribe/src/voice-lab"* ]] || {
  echo "cache must fast-forward to origin/main: $(cat "$WORKDIR/git.log")" >&2
  exit 1
}

# Before the first install the sibling still proves org access.
export HOME="$WORKDIR/home-fresh"
mkdir -p "$HOME"
"$tree/codescribe/scripts/install-voice-lab.sh" --verify-access ||
  { echo "sibling must prove access before the cache exists" >&2; exit 1; }

echo "install-voice-lab: ok"
