#!/usr/bin/env bash
# Quickstart for macOS using uv. No API keys needed for local models.
# Usage:
#   chmod +x scripts/quickstart_mac.sh
#   ./scripts/quickstart_mac.sh
#
# Optional env vars:
#   WHISPER_VARIANT=medium|large-v3-turbo (default: large-v3-turbo)
#   FORMAT_ENABLED=0|1 (default: 1 -> paste formatted text)
#   MODE=tray|backend (default: tray)
#   LLM_ID=<local path or HF mlx repo id> (optional; requires FORMAT_ENABLED=1)
#
# Tips:
# - On first microphone/paste/hotkey use, macOS will prompt for permissions.
# - If MLX complains about /Users paths, prefer lowercase /users if available.

set -euo pipefail

# Check for uv
if ! command -v uv >/dev/null 2>&1; then
  echo "[!] Nie znaleziono 'uv'. Zainstaluj i uruchom ponownie powłokę:"
  echo "  curl -LsSf https://astral.sh/uv/install.sh | sh"
  echo "  exec -l $SHELL"
  exit 1
fi

# Go to repo root
cd "$(dirname "${BASH_SOURCE[0]}")/.."
REPO_DIR="$(pwd)"
LOG_DIR="$REPO_DIR/logs"
mkdir -p "$LOG_DIR"

# Default values for parameters (use :- for proper default substitution)
WHISPER_VARIANT="${WHISPER_VARIANT:-large-v3-turbo}"
FORMAT_ENABLED="${FORMAT_ENABLED:-1}"
MODE="${MODE:-tray}"
LLM_ID="${LLM_ID:-}"

# MLX path quirk: prefer lowercase /users when available
lower_users() {
  local p="$1"
  if [[ "$p" == /Users/* ]]; then
    local fixed="/users/${p#*/Users/}"
    if [[ -e "$fixed" ]]; then
      echo "$fixed"; return
    fi
  fi
  echo "$p"
}

echo "==> Synchronizuję środowisko (uv sync)…"
uv sync

# Activate the venv for this script's process
if [[ -f .venv/bin/activate ]]; then
  source .venv/bin/activate
fi

echo "==> Pobieram modele (Whisper=${WHISPER_VARIANT})…"
if [[ -n "${LLM_ID}" ]]; then
  uv run python scripts/get_models.py --whisper "${WHISPER_VARIANT}" --llm "${LLM_ID}"
else
  uv run python scripts/get_models.py --whisper "${WHISPER_VARIANT}"
  # Try to find a local LLM model if formatting is enabled
  if [[ "$FORMAT_ENABLED" != "0" && "$FORMAT_ENABLED" != "false" && "$FORMAT_ENABLED" != "no" ]]; then
    MODEL_DIR="$REPO_DIR/models"
    for d in "$MODEL_DIR"/*; do
      if [[ -d "$d" && "$(basename "$d")" != whisper-* ]]; then
        if [[ -f "$d/tokenizer.json" || -f "$d/config.json" ]]; then
          LLM_ID="$d"
          break
        fi
      fi
    done
    # If no model found, try default path
    if [[ -z "$LLM_ID" && -d "$MODEL_DIR/bielik-4.5b-mxfp4-mlx" ]]; then
      LLM_ID="$MODEL_DIR/bielik-4.5b-mxfp4-mlx"
    fi
    # Normalize path if we found an LLM
    if [[ -n "$LLM_ID" ]]; then
      LLM_ID="$(lower_users "$LLM_ID")"
      echo "==> Znaleziono model LLM: $LLM_ID"
    fi
  fi
fi

# Resolve WHISPER_DIR
MODEL_DIR="$REPO_DIR/models"
if [[ -d "$MODEL_DIR/whisper-$WHISPER_VARIANT" ]]; then
  WHISPER_DIR="$MODEL_DIR/whisper-$WHISPER_VARIANT"
else
  # fallback to large then medium
  if [[ -d "$MODEL_DIR/whisper-large-v3-turbo" ]]; then
    WHISPER_DIR="$MODEL_DIR/whisper-large-v3-turbo"
  else
    WHISPER_DIR="$MODEL_DIR/whisper-medium"
  fi
fi
WHISPER_DIR="$(lower_users "$WHISPER_DIR")"

echo "==> Start aplikacji (${MODE})…"
if [[ "${MODE}" == "backend" ]]; then
  echo "Uruchamiam backend (HTTP API) na http://127.0.0.1:8237"
  WHISPER_DIR="$WHISPER_DIR" FORMAT_ENABLED="$FORMAT_ENABLED" HOST="127.0.0.1" PORT="8237" \
    ${LLM_ID:+LLM_ID="$LLM_ID"} uv run python backend.py
else
  echo "Uruchamiam aplikację w zasobniku systemowym (tray)"
  WHISPER_DIR="$WHISPER_DIR" FORMAT_ENABLED="$FORMAT_ENABLED" \
    ${LLM_ID:+LLM_ID="$LLM_ID"} uv run python main.py
fi

# Notes for the user (reached after app exits)
cat <<'EOF'

Gotowe.
- Jeśli nic nie nagrywa / nie wkleja: System Settings → Privacy & Security:
  • Microphone (Terminal/Python)
  • Accessibility (Terminal/Python)
  • Input Monitoring (Terminal/Python)
- Skróty:
  • Podwójny Option (⌥⌥) – start/stop
  • Shift+Command+/ (⇧⌘/) – start/stop
  • Przytrzymaj Control – naciśnij i mów
- Jeśli używasz ścieżek absolutnych do modeli i MLX narzeka na /Users, spróbuj /users.

EOF