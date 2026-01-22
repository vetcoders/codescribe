#!/usr/bin/env bash
set -euo pipefail

# =========================
# CodeScribe Repo Runner v0.1.0-dev
# - per-repo watermark in ./.codescribe/
# - generates loctree context + builds prompt + runs Claude
# =========================

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

CODESCRIBE_DIR="$ROOT/.codescribe"
mkdir -p "$CODESCRIBE_DIR"/{tools,config,context,agent,logs,tasks,reports,roadmaps,state}

PROJECT="${PROJECT:-$(basename "$ROOT")}"
AGENT="${AGENT:-claude}"
RUN_ID="${RUN_ID:-$(date +%Y%m%d_%H%M%S)}"
MODEL="${MODEL:-sonnet}"
MAX_LINES="${MAX_LINES:-1200}"
HORIZON="${HORIZON:-4800}"
EXPORT_AGENTS="${EXPORT_AGENTS:-1}"

LOG="$CODESCRIBE_DIR/logs/${RUN_ID}.log"
REPORT="$CODESCRIBE_DIR/reports/${RUN_ID}_report.md"
PROMPT_FILE="$CODESCRIBE_DIR/tasks/${RUN_ID}_prompt.md"
LOCT_FILE="$CODESCRIBE_DIR/context/${RUN_ID}_loct_for_ai.md"
META_FILE="$CODESCRIBE_DIR/meta.json"

# Truncate log on start and tee everything
: > "$LOG"
exec > >(tee -a "$LOG") 2>&1

json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

append_gitignore() {
  if [ -f .gitignore ]; then
    if command_exists rg; then
      rg -q '^\.codescribe/state/?$' .gitignore 2>/dev/null || {
        printf '\n# CodeScribe\n.codescribe/state/\n' >> .gitignore
      }
    else
      grep -q '^\.codescribe/state/?$' .gitignore 2>/dev/null || {
        printf '\n# CodeScribe\n.codescribe/state/\n' >> .gitignore
      }
    fi
  else
    printf '# CodeScribe\n.codescribe/state/\n' > .gitignore
  fi
}

resolve_ai_contexters() {
  AI_CONTEXT_CMD=()
  if command_exists ai-contexters; then
    AI_CONTEXT_CMD=(ai-contexters)
    return
  fi

  local base="${AI_CONTEXT_PATH:-$HOME/hosted/VetCoders/ai-contexters}"
  if [ -x "$base/target/release/ai-contexters" ]; then
    AI_CONTEXT_CMD=("$base/target/release/ai-contexters")
    return
  fi
  if [ -x "$base/target/debug/ai-contexters" ]; then
    AI_CONTEXT_CMD=("$base/target/debug/ai-contexters")
    return
  fi
  if [ -f "$base/Cargo.toml" ]; then
    AI_CONTEXT_CMD=(cargo run --quiet --manifest-path "$base/Cargo.toml" --)
    return
  fi
}

append_gitignore
resolve_ai_contexters

GIT_SHA="$(git rev-parse HEAD 2>/dev/null || echo "")"
GIT_DIRTY="false"
if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
  GIT_DIRTY="true"
fi

LOCT_VERSION=""
if command_exists loct; then
  LOCT_VERSION="$(loct --version 2>/dev/null || true)"
fi

CODESCRIBE_VERSION=""
if command_exists codescribe; then
  CODESCRIBE_VERSION="$(codescribe --version 2>/dev/null || true)"
fi

AI_CONTEXT_VERSION=""
if [ ${#AI_CONTEXT_CMD[@]} -gt 0 ]; then
  AI_CONTEXT_VERSION="$(${AI_CONTEXT_CMD[@]} --version 2>/dev/null || true)"
fi

cat > "$META_FILE" <<EOF_META
{
  "project": "$(json_escape "$PROJECT")",
  "root": "$(json_escape "$ROOT")",
  "run_id": "$(json_escape "$RUN_ID")",
  "created_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "git_sha": "$(json_escape "$GIT_SHA")",
  "git_dirty": $GIT_DIRTY,
  "tool_versions": {
    "loct": "$(json_escape "$LOCT_VERSION")",
    "ai_contexters": "$(json_escape "$AI_CONTEXT_VERSION")",
    "codescribe": "$(json_escape "$CODESCRIBE_VERSION")"
  }
}
EOF_META

echo "== codescribe runner =="
echo "root:    $ROOT"
echo "project: $PROJECT"
echo "agent:   $AGENT"
echo "run_id:  $RUN_ID"
echo "model:   $MODEL"
echo "log:     $LOG"
echo "meta:    $META_FILE"
echo

if ! command_exists loct; then
  echo "[FATAL] loct not found in PATH"
  exit 2
fi

if [ "$AGENT" != "claude" ] && [ "$AGENT" != "codex" ]; then
  echo "[FATAL] AGENT must be: claude|codex"
  exit 2
fi

echo "== Step 0/4: loct auto =="
loct auto
echo

echo "== Step 1/4: loct --for-ai -> $LOCT_FILE =="
loct --for-ai | tee "$LOCT_FILE" >/dev/null
echo

echo "== Step 2/4: ai-contexters (optional) =="
if [ ${#AI_CONTEXT_CMD[@]} -gt 0 ]; then
  (cd "$CODESCRIBE_DIR/context" && "${AI_CONTEXT_CMD[@]}" all -p "$PROJECT" -H "$HORIZON") || true
else
  echo "[WARN] ai-contexters not found; skipping"
fi
echo

echo "== Step 3/4: build prompt -> $PROMPT_FILE =="
cat > "$PROMPT_FILE" <<EOF
Jesteś agentem, który ma dokończyć pipeline dla dowolnego repozytorium.

Zasady:
- Nie masz internetu.
- Nie masz dostępu do repozytorium ani plików poza tym, co widzisz w treści promptu.
- Nie zgaduj faktów technicznych, jeśli ich nie ma w kontekście.

Zadanie:
1) Summary: czym jest repo i jak jest zorganizowane.
2) Build/Test Quickstart: konkretne komendy (albo jasno: czego brakuje).
3) Next Tasks: 5–10 kroków (checklista [ ]) – małe, domykalne.
4) Risks/Tech debt: max 10 + minimalne fixy.

Wymagany format odpowiedzi:
- Summary
- Build/Test Quickstart
- Next Tasks
- Risks

KONTEKST (skrócony):
EOF

{
  echo
  echo "## loct --for-ai (first $MAX_LINES lines)"
  sed -n "1,${MAX_LINES}p" "$LOCT_FILE"
} >> "$PROMPT_FILE"

shopt -s nullglob
for f in "$CODESCRIBE_DIR/context"/*memory_*.md "$CODESCRIBE_DIR/context"/*timeline*.md; do
  [ -f "$f" ] || continue
  {
    echo
    echo "## $(basename "$f") (first $MAX_LINES lines)"
    sed -n "1,${MAX_LINES}p" "$f"
  } >> "$PROMPT_FILE"
done
shopt -u nullglob

PROMPT_BYTES="$(wc -c < "$PROMPT_FILE" | tr -d ' ')"
echo "[debug] prompt size: ${PROMPT_BYTES} bytes"
if [ "$PROMPT_BYTES" -gt 180000 ]; then
  echo "[WARN] prompt is big (${PROMPT_BYTES}B). If Claude fails, lower MAX_LINES (e.g., 600)."
fi

echo
echo "== Step 4/4: agent $AGENT -> $REPORT =="

: > "$REPORT"

run_claude() {
  command_exists claude || { echo "[FATAL] claude not found in PATH"; exit 2; }

  # strict-mcp-config requires explicit mcp.json
  mkdir -p "$ROOT/.claude"
  if [ ! -f "$ROOT/.claude/mcp.json" ]; then
    cat > "$ROOT/.claude/mcp.json" <<'JSON'
{
  "mcpServers": {}
}
JSON
  fi

  claude -p \
    --no-session-persistence \
    --mcp-config "$ROOT/.claude/mcp.json" \
    --strict-mcp-config \
    --tools "" \
    --output-format text \
    --model "$MODEL" \
    "$(cat "$PROMPT_FILE")" \
  | tee -a "$REPORT"
}

run_codex() {
  command_exists codex || { echo "[FATAL] codex not found in PATH"; exit 2; }

  codex exec \
    -m "$MODEL" \
    --sandbox read-only \
    -C "$ROOT" \
    --output-last-message "$REPORT" \
    - < "$PROMPT_FILE" || true

  if [ ! -s "$REPORT" ]; then
    echo "[WARN] codex did not write report (see log: $LOG)" | tee -a "$REPORT"
  fi
}

case "$AGENT" in
  claude) run_claude ;;
  codex)  run_codex ;;
esac

echo
echo "== DONE =="
echo "[report] $REPORT"
echo "[prompt] $PROMPT_FILE"
echo "[loct]   $LOCT_FILE"
echo "[log]    $LOG"

if [ "$EXPORT_AGENTS" != "0" ]; then
  mkdir -p "$ROOT/agents/context" "$ROOT/agents/reports" "$ROOT/agents/tasks"
  cp "$REPORT" "$ROOT/agents/reports/${PROJECT}_${RUN_ID}_report.md" 2>/dev/null || true
  cp "$PROMPT_FILE" "$ROOT/agents/tasks/${PROJECT}_${RUN_ID}_prompt.md" 2>/dev/null || true
  cp "$LOCT_FILE" "$ROOT/agents/context/${PROJECT}_${RUN_ID}_loct_for_ai.md" 2>/dev/null || true
  echo "[export] agents/* updated"
fi
