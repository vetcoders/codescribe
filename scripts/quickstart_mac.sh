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
#   --daemon [--log FILE]               # run tray/backend in background (default log: CodeScribe.log)
#   --with-backend                      # start backend alongside tray (same as --mode both)
#   --no-models                         # skip model downloader
#   --dev | --verbose                   # enable DEV diagnostics, debug logs; run in foreground
#   --fresh [--yes]                     # stop processes and wipe app caches/settings (keeps .env & models)
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
  echo "[!] Could not find 'uv'. Install it and restart the shell:"
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

# Known ports/patterns we manage (clean stop + guard before each start)
PORT_GUARD_PORTS=(8237 7237 6237 5237)
PROCESS_PATTERNS=(
  "CodeScribeTray"
  "CodeScribeServer"
  "python -m codescribe.main"
  "python -m codescribe.codescribe_server"
)
PRECOMMIT_SENTINEL=".git/hooks/.codescribe-pre-commit.v1"

ensure_precommit_hooks() {
  if git -C . config --get core.hooksPath >/dev/null 2>&1; then
    echo "==> Skipping pre-commit install (core.hooksPath already configured)"
    return
  fi
  if [[ "${SKIP_PRECOMMIT_BOOTSTRAP:-0}" == "1" ]]; then
    return
  fi
  [[ -d .git ]] || return
  if [[ -f "$PRECOMMIT_SENTINEL" && -f .git/hooks/pre-commit ]]; then
    return
  fi
  echo "==> Installing git hooks (pre-commit)"
  mkdir -p .git/hooks
  if uv run ${UV_ACTIVE:+--active} pre-commit install --install-hooks --overwrite; then
    touch "$PRECOMMIT_SENTINEL"
  else
    echo "[!] pre-commit install failed. Run 'uv run pre-commit install --install-hooks' manually." >&2
  fi
}

# Ensure we do NOT inherit a foreign virtualenv (e.g., from old ../vista-scribe)
if [[ -n "${VIRTUAL_ENV:-}" ]]; then
  echo "warning: ignoring inherited VIRTUAL_ENV=$VIRTUAL_ENV; using local .venv instead"
  unset VIRTUAL_ENV
fi

# Preload selected keys from .env (if present) to reuse persisted defaults
if [[ -f .env ]]; then
  while IFS='=' read -r key value; do
    key="${key%%[[:space:]]*}"
    [[ -z "$key" || "${key:0:1}" == "#" ]] && continue
    value="${value%$'\r'}"
    value="${value%%#*}"
    # trim leading/trailing whitespace
    value="${value#${value%%[![:space:]]*}}"
    value="${value%${value##*[![:space:]]}}"
    # strip surrounding double quotes if present
    if [[ "$value" == "\""* && "$value" == *"\"" ]]; then
      value="${value:1:${#value}-2}"
    fi
    case "$key" in
      WHISPER_VARIANT|WHISPER_DIR|FORMAT_ENABLED|MODE|HOLD_MODS|HOLD_EXCLUSIVE|BEEP_ON_START|SOUND_NAME|SOUND_VOLUME)
        export "$key=$value"
        ;;
    esac
  done < .env
fi

# Default values for parameters (use :- for proper default substitution)
WHISPER_VARIANT="${WHISPER_VARIANT:-large-v3-turbo}"
FORMAT_ENABLED="${FORMAT_ENABLED:-1}"
MODE="${MODE:-tray}"
LLM_ID="${LLM_ID:-}"
UV_ACTIVE=0
# Default: run in background with logging (nohup + disown)
DAEMON=1
LOG_FILE="$LOG_DIR/CodeScribe.log"
SKIP_MODELS=0
PERSIST_ENVS=()
WITH_BACKEND=0
STOP_TRAY=0
STOP_BACK=0
STOP_ALL=0
DEV_MODE=0
FRESH=0
FRESH_YES=0
RESET_TCC_SELF=0
FORCE_FOREGROUND_AFTER_FRESH=0

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

kill_patterns() {
  local signal="$1"; shift
  local pattern
  for pattern in "$@"; do
    pkill "-$signal" -f "$pattern" >/dev/null 2>&1 || true
  done
}

collect_running_processes() {
  local seen=""
  local results=()
  local pattern
  for pattern in "${PROCESS_PATTERNS[@]}"; do
    while IFS= read -r pid; do
      [[ -z "$pid" ]] && continue
      case " $seen " in
        *" $pid "*) continue ;;
      esac
      seen+=" $pid"
      local cmd
      cmd=$(ps -p "$pid" -o command= 2>/dev/null | tr -d '\n')
      results+=("${pid}|${cmd:-unknown}")
    done < <(pgrep -f "$pattern" 2>/dev/null || true)
  done
  if [[ ${#results[@]} -gt 0 ]]; then
    printf '%s\n' "${results[@]}" | sort -u
  fi
}

ensure_processes_stopped() {
  stop_by_name tray "python -m codescribe.main"
  stop_by_name server "python -m codescribe.codescribe_server"
  kill_patterns TERM "${PROCESS_PATTERNS[@]}"
  sleep 1
  kill_patterns KILL "${PROCESS_PATTERNS[@]}"
  local leftovers
  leftovers=$(collect_running_processes || true)
  if [[ -n "$leftovers" ]]; then
    echo "[!] Could not stop existing CodeScribe processes:" >&2
    while IFS='|' read -r pid cmd; do
      [[ -z "$pid" ]] && continue
      echo "    pid $pid → ${cmd:-unknown}" >&2
    done <<<"$leftovers"
    echo "[!] Resolve the stuck processes above and rerun the launcher." >&2
    exit 1
  fi
}

listening_pids_for_port() {
  local port="$1"
  if command -v lsof >/dev/null 2>&1; then
    local out
    out=$(lsof -nP -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)
    printf '%s\n' "$out"
  else
    echo ""
  fi
}

port_conflicts_snapshot() {
  local report=""
  local port
  for port in "${PORT_GUARD_PORTS[@]}"; do
    local pids
    pids=$(listening_pids_for_port "$port")
    if [[ -n "$pids" ]]; then
      local pid
      while IFS= read -r pid; do
        [[ -z "$pid" ]] && continue
        local cmd
        cmd=$(ps -p "$pid" -o command= 2>/dev/null | tr -d '\n')
        report+="${port}\t${pid}\t${cmd:-unknown}\n"
      done <<<"$pids"
    fi
  done
  printf '%b' "$report"
}

ensure_ports_available() {
  local needs_backend="$1"
  [[ "$needs_backend" -eq 1 ]] || return 0
  if ! command -v lsof >/dev/null 2>&1; then
    echo "[!] Port guard requires 'lsof'. Install Command Line Tools (xcode-select --install) to enable it." >&2
    return 0
  fi
  local snapshot
  snapshot=$(port_conflicts_snapshot || true)
  if [[ -z "$snapshot" ]]; then
    return 0
  fi
  while IFS=$'\t' read -r port pid cmd; do
    [[ -z "$port" ]] && continue
    if [[ "$cmd" == *"CodeScribe"* || "$cmd" == *"codescribe"* ]]; then
      graceful_kill "$pid" "port-$port"
    fi
  done <<<"$snapshot"
  sleep 1
  snapshot=$(port_conflicts_snapshot || true)
  if [[ -z "$snapshot" ]]; then
    return 0
  fi
  echo "[!] Required CodeScribe ports are already in use:" >&2
  while IFS=$'\t' read -r port pid cmd; do
    [[ -z "$port" ]] && continue
    echo "    port $port ← pid $pid (${cmd:-unknown})" >&2
  done <<<"$snapshot"
  echo "[!] Stop the processes above or choose a different port, then rerun." >&2
  exit 1
}

ensure_clean_start() {
  local needs_backend="$1"
  ensure_processes_stopped
  ensure_ports_available "$needs_backend"
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
    --fg|--foreground|--no-daemon) DAEMON=0; shift;;
    --log) LOG_FILE="$2"; shift 2;;
    --no-models) SKIP_MODELS=1; shift;;
    --with-backend) WITH_BACKEND=1; shift;;
    --stop) STOP_TRAY=1; shift;;
    --stop-backend) STOP_BACK=1; shift;;
    --stop-all) STOP_ALL=1; shift;;
    --dev|--verbose) DEV_MODE=1; DAEMON=0; export LOG_LEVEL=DEBUG; shift;;
    --fresh) FRESH=1; shift;;
    --reset-tcc-self|--reset-access) RESET_TCC_SELF=1; shift;;
    --yes|--force) FRESH_YES=1; shift;;
    --hold-mods) PERSIST_ENVS+=("HOLD_MODS=$2"); shift 2;;
    --hold-exclusive) PERSIST_ENVS+=("HOLD_EXCLUSIVE=$2"); shift 2;;
    --beep) PERSIST_ENVS+=("BEEP_ON_START=$2"); shift 2;;
    --sound-name) PERSIST_ENVS+=("SOUND_NAME=$2"); shift 2;;
    --sound-volume) PERSIST_ENVS+=("SOUND_VOLUME=$2"); shift 2;;
    yes) FRESH_YES=1; shift;;
    no) FRESH_YES=0; shift;;
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

# Optional fresh cleanup (run before building/updating the venv)
if [[ "$FRESH" -eq 1 ]]; then
  APP_SUPP_DIR="$HOME/.CodeScribe"
  LA_PLIST="$HOME/Library/LaunchAgents/com.codescribe.tray.plist"
  APP_LOG="$HOME/Library/Logs/CodeScribe.app.log"
  echo "==> Fresh cleanup plan (keeps .env & models):"
  echo "    - remove .pids/*.pid"
  echo "    - remove logs/*.log"
  echo "    - remove logs/codescribe-server.port"
  echo "    - remove '$APP_SUPP_DIR'"
  echo "    - unload & remove '$LA_PLIST' (if present)"
  echo "    - remove '$APP_LOG' (if present)"
  echo "    - remove Python caches (.venv, .ruff_cache, .mypy_cache, .pytest_cache)"
  echo "    - uv cache prune (global)"
  if [[ "$RESET_TCC_SELF" -eq 1 ]]; then
    bundle="${TCC_BUNDLE_ID:-com.codescribe.app}"
    echo "    - reset TCC (Accessibility/Input/Mic) for bundle: $bundle"
  fi
  if [[ "$FRESH_YES" -ne 1 ]]; then
    read -r -p "Proceed? [y/N] " ans
    case "${ans,,}" in
      y|yes) :;;
      *) echo "Cancelled fresh cleanup."; exit 1;;
    esac
  fi
  if [[ -f .env ]]; then
    backup_dir="$REPO_DIR/.archive/envs"
    mkdir -p "$backup_dir"
    ts=$(date +"%Y%m%d-%H%M%S")
    cp .env "$backup_dir/.env.bak.$ts"
    echo "==> Backed up .env to $backup_dir/.env.bak.$ts"
  fi
  # Stop any lingering processes one more time
  stop_by_name tray "python -m codescribe.main"
  stop_by_name server "python -m codescribe.codescribe_server"
  rm -f .pids/*.pid 2>/dev/null || true
  rm -f logs/*.log 2>/dev/null || true
  rm -f "$LOG_DIR/codescribe-server.port" 2>/dev/null || true
  if [[ -f "$LA_PLIST" ]]; then
    launchctl unload -w "$LA_PLIST" >/dev/null 2>&1 || true
    rm -f "$LA_PLIST" || true
  fi
  rm -rf "$APP_SUPP_DIR" || true
  rm -f "$APP_LOG" || true
  rm -rf .venv .ruff_cache .mypy_cache .pytest_cache .uv_cache 2>/dev/null || true
  uv cache prune --all >/dev/null 2>&1 || true
  if [[ "$RESET_TCC_SELF" -eq 1 ]]; then
    bundle="${TCC_BUNDLE_ID:-com.codescribe.app}"
    echo "==> Resetting macOS permissions for $bundle…"
    /usr/bin/tccutil reset Accessibility "$bundle" || true
    /usr/bin/tccutil reset ListenEvent "$bundle" || true
    /usr/bin/tccutil reset Microphone "$bundle" || true
    /usr/bin/tccutil reset AppleEvents "$bundle" || true
  fi
  echo "==> Fresh cleanup done."
  FORCE_FOREGROUND_AFTER_FRESH=1
fi

echo "==> Syncing environment (uv sync)…"
# Ensure local .venv exists and is used
if [[ ! -d .venv ]]; then
  uv venv .venv
elif [[ ! -x .venv/bin/python ]]; then
  uv venv .venv --upgrade
fi
# Activate the venv for this script's process and force uv to use it
source .venv/bin/activate
UV_ACTIVE=1
uv sync --active
ensure_precommit_hooks

echo "==> Python: $(python -c 'import sys; print(sys.executable)')"
echo "==> VIRTUAL_ENV=${VIRTUAL_ENV:-}"
if [[ "$DEV_MODE" -eq 1 ]]; then
  export DEV_MODE=1
  echo "==> DEV diagnostics enabled (LOG_LEVEL=${LOG_LEVEL:-INFO})"
fi

if [[ "$STOP_ALL" -eq 1 ]]; then
  STOP_TRAY=1; STOP_BACK=1
fi

if [[ "$STOP_TRAY" -eq 1 ]]; then
  stop_by_name tray "python -m codescribe.main"
fi
if [[ "$STOP_BACK" -eq 1 ]]; then
  stop_by_name server "python -m codescribe.codescribe_server"
fi
if [[ "$STOP_TRAY" -eq 1 || "$STOP_BACK" -eq 1 ]]; then
  echo "==> Stopped as requested."
  exit 0
fi

if [[ "$FORCE_FOREGROUND_AFTER_FRESH" -eq 1 ]]; then
  echo "==> Fresh run detected; launching in foreground so onboarding can appear"
  DAEMON=0
fi

if [[ "$SKIP_MODELS" -eq 0 ]]; then
  echo "==> Downloading models (Whisper=${WHISPER_VARIANT})…"
  set +e
  if [[ -n "${LLM_ID}" ]]; then
    uv run ${UV_ACTIVE:+--active} python scripts/get_models.py --whisper "${WHISPER_VARIANT}" --llm "${LLM_ID}"
  else
    uv run ${UV_ACTIVE:+--active} python scripts/get_models.py --whisper "${WHISPER_VARIANT}"
  fi
  rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    echo "[!] Download failed (rc=$rc). Continuing without local models."
    echo "    Hint: set HF_TOKEN and use tray menu → Models → Download later."
  fi
else
  echo "==> Skipping model download (--no-models)"
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
      echo "==> Found local LLM model: $LLM_ID"
    fi
  fi

# Resolve WHISPER_DIR (respect existing value from environment/.env)
MODEL_DIR="$REPO_DIR/models"
if [[ -z "${WHISPER_DIR:-}" ]]; then
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
fi
WHISPER_DIR="$(lower_users "$WHISPER_DIR")"

echo "==> Launching application mode (${MODE})…"

# Persist selected envs into .env when flags are provided
if [[ ${#PERSIST_ENVS[@]} -gt 0 ]]; then
  echo "==> Persisting settings to .env: ${PERSIST_ENVS[*]}"
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

CMD=(uv run ${UV_ACTIVE:+--active} python -m codescribe.main)
SERVER_OUT="$LOG_DIR/codescribe-server.out.log"
SERVER_ERR="$LOG_DIR/codescribe-server.err.log"

ENVVARS=(WHISPER_DIR="$WHISPER_DIR" FORMAT_ENABLED="$FORMAT_ENABLED")
if [[ "$DAEMON" -eq 1 ]]; then
  ENVVARS+=(NOHUP_MODE="1")
fi
[[ -n "$LLM_ID" ]] && ENVVARS+=(LLM_ID="$LLM_ID")

NEEDS_BACKEND=0
if [[ "$MODE" == "both" || "$MODE" == "backend" || "$WITH_BACKEND" -eq 1 ]]; then
  NEEDS_BACKEND=1
fi

ensure_clean_start "$NEEDS_BACKEND"

if [[ "${MODE}" == "backend" ]]; then
  echo "Starting CodeScribeServer (HTTP API)"
  if [[ "$DAEMON" -eq 1 ]]; then
    echo "==> Daemon mode: log → $SERVER_OUT"
    nohup env "${ENVVARS[@]}" \
      uv run ${UV_ACTIVE:+--active} python -m codescribe.codescribe_server start >> "$SERVER_OUT" 2>> "$SERVER_ERR" &
    server_pid=$!
    disown "$server_pid" || true
    write_pid server "$server_pid"
    echo "CodeScribeServer pid: $server_pid (log: $SERVER_OUT)"
    sleep 1
    if [[ -f "$LOG_DIR/codescribe-server.port" ]]; then
      port_msg=$(cat "$LOG_DIR/codescribe-server.port")
      echo "→ Listening on port: $port_msg"
    else
      echo "[!] Server did not write the port file; check log: $SERVER_OUT"
    fi
    echo "Use 'uv run python -m codescribe.codescribe_server status' to inspect the state."
  else
    env "${ENVVARS[@]}" \
      uv run ${UV_ACTIVE:+--active} python -m codescribe.codescribe_server start
  fi
  exit 0
fi

echo "Starting the tray application"

# If mode is both or --with-backend, start backend in background first
if [[ "$MODE" == "both" || "$WITH_BACKEND" -eq 1 ]]; then
  echo "==> Launching CodeScribeServer in the background"
  nohup env "${ENVVARS[@]}" \
    uv run ${UV_ACTIVE:+--active} python -m codescribe.codescribe_server start >> "$SERVER_OUT" 2>> "$SERVER_ERR" &
  back_pid=$!
  disown "$back_pid" || true
  write_pid server "$back_pid"
  echo "CodeScribeServer pid: $back_pid (log: $SERVER_OUT)"
  sleep 1
  if [[ -f "$LOG_DIR/codescribe-server.port" ]]; then
    port_msg=$(cat "$LOG_DIR/codescribe-server.port")
    echo "→ Listening on port: $port_msg"
  else
    echo "[!] Server did not write the port file; check log: $SERVER_OUT"
  fi
fi

if [[ "$DAEMON" -eq 1 ]]; then
  echo "==> Daemon mode: log → $LOG_FILE"
  nohup env "${ENVVARS[@]}" "${CMD[@]}" >> "$LOG_FILE" 2>&1 &
  tray_pid=$!
  disown "$tray_pid" || true
  write_pid tray "$tray_pid"
  echo "tray pid: $tray_pid (log: $LOG_FILE)"
  echo "tray pid: $tray_pid" >> "$LOG_FILE" || true
else
  env "${ENVVARS[@]}" "${CMD[@]}"
fi

# Notes for the user (reached after app exits)
cat <<'EOF'

Done.
- If nothing records/pastes: System Settings → Privacy & Security → allow Microphone,
  Accessibility, and Input Monitoring for your shell/Python.
- Hotkeys (configured in the tray menu):
  • Hold (Ctrl by default) – records while pressed
  • Hold + Shift – assistive AI mode (El Niño)
  • Double-Option (⌥⌥) – hands-off toggle start/stop
- Formatting: Light Plus always runs; AI (Ollama/Harmony) is optional.
- When MLX complains about `/Users/...`, try the lowercase `/users/...` path instead.

EOF
