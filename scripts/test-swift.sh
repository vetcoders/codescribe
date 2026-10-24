#!/usr/bin/env bash
# Production test-swift body. Make owns the helper prerequisite and test data
# fixture/trap; arguments preserve Make overrides without another helper owner.
set -uo pipefail
PROFILE="$1"
ENGINE_BRIDGE="$2"
SWIFT_TEST_CODESIGN_IDENTITY="$3"
SWIFT_TEST_MAX_SECONDS="$4"
SWIFT_TEST_MAX_TEST_SECONDS="$5"
SWIFT_TEST_LOG="$6"
shift 6

# Xcode forwards only TEST_RUNNER_ variables into the XCTest host. Keep its
# native config and FFI on Make's disposable test root, never ~/.codescribe.
: "${CODESCRIBE_DATA_DIR:?test-swift requires an isolated test data directory}"
export TEST_RUNNER_CODESCRIBE_DATA_DIR="$CODESCRIBE_DATA_DIR"
export TEST_RUNNER_CODESCRIBE_DISABLE_KEYCHAIN=1
export TEST_RUNNER_CODESCRIBE_TEST_ISOLATED_GUI="${CODESCRIBE_TEST_ISOLATED_GUI:-0}"
# Do not inherit a previous GUI run's mailbox or receipt identity in hermetic mode.
unset TEST_RUNNER_CODESCRIBE_AX_PROBE TEST_RUNNER_CODESCRIBE_INTERACTION_RUN_ID \
  TEST_RUNNER_CODESCRIBE_INTERACTION_SOURCE_SHA TEST_RUNNER_CODESCRIBE_INTERACTION_RECEIPTS
# The host's overlay polls the agent bridge and its Rust core answers the
# channel roster from it. A live ~/.codescribe/agent-bridge drew a second
# overlay with the real conversations during a take (2026-10-13).
mkdir -p "$CODESCRIBE_DATA_DIR/agent-bridge" || exit $?
export TEST_RUNNER_CODESCRIBE_AGENT_BRIDGE_HOME="$CODESCRIBE_DATA_DIR/agent-bridge"

echo "=== Apple phrase-restart Rust/Swift lockstep self-test ==="
"${ENGINE_BRIDGE}" --phrase-restart-self-test || exit $?
# Select the Rust artifact independently from the Swift XCTest configuration.
# The suite requires DEBUG-only fixtures; release optimization must not remove
# those fixtures merely because the selected Rust dylib is optimized.
case "$PROFILE" in
  debug|release|local-release) CONFIG=Debug ;;
  *) echo "test-swift: unsupported profile: $PROFILE" >&2; exit 2 ;;
esac
# Cargo resolves environment and config-relative paths from the repository root.
# No fallback: an old local target must never stand in for the selected library.
if ! TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
value = json.load(sys.stdin)["target_directory"]
if (not isinstance(value, str) or not value.startswith("/")
        or any(ord(c) < 32 or ord(c) == 127 or c in chr(34) + chr(39) + chr(92) + "$`" for c in value)):
    raise SystemExit("invalid Cargo target_directory (expected an absolute usable path)")
print(value)
')"; then
  echo "test-swift: cannot resolve Cargo artifact root via cargo metadata" >&2
  exit 2
fi
TARGET_DIR="${TARGET_ROOT%/}/$PROFILE"
if [ ! -f "$TARGET_DIR/libcodescribe_ffi.dylib" ] || [ ! -r "$TARGET_DIR/libcodescribe_ffi.dylib" ]; then
  echo "test-swift: $TARGET_DIR/libcodescribe_ffi.dylib is missing or unreadable." >&2
  echo "test-swift: run 'make app-bindings' (or 'make app') first; only host artifacts are supported." >&2
  exit 2
fi
if ! command -v xcodegen >/dev/null 2>&1; then
  echo "test-swift: xcodegen is required because the Xcode project is generated, not committed." >&2
  exit 2
fi
echo "=== Regenerating Xcode project from project.yml ==="
( cd macos && xcodegen generate ) || exit $?
if [[ "${CODESCRIBE_TEST_ISOLATED_GUI:-0}" == 1 ]]; then
  # Keep receipts beside the retained runner log, outside Make's disposable root.
  # The AX mailbox/log can contain fixture text; neither is a durable receipt.
  INTERACTION_RUN_ID="$(uuidgen)" || exit 2
  INTERACTION_SOURCE_SHA="$(git rev-parse HEAD)" || exit 2
  INTERACTION_RECEIPTS="$(mktemp "${SWIFT_TEST_LOG}.interactions.jsonl.XXXXXXXX")" || exit 2
  chmod 600 "$INTERACTION_RECEIPTS" || exit 2
  if ! INTERACTION_RECEIPTS="$(python3 - "$INTERACTION_RECEIPTS" "$CODESCRIBE_DATA_DIR" <<'PY'
from pathlib import Path
import sys
receipt, disposable = (Path(value).resolve() for value in sys.argv[1:])
if receipt == disposable or disposable in receipt.parents:
    raise SystemExit("test-swift: receipts must survive disposal of the test data root")
print(receipt)
PY
  )"; then exit 2; fi
  export TEST_RUNNER_CODESCRIBE_INTERACTION_RUN_ID="$INTERACTION_RUN_ID"
  export TEST_RUNNER_CODESCRIBE_INTERACTION_SOURCE_SHA="$INTERACTION_SOURCE_SHA"
  export TEST_RUNNER_CODESCRIBE_INTERACTION_RECEIPTS="$INTERACTION_RECEIPTS"
  echo "test-swift: synthetic DerivedData test-host AX acceptance; no installed-app or HID claim."
  echo "test-swift: private interaction receipts $INTERACTION_RECEIPTS"
  # Keep the independent AX client in its existing TCC identity. Never request trust.
  mkdir -p "$CODESCRIBE_DATA_DIR/ax-probe" || exit $?
  xcrun swift -swift-version 6 -warnings-as-errors scripts/tests/native-ax-probe.swift \
    "$CODESCRIBE_DATA_DIR/ax-probe" > "$CODESCRIBE_DATA_DIR/ax-probe.log" 2>&1 &
  AX_PROBE_PID=$!
  trap 'kill "$AX_PROBE_PID" 2>/dev/null || true; wait "$AX_PROBE_PID" 2>/dev/null || true' EXIT
  export TEST_RUNNER_CODESCRIBE_AX_PROBE="$CODESCRIBE_DATA_DIR/ax-probe"
  AX_PROBE_DEADLINE=$((SECONDS + 15))
  while [[ ! -f "$CODESCRIBE_DATA_DIR/ax-probe/ready" ]]; do
    if ! kill -0 "$AX_PROBE_PID" 2>/dev/null || (( SECONDS >= AX_PROBE_DEADLINE )); then
      echo "test-swift: independent AX client did not start (private transient log withheld)" >&2
      exit 2
    fi
    sleep 0.05
  done
  if [[ "$(cat "$CODESCRIBE_DATA_DIR/ax-probe/ready")" != trusted ]]; then
    echo "test-swift: independent AX client has no accessibility permission" >&2
    exit 2
  fi
fi

echo "=== Swift front-end tests (CodescribeTests) ==="
if [[ "${CODESCRIBE_TEST_ISOLATED_GUI:-0}" != 1 ]]; then
  echo "test-swift: invisible fixtures; native window/AX acceptance is explicitly skipped (isolated GUI opt-in required)."
fi
cd macos || exit $?
# Prefer the selected dylib at runtime while retaining bundled framework lookup.
xcodebuild test \
  -scheme Codescribe \
  -configuration "$CONFIG" \
  ONLY_ACTIVE_ARCH=YES \
  ENABLE_TESTABILITY=YES \
  LIBRARY_SEARCH_PATHS="\"$TARGET_DIR\"" \
  LD_RUNPATH_SEARCH_PATHS="\"$TARGET_DIR\" @executable_path/../Frameworks" \
  -destination 'platform=macOS,arch=arm64' \
  CODE_SIGN_IDENTITY="${SWIFT_TEST_CODESIGN_IDENTITY}" \
  "$@" 2>&1 | tee "${SWIFT_TEST_LOG}" | \
  grep -aE "^Test Case .* (failed|error)|Executed [0-9]+ tests|^\*\* TEST|error:"
rc=${PIPESTATUS[0]}
executed=$(grep -aoE 'Executed [0-9]+ test' "${SWIFT_TEST_LOG}" | tail -1 | grep -oE '[0-9]+')
if [ "$rc" -eq 0 ] && [ "${executed:-0}" -eq 0 ]; then
  echo "test-swift: xcodebuild said TEST SUCCEEDED but executed 0 tests." >&2
  echo "test-swift: a -only-testing filter that matches nothing exits 0 — that is a" >&2
  echo "test-swift: silent pass, not a green gate. Check SWIFT_TEST_ARGS." >&2
  rc=3
fi
if [[ "${CODESCRIBE_TEST_ISOLATED_GUI:-0}" == 1 ]]; then
  # Assertions return Void: only explicit Bool postcondition AND cleanup receipts
  # can certify an action. Ordinary AX queries/AXPress success cannot pass this gate.
  if ! python3 - "$INTERACTION_RECEIPTS" "$INTERACTION_RUN_ID" \
    "$INTERACTION_SOURCE_SHA" "$SWIFT_TEST_LOG" <<'PY'
import json
from pathlib import Path
import re
import sys

def refuse():
    raise SystemExit("test-swift: isolated GUI requires executed, unskipped tests and complete verified action receipts")

path, run_id, source_sha, log_path = sys.argv[1:]
log = Path(log_path).read_bytes().replace(b"\0", b"").decode("utf-8", errors="replace")
# Unknown skip output also fails closed; only an explicit zero-skips summary
# can contain that word. Never let a zero summary hide an individual skip.
for line in log.splitlines():
    if not re.search(r"\bskipped\b", line, re.IGNORECASE):
        continue
    if re.search(r"Test Case .*\bskipped\b", line, re.IGNORECASE):
        refuse()
    counts = re.findall(r"\b([0-9]+) (?:tests? )?(?:were )?skipped\b", line, re.IGNORECASE)
    if not counts or any(int(count) != 0 for count in counts):
        refuse()
keys = {"schema", "runID", "sourceSHA", "requestID", "testMethod", "identifier",
        "verb", "hostKind", "phase", "outcome", "axStatus", "postcondition", "cleanup"}
actions = {}
try:
    for line in Path(path).read_text().splitlines():
        row = json.loads(line)
        if set(row) != keys or row["schema"] != "codescribe.test-host-interaction.v1":
            refuse()
        if row["runID"] != run_id or row["sourceSHA"] != source_sha:
            refuse()
        if (row["hostKind"] != "synthetic-deriveddata-xctest" or row["verb"] != "AXPress"
                or row["identifier"] != "overlay-reply-play-" + "d" * 24
                or row["testMethod"] != "OverlayConversationAcceptanceTests.testVoicelessReplyAndAgentTextRenderEnabledPlay()"
                or not isinstance(row["requestID"], str)
                or not re.fullmatch(r"[0-9A-Fa-f-]{36}", row["requestID"])):
            refuse()
        phases = actions.setdefault(row["requestID"], [])
        phases.append(row)
    if not actions:
        refuse()
    for phases in actions.values():
        if len(phases) != 2:
            refuse()
        start, end = phases
        if (start["phase"] != "started" or start["outcome"] != "UNVERIFIED"
                or start["axStatus"] is not None or start["postcondition"] is not False
                or start["cleanup"] is not False
                or end["phase"] != "completed" or end["outcome"] != "PASS"
                or type(end["axStatus"]) is not int or end["axStatus"] != 0
                or end["postcondition"] is not True or end["cleanup"] is not True):
            refuse()
        case, method = end["testMethod"].removesuffix("()").split(".")
        if not re.search(r"Test Case '-\[CodescribeTests\." + re.escape(case) + r" "
                         + re.escape(method) + r"\]' passed", log):
            refuse()
except (OSError, ValueError, TypeError, KeyError):
    refuse()
print(f"test-swift: verified synthetic test-host AX actions={len(actions)}")
PY
  then
    if [ "$rc" -eq 0 ]; then rc=6; fi
  fi
fi
secs=$(LC_ALL=C tr -d '\000' < "${SWIFT_TEST_LOG}" | sed -nE 's/^.*Executed [0-9]+ tests?,.* in ([0-9.]+) \([0-9.]+\) seconds.*$/\1/p' | tail -1)
test_durations=$(LC_ALL=C tr -d '\000' < "${SWIFT_TEST_LOG}" | sed -nE "s/^.*CodescribeTests\.([A-Za-z0-9_]+) ([A-Za-z0-9_]+)\]' passed \(([0-9.]+) seconds\)\..*$/\3 \1.\2/p")
slowest=$(printf '%s\n' "$test_durations" | sort -rn | head -1)
echo "test-swift: full log ${SWIFT_TEST_LOG} (rc=$rc, executed=${executed:-0}, seconds=${secs:-unknown})"
if [ -n "$slowest" ]; then echo "test-swift: slowest test $slowest"; fi
over_tests=$(printf '%s\n' "$test_durations" | awk -v m="${SWIFT_TEST_MAX_TEST_SECONDS}" 'NF == 2 && $1 > m {print}')
if [ -n "$over_tests" ]; then
  while read -r test_secs test_name; do
    echo "test-swift: test ${test_name} took ${test_secs} s, over the ${SWIFT_TEST_MAX_TEST_SECONDS} s per-test ceiling." >&2
  done <<< "$over_tests"
  if [ "$rc" -eq 0 ]; then rc=5; fi
fi
if [ "$rc" -eq 0 ] && [ -n "$secs" ] && \
   awk -v s="$secs" -v m="${SWIFT_TEST_MAX_SECONDS}" 'BEGIN{exit !(s>m)}'; then
  echo "test-swift: suite took $secs s, over the ${SWIFT_TEST_MAX_SECONDS} s budget." >&2
  echo "test-swift: green-but-slow is the shape this gate exists to catch — a 10x swing" >&2
  echo "test-swift: here has meant the core is doing real (blocking) work for a test run," >&2
  echo "test-swift: not that the machine is busy. Check the slowest test above, then" >&2
  echo "test-swift: core/config/keychain.rs::in_xctest_host and macos/CodescribeTests/README.md." >&2
  echo "test-swift: investigate before choosing an explicit SWIFT_TEST_MAX_SECONDS override." >&2
  rc=4
fi
exit $rc
