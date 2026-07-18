#!/bin/sh
# Enforce VetCoders provenance tags on every local commit message.
#
# Accepted first lines:
#   [claude/vc-workflow] fix: example
#   [codex/vc-ownership] release: example
#   [maciej/vc-manual] docs: example
#   [monika/vc-manual] chore: example
#   [ok-commit] fix: example
#   Merge branch 'feature' into develop
#   Squashed commit of the following:

set -eu

MSG_FILE="${1:-}"
if [ -z "$MSG_FILE" ] || [ ! -f "$MSG_FILE" ]; then
  echo "commit-msg provenance hook: missing commit message file" >&2
  exit 1
fi

first_line=$(
  sed -n '
    /^[[:space:]]*#/d
    /^[[:space:]]*$/d
    p
    q
  ' "$MSG_FILE"
)

agent_pattern='(claude|codex|gemini|maciej|monika)'
workflow_pattern='vc-[a-z0-9][a-z0-9-]*'
agent_commit_pattern="^\\[${agent_pattern}/${workflow_pattern}\\] .+"
human_commit_pattern='^\[ok-commit\] .+'
merge_commit_pattern='^Merge .+'
squash_commit_pattern='^Squashed commit of the following:.*'

if printf '%s\n' "$first_line" | grep -Eq "$agent_commit_pattern|$human_commit_pattern|$merge_commit_pattern|$squash_commit_pattern"; then
  exit 0
fi

echo "✋ Commit blocked: add provenance tag to the commit message." >&2
echo "" >&2
echo "  Agent telemetry:  [claude/vc-marbles] fix: overlay crash" >&2
echo "  Agent telemetry:  [codex/vc-ownership] release: embed models by default" >&2
echo "  Human commit:     [monika/vc-manual] chore: normalize docs" >&2
echo "  Human quick:      [ok-commit] fix: overlay crash" >&2
echo "  Merge commit:     Merge branch 'feature' into develop" >&2
echo "  Squash commit:    Squashed commit of the following:" >&2
echo "" >&2
echo "  Format: [<agent>/vc-<workflow>] <description>" >&2
echo "  Agents: claude, codex, gemini, maciej, monika" >&2
echo "  Workflows: any vc-* workflow, e.g. vc-marbles, vc-justdo, vc-workflow, vc-ownership, vc-manual" >&2
echo "" >&2
echo "  Current first line: ${first_line:-<empty>}" >&2
exit 1
