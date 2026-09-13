#!/usr/bin/env bash
# Production test-swift body. Make owns the helper prerequisite and test data
# fixture/trap; arguments preserve Make overrides without another helper owner.
set -uo pipefail
PROFILE="$1"
ENGINE_BRIDGE="$2"
SWIFT_TEST_CODESIGN_IDENTITY="$3"
SWIFT_TEST_MAX_SECONDS="$4"
SWIFT_TEST_LOG="$5"
shift 5

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
echo "=== Swift front-end tests (CodescribeTests) ==="
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
  grep -E "^Test Case .* (failed|error)|Executed [0-9]+ tests|^\*\* TEST|error:"
rc=${PIPESTATUS[0]}
executed=$(grep -oE 'Executed [0-9]+ test' "${SWIFT_TEST_LOG}" | tail -1 | grep -oE '[0-9]+')
if [ "$rc" -eq 0 ] && [ "${executed:-0}" -eq 0 ]; then
  echo "test-swift: xcodebuild said TEST SUCCEEDED but executed 0 tests." >&2
  echo "test-swift: a -only-testing filter that matches nothing exits 0 — that is a" >&2
  echo "test-swift: silent pass, not a green gate. Check SWIFT_TEST_ARGS." >&2
  rc=3
fi
secs=$(sed -nE 's/^.*Executed [0-9]+ tests?,.* in ([0-9.]+) \([0-9.]+\) seconds.*$/\1/p' "${SWIFT_TEST_LOG}" | tail -1)
slowest=$(sed -nE "s/^.*CodescribeTests\.([A-Za-z0-9_]+) ([A-Za-z0-9_]+)\]' passed \(([0-9.]+) seconds\)\..*$/\3 \1.\2/p" "${SWIFT_TEST_LOG}" | sort -rn | head -1)
echo "test-swift: full log ${SWIFT_TEST_LOG} (rc=$rc, executed=${executed:-0}, seconds=${secs:-unknown})"
if [ -n "$slowest" ]; then echo "test-swift: slowest test $slowest"; fi
if [ "$rc" -eq 0 ] && [ -n "$secs" ] && \
   awk -v s="$secs" -v m="${SWIFT_TEST_MAX_SECONDS}" 'BEGIN{exit !(s>m)}'; then
  echo "test-swift: suite took $secs s, over the ${SWIFT_TEST_MAX_SECONDS} s budget." >&2
  echo "test-swift: green-but-slow is the shape this gate exists to catch — a 10x swing" >&2
  echo "test-swift: here has meant the core is doing real (blocking) work for a test run," >&2
  echo "test-swift: not that the machine is busy. Check the slowest test above, then" >&2
  echo "test-swift: core/config/keychain.rs::in_xctest_host and macos/CodescribeTests/README.md." >&2
  echo "test-swift: if the host really is loaded: make test-swift SWIFT_TEST_MAX_SECONDS=90" >&2
  rc=4
fi
exit $rc
