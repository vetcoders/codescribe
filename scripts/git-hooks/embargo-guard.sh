#!/bin/sh
# Run one pre-commit command unless a valid repository-owned marker defers it.
set -eu

usage() {
  echo "usage: embargo-guard.sh <gate-id> -- <command> [args ...]" >&2
  exit 2
}

[ "$#" -ge 3 ] || usage
gate=$1
shift
[ "$1" = "--" ] || usage
shift
[ "$#" -gt 0 ] || usage

repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo "compile-embargo: not inside a Git repository" >&2
  exit 2
}
marker="$repo_root/.vibecrafted/embargo.toml"

# No marker is the ordinary repository path: run the original gate unchanged.
[ -f "$marker" ] || exec "$@"

set +e
decision=$(
  python3 -I - "$marker" "$gate" <<'PY'
import re
import sys
import tomllib

marker_path, gate = sys.argv[1:]

try:
    with open(marker_path, "rb") as handle:
        marker = tomllib.load(handle)
except (OSError, tomllib.TOMLDecodeError) as exc:
    print(f"compile-embargo: invalid marker: {exc}", file=sys.stderr)
    raise SystemExit(2)

required = {
    "plan_id": str,
    "phase": str,
    "deferred_gates": list,
    "attestation": str,
    "recovery_ref": str,
}
errors = []

unknown = sorted(set(marker) - set(required))
if unknown:
    errors.append("unknown fields: " + ", ".join(unknown))

for key, expected_type in required.items():
    value = marker.get(key)
    if type(value) is not expected_type:
        errors.append(f"{key} must be {expected_type.__name__}")

if errors:
    print("compile-embargo: invalid marker: " + "; ".join(errors), file=sys.stderr)
    raise SystemExit(2)

plan_id = marker["plan_id"]
phase = marker["phase"]
deferred = marker["deferred_gates"]
attestation = marker["attestation"]
recovery_ref = marker["recovery_ref"]

if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", plan_id):
    errors.append("plan_id must be a lowercase slug")
if phase not in {"W1", "W2"}:
    errors.append("phase must be W1 or W2")

allowed_deferred = {"cargo-check", "cargo-fmt", "cargo-clippy", "prettier"}
if any(type(item) is not str for item in deferred):
    errors.append("deferred_gates entries must be strings")
elif len(deferred) != len(set(deferred)):
    errors.append("deferred_gates must not contain duplicates")
elif not deferred:
    errors.append("deferred_gates must not be empty")
elif not set(deferred) <= allowed_deferred:
    invalid = sorted(set(deferred) - allowed_deferred)
    errors.append("unsupported deferred_gates: " + ", ".join(invalid))

expected_ref = f"embargo/{plan_id}"
if recovery_ref != expected_ref:
    errors.append(f"recovery_ref must be {expected_ref}")

if attestation not in {"open", "W2_STRUCTURALLY_CLOSED"}:
    errors.append("attestation must be open or W2_STRUCTURALLY_CLOSED")
elif attestation == "W2_STRUCTURALLY_CLOSED" and phase != "W2":
    errors.append("W2_STRUCTURALLY_CLOSED requires phase W2")

if errors:
    print("compile-embargo: invalid marker: " + "; ".join(errors), file=sys.stderr)
    raise SystemExit(2)

if attestation != "open":
    raise SystemExit(10)

skip_value = ",".join(deferred)
action = "skip" if gate in deferred else "run"
print(f"{phase}|{skip_value}|{action}")
PY
)
status=$?
set -e

case "$status" in
  0) ;;
  10) exec "$@" ;;
  2) exit 2 ;;
  *)
    echo "compile-embargo: validator failed unexpectedly (status $status)" >&2
    exit 2
    ;;
esac

phase=${decision%%|*}
remainder=${decision#*|}
skip_value=${remainder%%|*}
action=${remainder##*|}

# SKIP is deliberately local to this wrapper. A child hook cannot mutate the
# parent pre-commit process, so each deferrable gate is wrapped independently.
SKIP=$skip_value
export SKIP

if [ "$action" = "skip" ]; then
  echo "compile-embargo: phase=$phase SKIP=$SKIP gate=$gate deferred" >&2
  exit 0
fi

echo "compile-embargo: phase=$phase SKIP=$SKIP gate=$gate hard" >&2
exec "$@"
