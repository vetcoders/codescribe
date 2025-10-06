#!/usr/bin/env bash
# Quickstart for macOS using uv. No API keys needed for local models.
# Usage:
#   chmod +x scripts/quickstart_mac.sh
#   ./scripts/quickstart_mac.sh [flags]
#
# Flags (override env vars):
#   --mode tray|backend|both            # default tray
#   --whisper medium|large-v3-turbo|large-v3
#   --format 0|1                        # enable/disable formatting
#   --llm-id <path|hf-repo>
#   --active                            # use uv --active (silences VIRTUAL_ENV warning)
#   --daemon [--log FILE]               # run tray/backend in background (default log: VistaScribe.log)
#   --with-backend                      # start backend alongside tray (same as --mode both)
#   --no-models                         # skip model downloader
#   --hold-mods <ctrl|ctrl+alt|...>     # persist to .env
#   --hold-exclusive 0|1                # persist to .env
#   --beep 0|1 --sound-name Tink|Pop --sound-volume 0.0-1.0  # persist to .env
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

print_help() {
  sed -n '1,80p' "$0" | sed -n '1,40p'
}

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
PID_DIR="$REPO_DIR/.pids"
mkdir -p "$PID_DIR"

# Ensure we do NOT inherit a foreign virtualenv (e.g., from old ../vista-scribe)
if [[ -n "${VIRTUAL_ENV:-}" ]]; then
  echo "warning: ignoring inherited VIRTUAL_ENV=$VIRTUAL_ENV; using local .venv instead"
  unset VIRTUAL_ENV
fi

# Default values for parameters (use :- for proper default substitution)
WHISPER_VARIANT="${WHISPER_VARIANT:-large-v3-turbo}"
FORMAT_ENABLED="${FORMAT_ENABLED:-1}"
MODE="${MODE:-tray}"
LLM_ID="${LLM_ID:-}"
UV_ACTIVE=0
DAEMON=0
LOG_FILE="$REPO_DIR/VistaScribe.log"
SKIP_MODELS=0
PERSIST_ENVS=()
WITH_BACKEND=0
STOP_TRAY=0
STOP_BACK=0
STOP_ALL=0

graceful_kill() {
  local pid="$1"; local name="$2"; local timeout=5
  if kill -0 "$pid" >/dev/null 2>&1; then
    echo "==> SIGTERM $name (pid $pid)"
    kill "$pid" || true
    for _ in $(seq 1 $timeout); do
      sleep 1
      kill -0 "$pid" >/dev/null 2>&1 || break
    done
    if kill -0 "$pid" >/dev/null 2>&1; then
      echo "[!] SIGKILL $name (pid $pid)"
      kill -9 "$pid" || true
    fi
  fi
}

write_pid() {
  local name="$1"; local pid="$2"; echo "$pid" > "$PID_DIR/${name}.pid";
}

stop_by_name() {
  local name="$1"; local fallback="$2"; local f="$PID_DIR/${name}.pid"
  if [[ -f "$f" ]]; then
    local pid; pid=$(cat "$f" 2>/dev/null || echo "")
    if [[ -n "$pid" ]]; then
      graceful_kill "$pid" "$name"
    fi
    rm -f "$f" || true
  else
    # fallback by pattern
    pkill -f "$fallback" || true
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) print_help; exit 0;;
    --mode) MODE="$2"; shift 2;;
    --whisper) WHISPER_VARIANT="$2"; shift 2;;
    --format) FORMAT_ENABLED="$2"; shift 2;;
    --llm-id) LLM_ID="$2"; shift 2;;
    --active) UV_ACTIVE=1; shift;;
    --daemon) DAEMON=1; shift;;
    --log) LOG_FILE="$2"; shift 2;;
    --no-models) SKIP_MODELS=1; shift;;
    --with-backend) WITH_BACKEND=1; shift;;
    --stop) STOP_TRAY=1; shift;;
    --stop-backend) STOP_BACK=1; shift;;
    --stop-all) STOP_ALL=1; shift;;
    --hold-mods) PERSIST_ENVS+=("HOLD_MODS=$2"); shift 2;;
    --hold-exclusive) PERSIST_ENVS+=("HOLD_EXCLUSIVE=$2"); shift 2;;
    --beep) PERSIST_ENVS+=("BEEP_ON_START=$2"); shift 2;;
    --sound-name) PERSIST_ENVS+=("SOUND_NAME=$2"); shift 2;;
    --sound-volume) PERSIST_ENVS+=("SOUND_VOLUME=$2"); shift 2;;
    *) echo "[!] Unknown flag: $1"; print_help; exit 2;;
  esac
done

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
# Ensure local .venv exists and is used
if [[ ! -d .venv ]]; then
  uv venv .venv
fi
# Activate the venv for this script's process and force uv to use it
source .venv/bin/activate
UV_ACTIVE=1
uv sync --active

if [[ "$STOP_ALL" -eq 1 ]]; then
  STOP_TRAY=1; STOP_BACK=1
fi

if [[ "$STOP_TRAY" -eq 1 ]]; then
  stop_by_name tray "python main.py"
fi
if [[ "$STOP_BACK" -eq 1 ]]; then
  stop_by_name backend "python backend.py"
fi
if [[ "$STOP_TRAY" -eq 1 || "$STOP_BACK" -eq 1 ]]; then
  echo "==> Stopped as requested."
  exit 0
fi

if [[ "$SKIP_MODELS" -eq 0 ]]; then
  echo "==> Pobieram modele (Whisper=${WHISPER_VARIANT})…"
  if [[ -n "${LLM_ID}" ]]; then
    uv run ${UV_ACTIVE:+--active} python scripts/get_models.py --whisper "${WHISPER_VARIANT}" --llm "${LLM_ID}"
  else
    uv run ${UV_ACTIVE:+--active} python scripts/get_models.py --whisper "${WHISPER_VARIANT}"
  fi
else
  echo "==> Pomijam pobieranie modeli (--no-models)"
fi
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

# Persist selected envs into .env jeśli podano flagi
if [[ ${#PERSIST_ENVS[@]} -gt 0 ]]; then
  echo "==> Zapisuję ustawienia do .env: ${PERSIST_ENVS[*]}"
  python - <<PY
from config import update_env_vars
raw = """${PERSIST_ENVS[*]}""".strip().split()
pairs = {}
for item in raw:
    if '=' in item:
        k, v = item.split('=', 1)
        pairs[k] = v
if pairs:
    update_env_vars(pairs)
    print("persisted:", ", ".join(f"{k}={v}" for k, v in pairs.items()))
PY
fi

CMD=(uv run ${UV_ACTIVE:+--active} python)
if [[ "${MODE}" == "backend" ]]; then
  TARGET=backend.py
  echo "Uruchamiam backend (HTTP API) na http://127.0.0.1:8237"
else
  TARGET=main.py
  echo "Uruchamiam aplikację w zasobniku systemowym (tray)"
fi

ENVVARS=(WHISPER_DIR="$WHISPER_DIR" FORMAT_ENABLED="$FORMAT_ENABLED")
[[ -n "$LLM_ID" ]] && ENVVARS+=(LLM_ID="$LLM_ID")

# If mode is both or --with-backend, start backend in background first
if [[ "$MODE" == "both" || "$WITH_BACKEND" -eq 1 ]]; then
  echo "==> Uruchamiam backend w tle (127.0.0.1:8237)"
  nohup env "${ENVVARS[@]}" HOST="127.0.0.1" PORT="8237" \
    uv run ${UV_ACTIVE:+--active} python backend.py >> "$LOG_DIR/backend.out.log" 2>> "$LOG_DIR/backend.err.log" &
  back_pid=$!
  disown "$back_pid" || true
  write_pid backend "$back_pid"
  # krótkie oczekiwanie (bez twardego fail)
  sleep 1
fi

if [[ "$DAEMON" -eq 1 ]]; then
  echo "==> Tryb daemon: log → $LOG_FILE"
  nohup env "${ENVVARS[@]}" "${CMD[@]}" "$TARGET" >> "$LOG_FILE" 2>&1 &
  tray_pid=$!
  disown "$tray_pid" || true
  write_pid tray "$tray_pid"
else
  env "${ENVVARS[@]}" "${CMD[@]}" "$TARGET"
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
