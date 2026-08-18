#!/usr/bin/env bash
# Print 1 only when both production-truth public keys resolve.
# Used by `make install-app` to bake CSDeveloperSurface. Default is 0.
# Release / DMG builds must never call this as an enablement path.
#
# Env (same sources as Makefile distribution keys):
#   SPARKLE_ED_PUBLIC_KEY / CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE
#   CODESCRIBE_LICENSE_PUBLIC_KEY_HEX / CODESCRIBE_LICENSE_PUBLIC_KEY_FILE
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI
set -euo pipefail

DEV_PACK="${HOME}/.codescribe/config/dev/keys"
VIBE_SECRETS="${HOME}/.vibecrafted/secrets/codescribe"
if [[ -z "${CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE:-}" ]]; then
  if [[ -f "${DEV_PACK}/sparkle-public.b64" ]]; then
    CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE="${DEV_PACK}/sparkle-public.b64"
  else
    CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE="${VIBE_SECRETS}/sparkle-public.b64"
  fi
fi
if [[ -z "${CODESCRIBE_LICENSE_PUBLIC_KEY_FILE:-}" ]]; then
  if [[ -f "${DEV_PACK}/license-public.hex" ]]; then
    CODESCRIBE_LICENSE_PUBLIC_KEY_FILE="${DEV_PACK}/license-public.hex"
  else
    CODESCRIBE_LICENSE_PUBLIC_KEY_FILE="${VIBE_SECRETS}/license-public.hex"
  fi
fi
SPARKLE_FILE="$CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE"
LICENSE_FILE="$CODESCRIBE_LICENSE_PUBLIC_KEY_FILE"

read_trimmed() {
  local path="$1"
  [[ -f "$path" ]] || return 0
  tr -d '[:space:]' <"$path" || true
}

sparkle="${SPARKLE_ED_PUBLIC_KEY:-}"
if [[ -z "$sparkle" ]]; then
  sparkle="$(read_trimmed "$SPARKLE_FILE")"
fi

license="${CODESCRIBE_LICENSE_PUBLIC_KEY_HEX:-}"
if [[ -z "$license" ]]; then
  license="$(read_trimmed "$LICENSE_FILE")"
fi

sparkle_ok=0
if [[ ${#sparkle} -ge 32 && "$sparkle" =~ ^[A-Za-z0-9+/=]+$ ]]; then
  sparkle_ok=1
fi

license_ok=0
if [[ ${#license} -eq 64 && "$license" =~ ^[0-9A-Fa-f]+$ ]]; then
  license_ok=1
fi

if [[ "${CODESCRIBE_DEVELOPER_SURFACE:-}" == "0" ]]; then
  echo 0
  exit 0
fi

if [[ "$sparkle_ok" -eq 1 && "$license_ok" -eq 1 ]]; then
  echo 1
else
  echo 0
fi
