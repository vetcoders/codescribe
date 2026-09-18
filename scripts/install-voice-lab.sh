#!/usr/bin/env bash
# Fail-closed Voice Lab toolbox install for `make install-app`.
#
# Org-only: the operator must be able to read vetcoders/voice-lab (sibling
# checkout or git clone). External contributors cannot walk this path.
# Public Sparkle Ed + license verify keys come from the operator settings pack so
# CSDeveloperSurface and agent Lab extras stay armed on this hot path.
#
# Env:
#   VOICE_LAB_REPO_URL            optional override. Empty = org HTTPS or SSH,
#                                 ordered by `gh config git_protocol` (https on
#                                 the current host). Only vetcoders/
#                                 voice-lab URLs are accepted.
#   CODESCRIBE_VOICE_LAB_SRC      existing checkout (skips clone)
#   App settings are merged by Codescribe at launch, never by this installer.
#   HOME                          runtime dest ~/.codescribe/voice-lab
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
set -euo pipefail

VOICE_LAB_HTTPS="https://github.com/vetcoders/voice-lab.git"
VOICE_LAB_SSH="git@github.com:vetcoders/voice-lab.git"
REPO_URL="${VOICE_LAB_REPO_URL:-}"
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
  [[ -f "${root}/server.py" && -f "${root}/setup.sh" ]] &&
    operator_pack_dir "$root" >/dev/null
}

operator_pack_dir() {
  local root="$1"
  local candidate
  for candidate in "${root}"/examples/*; do
    if [[ -d "${candidate}/keys" && -f "${candidate}/settings.json" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

normalize_repo_url() {
  local url="${1%%/}"
  url="${url%.git}"
  printf '%s\n' "$url"
}

# Org lock: HTTPS or SSH according to the current host. Not "any URL with voice-lab".
remote_is_voice_lab() {
  local url
  url="$(normalize_repo_url "$1")"
  case "$url" in
    https://github.com/vetcoders/voice-lab) return 0 ;;
    git@github.com:vetcoders/voice-lab) return 0 ;;
    ssh://git@github.com/vetcoders/voice-lab) return 0 ;;
    *) return 1 ;;
  esac
}

git_protocol() {
  if command -v gh >/dev/null 2>&1; then
    gh config get git_protocol 2>/dev/null || echo https
  else
    echo https
  fi
}

# Preferred transport first, the other as fallback. Explicit override is alone.
candidate_repo_urls() {
  if [[ -n "${VOICE_LAB_REPO_URL:-}" ]]; then
    printf '%s\n' "$VOICE_LAB_REPO_URL"
    return
  fi
  if [[ "$(git_protocol)" == "ssh" ]]; then
    printf '%s\n' "$VOICE_LAB_SSH" "$VOICE_LAB_HTTPS"
  else
    printf '%s\n' "$VOICE_LAB_HTTPS" "$VOICE_LAB_SSH"
  fi
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
  local cand origin probed=""

  if looks_like_voice_lab "$src"; then
    if [[ -d "${src}/.git" && "$src" == "$CACHE" ]]; then
      echo "==> updating ${src}"
      origin="$(git -C "$src" remote get-url origin 2>/dev/null || true)"
      [[ -n "$origin" ]] || fail "${src} has no origin"
      remote_is_voice_lab "$origin" || fail "${src} origin is not vetcoders/voice-lab (got ${origin})"
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
  REPO_URL=""
  while IFS= read -r cand; do
    [[ -n "$cand" ]] || continue
    remote_is_voice_lab "$cand" || fail "VOICE_LAB_REPO_URL must point at the org voice-lab repo (got ${cand})"
    echo "==> probing ${cand}"
    probed="${probed}${probed:+, }${cand}"
    if git ls-remote "$cand" HEAD >/dev/null 2>&1; then
      REPO_URL="$cand"
      break
    fi
  done < <(candidate_repo_urls)
  if [[ -z "$REPO_URL" ]]; then
    fail "no access to ${probed:-vetcoders/voice-lab}. Voice Lab is org-closed. Ask for vetcoders/voice-lab, or use make app without install-app."
  fi
  mkdir -p "$(dirname "$src")"
  echo "==> cloning ${REPO_URL} → ${src}"
  git clone --branch main --single-branch "$REPO_URL" "$src"
  looks_like_voice_lab "$src" || fail "clone succeeded but ${src} is missing server.py / operator settings pack"
}

run_setup() {
  local src="$1"
  [[ -x "${src}/setup.sh" ]] || fail "missing ${src}/setup.sh"
  echo "==> setup.sh → ${RUNTIME}"
  INSTALL_PUBLIC_KEYS=1 \
    INSTALL_SETTINGS=0 \
    SKIP_CODESCRIBE_CLONE=1 \
    "${src}/setup.sh"
}

verify_runtime() {
  [[ -f "${RUNTIME}/server.py" ]] || fail "runtime missing ${RUNTIME}/server.py"
  [[ -x "$LAUNCHER" ]] || fail "launcher missing ${LAUNCHER}"
  if [[ -f "${HOME}/.codescribe/config/dev/keys/sparkle-public.b64" ]]; then
    :
  elif [[ -f "${HOME}/.vibecrafted/secrets/codescribe/sparkle-public.b64" ]]; then
    :
  else
    fail "Sparkle public key missing (~/.codescribe/config/dev/keys or operator pack)"
  fi
  if [[ -f "${HOME}/.codescribe/config/dev/keys/license-public.hex" ]]; then
    :
  elif [[ -f "${HOME}/.vibecrafted/secrets/codescribe/license-public.hex" ]]; then
    :
  else
    fail "license public key missing (~/.codescribe/config/dev/keys or operator pack)"
  fi
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
