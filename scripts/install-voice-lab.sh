#!/usr/bin/env bash
# Fail-closed Voice Lab toolbox install for `make install-app`.
#
# Org-only: the operator must be able to read vetcoders/voice-lab (sibling
# checkout or git clone). External contributors cannot walk this path.
# Public Sparkle Ed + license verify keys come from the Monika pack so
# CSDeveloperSurface and agent Lab extras stay armed on this hot path.
#
# Env:
#   VOICE_LAB_REPO_URL            default git@github.com:vetcoders/voice-lab.git
#   CODESCRIBE_VOICE_LAB_SRC      existing checkout (skips clone)
#   VOICE_LAB_INSTALL_SETTINGS    1 = also copy examples/monika/settings.json
#   HOME                          runtime dest ~/.codescribe/voice-lab
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
set -euo pipefail

REPO_URL="${VOICE_LAB_REPO_URL:-git@github.com:vetcoders/voice-lab.git}"
CACHE="${HOME}/.codescribe/src/voice-lab"
RUNTIME="${HOME}/.codescribe/voice-lab"
LAUNCHER="${HOME}/.codescribe/bin/voice-lab"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CODESCRIBE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SIBLING="$(cd "${CODESCRIBE_ROOT}/.." && pwd)/voice-lab"

fail() {
  echo "install-voice-lab: $*" >&2
  exit 1
}

looks_like_voice_lab() {
  local root="$1"
  [[ -f "${root}/server.py" && -f "${root}/setup.sh" && -d "${root}/examples/monika/keys" ]]
}

remote_is_voice_lab() {
  local url="$1"
  [[ "$url" == *voice-lab* ]]
}

resolve_src() {
  if [[ -n "${CODESCRIBE_VOICE_LAB_SRC:-}" ]]; then
    echo "${CODESCRIBE_VOICE_LAB_SRC}"
    return
  fi
  if [[ -d "${SIBLING}/.git" ]] && looks_like_voice_lab "$SIBLING"; then
    echo "$SIBLING"
    return
  fi
  echo "$CACHE"
}

need_git() {
  command -v git >/dev/null 2>&1 || fail "git is required to fetch the org Voice Lab repo"
}

ensure_checkout() {
  local src="$1"
  remote_is_voice_lab "$REPO_URL" || fail "VOICE_LAB_REPO_URL must point at the org voice-lab repo (got ${REPO_URL})"

  if looks_like_voice_lab "$src"; then
    if [[ -d "${src}/.git" && "$src" == "$CACHE" ]]; then
      echo "==> updating ${src}"
      git -C "$src" remote get-url origin >/dev/null 2>&1 || fail "${src} has no origin"
      git -C "$src" fetch --tags origin
      git -C "$src" checkout --quiet main
      git -C "$src" merge --ff-only origin/main
    else
      echo "==> using existing checkout ${src}"
    fi
    return
  fi

  if [[ -e "$src" ]]; then
    fail "${src} exists but is not a Voice Lab checkout"
  fi

  need_git
  echo "==> probing ${REPO_URL}"
  if ! git ls-remote "$REPO_URL" HEAD >/dev/null 2>&1; then
    fail "no access to ${REPO_URL}. Voice Lab is org-closed. Ask for vetcoders/voice-lab, or use make app without install-app."
  fi
  mkdir -p "$(dirname "$src")"
  echo "==> cloning ${REPO_URL} → ${src}"
  git clone --branch main --single-branch "$REPO_URL" "$src"
  looks_like_voice_lab "$src" || fail "clone succeeded but ${src} is missing server.py / Monika pack"
}

run_setup() {
  local src="$1"
  [[ -x "${src}/setup.sh" ]] || fail "missing ${src}/setup.sh"
  echo "==> setup.sh → ${RUNTIME}"
  INSTALL_PUBLIC_KEYS=1 \
    INSTALL_SETTINGS="${VOICE_LAB_INSTALL_SETTINGS:-0}" \
    SKIP_CODESCRIBE_CLONE=1 \
    "${src}/setup.sh"
}

verify_runtime() {
  [[ -f "${RUNTIME}/server.py" ]] || fail "runtime missing ${RUNTIME}/server.py"
  [[ -x "$LAUNCHER" ]] || fail "launcher missing ${LAUNCHER}"
  [[ -f "${HOME}/.vibecrafted/secrets/codescribe/sparkle-public.b64" ]] \
    || fail "Sparkle public key missing after Monika pack"
  [[ -f "${HOME}/.vibecrafted/secrets/codescribe/license-public.hex" ]] \
    || fail "license public key missing after Monika pack"
  echo "==> Voice Lab runtime ${RUNTIME}"
  echo "==> launcher ${LAUNCHER}"
}

main() {
  local src
  src="$(resolve_src)"
  ensure_checkout "$src"
  run_setup "$src"
  verify_runtime
}

main "$@"
