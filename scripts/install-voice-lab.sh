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
#   VOICE_LAB_INSTALL_SETTINGS    unset = seed missing app settings + empty
#                                 engine keys from examples/monika/settings.json
#                                 1 = overwrite settings (setup.sh backup)
#                                 0 = never touch Application Support settings
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

app_settings_path() {
  echo "${HOME}/Library/Application Support/Codescribe/settings.json"
}

# Codescribe does not invent the org cloud URL on first launch. The Monika
# pack in voice-lab does. Seed it on this hot path: missing file → copy;
# existing file → fill empty asr_mode / cloud_transcription_endpoint only.
# A host that already pointed STT at loopback or Libraxis is left alone.
seed_app_settings() {
  local src="$1"
  local pack="${src}/examples/monika/settings.json"
  local dest
  dest="$(app_settings_path)"
  local mode="${VOICE_LAB_INSTALL_SETTINGS:-auto}"

  if [[ "$mode" == "0" ]]; then
    echo "==> app settings skipped (VOICE_LAB_INSTALL_SETTINGS=0)"
    return
  fi
  [[ -f "$pack" ]] || fail "Monika settings pack missing: ${pack}"

  if [[ "$mode" == "1" ]]; then
    return
  fi

  command -v python3 >/dev/null 2>&1 || fail "python3 is required to seed Codescribe settings"
  python3 - "$pack" "$dest" <<'PY'
import json
import sys
from pathlib import Path

pack = Path(sys.argv[1])
dest = Path(sys.argv[2])
wanted = json.loads(pack.read_text())
engine = (wanted.get("speech") or {}).get("engine") or {}
want_endpoint = (engine.get("cloud_transcription_endpoint") or "").strip()
want_mode = (engine.get("asr_mode") or "").strip()

if not dest.is_file():
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(pack.read_text())
    print(f"==> seeded app settings from Monika pack → {dest}")
    raise SystemExit(0)

data = json.loads(dest.read_text())
speech = data.setdefault("speech", {})
cur = speech.setdefault("engine", {})
changed = []
if not str(cur.get("cloud_transcription_endpoint") or "").strip() and want_endpoint:
    cur["cloud_transcription_endpoint"] = want_endpoint
    changed.append("cloud_transcription_endpoint")
if not str(cur.get("asr_mode") or "").strip() and want_mode:
    cur["asr_mode"] = want_mode
    changed.append("asr_mode")
if changed:
    dest.write_text(json.dumps(data, indent=2) + "\n")
    print("==> filled empty engine keys:", ", ".join(changed))
else:
    print("==> app settings kept (endpoint/mode already set)")
PY
}

print_settings_guarantee() {
  local dest
  dest="$(app_settings_path)"
  if [[ ! -f "$dest" ]]; then
    echo "==> app settings: none at ${dest}"
    return
  fi
  command -v python3 >/dev/null 2>&1 || return
  python3 - "$dest" <<'PY'
import json, sys
from pathlib import Path
data = json.loads(Path(sys.argv[1]).read_text())
engine = (data.get("speech") or {}).get("engine") or {}
mode = engine.get("asr_mode") or "(unset)"
endpoint = engine.get("cloud_transcription_endpoint") or "(unset)"
print(f"==> app settings guarantee asr_mode={mode}")
print(f"==> app settings guarantee endpoint={endpoint}")
PY
}

run_setup() {
  local src="$1"
  [[ -x "${src}/setup.sh" ]] || fail "missing ${src}/setup.sh"
  local settings_flag="${VOICE_LAB_INSTALL_SETTINGS:-0}"
  if [[ -z "${VOICE_LAB_INSTALL_SETTINGS:-}" ]]; then
    settings_flag=0
  fi
  echo "==> setup.sh → ${RUNTIME}"
  INSTALL_PUBLIC_KEYS=1 \
    INSTALL_SETTINGS="$settings_flag" \
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
    fail "Sparkle public key missing (~/.codescribe/config/dev/keys or Monika pack)"
  fi
  if [[ -f "${HOME}/.codescribe/config/dev/keys/license-public.hex" ]]; then
    :
  elif [[ -f "${HOME}/.vibecrafted/secrets/codescribe/license-public.hex" ]]; then
    :
  else
    fail "license public key missing (~/.codescribe/config/dev/keys or Monika pack)"
  fi
  echo "==> Voice Lab runtime ${RUNTIME}"
  echo "==> launcher ${LAUNCHER}"
}

main() {
  local src
  src="$(resolve_src)"
  ensure_checkout "$src"
  run_setup "$src"
  seed_app_settings "$src"
  verify_runtime
  print_settings_guarantee
}

main "$@"
