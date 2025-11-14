#!/bin/bash
#!/bin/bash
# Start test instance of VistaScribe on port 7237 with refactored code

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "🚀 Starting TEST instance on port 7237..."
echo "   Production instance on 8237 remains untouched"

# Load local overrides if a developer has a .env; otherwise fall back to the template.
load_env_file() {
  local file="$1"
  if [ -f "$file" ]; then
    set -a
    # shellcheck disable=SC1090
    source "$file"
    set +a
  fi
}

load_env_file ".env.example"
load_env_file ".env"

# Deterministic test overrides (mirrors the old .env.test but lives inline now)
export VISTASCRIBE_INSTANCE=test
export INSTANCE_LOCK_FILE=.vista_scribe_test.lock
export DEV_MODE=1
export NOHUP_MODE=1
export LOG_LEVEL=DEBUG
export MODE=hands_off
export HOLD_MODS=ctrl
export HOLD_EXCLUSIVE=1
export HOLD_START_DELAY_MS=800
export HOLD_STREAMING=1
export TOGGLE_TRIGGER=double_option
export HISTORY_ENABLED=1
export WHISPER_LANGUAGE=pl
export AGENT_NAME="El Niño TEST"
export OLLAMA_MODEL=${OLLAMA_MODEL:-qwen3-coder:30b}
export VISTASCRIBE_PORT_FALLBACKS=${VISTASCRIBE_PORT_FALLBACKS:-"7237,6237,5237"}

# Start backend on port 7237
echo "Starting backend on port 7237..."
nohup uv run python -m vistascribe.vistascribe_server > logs/test-server.log 2>&1 &
echo "Backend PID: $!"
sleep 2

# Start the tray using the packaged module path
echo "Starting tray (vistascribe.main) with test config..."
nohup uv run python -m vistascribe.main > logs/test-main.log 2>&1 &
echo "Main PID: $!"

echo ""
echo "✅ Test instance started!"
echo "   Logs: logs/test-*.log"
echo "   Port: 7237"
echo "   Production on 8237 still running"
echo ""
echo "To stop test instance:"
echo "   pkill -f 'VISTASCRIBE_INSTANCE=test'"
