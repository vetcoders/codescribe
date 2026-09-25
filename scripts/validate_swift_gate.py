#!/usr/bin/env python3
"""Bounded Make/helper source contract; candidate paths stay relative to caller cwd."""
from pathlib import Path
import re
import sys


def refuse(code, detail):
    print("  ✗ test-swift [" + code + "]: " + detail, file=sys.stderr)
    raise SystemExit(1)


def production(source, label):
    """Read one unconditionally defined Make production, without expansion."""
    lines = source.splitlines()
    found = []
    defines = conditions = 0
    continued = False
    for i, line in enumerate(lines):
        was_continued, continued = continued, line.endswith("\\")
        if line.startswith("\t") or not line.strip() or line.lstrip().startswith("#"):
            continue
        line = line.strip()
        if re.match(r"(?:-?include|sinclude)\s", line) or "$(eval" in line or "${eval" in line:
            refuse("make-shape", "includes/eval are outside the supported Make shape")
        if re.match(r"(?:override\s+|export\s+)?(?:\.RECIPEPREFIX|\.SHELLFLAGS)\s*[:?+!]?=", line) or line.startswith((".ONESHELL:", ".IGNORE:")):
            refuse("make-shape", "alternate recipe execution is unsupported")
        if re.match(r"(?:(?:override|export)\s+)*define\s+.*\$", line):
            refuse("make-shape", "dynamic Make definitions are unsupported")
        assignment = re.match(r"([^=]*?)[?:+!]?=", line)
        if assignment and not defines and not was_continued and "$" in assignment.group(1):
            refuse("make-shape", "dynamic Make assignment names are unsupported")
        if line == label:
            if defines or conditions or was_continued:
                refuse("make-shape", "conditional/nested " + label)
            found.append(i)
        if re.match(r"(?:(?:override|export)\s+)*define\s", line):
            defines += 1
        elif line == "endef":
            defines -= 1
        elif re.match(r"(?:ifeq|ifneq|ifdef|ifndef)\b", line):
            conditions += 1
        elif line == "endif":
            conditions -= 1
    if len(found) != 1:
        refuse("make-shape", "expected exactly one " + label)
    return lines, found[0]


def read_source(path):
    # Universal-newline/splitlines normalization could turn invalid shell bytes
    # into an accepted production (notably CRLF, vertical tab and form feed).
    text = path.read_bytes().decode("utf-8")
    if any((ord(c) < 32 and c not in "\n\t") or c in "\x7f\x85\u2028\u2029" for c in text):
        refuse("source-bytes", "unsupported control/line-ending bytes in " + str(path))
    return text


make = read_source(Path(sys.argv[1]))
# A rule may not be borrowed from a define or disabled Make conditional.
lines, start = production(make, "test-swift: $(ENGINE_BRIDGE)")
headers = [line for line in lines if not line.startswith("\t")
           and re.match(r"[^#:=]*\btest-swift\s*(?:[^:=]*):", line)
           and not line.startswith(".PHONY:")]
if headers != ["test-swift: $(ENGINE_BRIDGE)"]:
    refuse("bridge-prerequisite", "expected one canonical Apple STT bridge prerequisite rule")
end = start + 1
while end < len(lines) and (lines[end].startswith("\t") or not lines[end].strip() or lines[end].startswith("#")):
    end += 1
# Trailing prose is not a recipe, but comments inside a continued recipe ARE
# significant. Stop only after its final noncontinued command.
recipe_lines = lines[start:start + 1]
for line in lines[start + 1:end]:
    if not line.startswith("\t"):
        if recipe_lines[-1].endswith("\\"):
            refuse("invocation", "comment/blank interrupts the helper invocation")
        continue
    recipe_lines.append(line)
expected_recipe = r'''test-swift: $(ENGINE_BRIDGE)
	@$(TEST_DATA_DIR_SETUP); \
	$(SHELL) scripts/test-swift.sh "$(PROFILE)" "$(ENGINE_BRIDGE)" \
	  "$(SWIFT_TEST_CODESIGN_IDENTITY)" "$(SWIFT_TEST_MAX_SECONDS)" \
	  "$(SWIFT_TEST_MAX_TEST_SECONDS)" \
	  "$(SWIFT_TEST_LOG)" $(SWIFT_TEST_ARGS)'''
if recipe_lines != expected_recipe.splitlines():
    refuse("invocation", "require data-directory setup before the connected scripts/test-swift.sh invocation with canonical bridge argument 2")

# The connected setup cannot be shadowed or replaced by token-bearing dead code.
setup_lines, setup_start = production(make, "define TEST_DATA_DIR_SETUP")
expected_setup = r'''define TEST_DATA_DIR_SETUP
CODESCRIBE_TEST_TMP_ROOT="$${TMPDIR:-/tmp}"; \
CODESCRIBE_TEST_TMP_ROOT="$${CODESCRIBE_TEST_TMP_ROOT%/}"; \
if [[ -z "$$CODESCRIBE_TEST_TMP_ROOT" ]]; then CODESCRIBE_TEST_TMP_ROOT=/tmp; fi; \
CODESCRIBE_TEST_DATA_DIR="$$(mktemp -d "$$CODESCRIBE_TEST_TMP_ROOT/codescribe-test-data.XXXXXX")" || { \
  echo "test-data-dir: mktemp failed under $$CODESCRIBE_TEST_TMP_ROOT" >&2; \
  exit 1; \
}; \
export CODESCRIBE_DATA_DIR="$$CODESCRIBE_TEST_DATA_DIR"; \
cleanup_codescribe_test_data_dir() { \
  isolated_log="$$CODESCRIBE_TEST_DATA_DIR/logs/codescribe.log"; \
  if [[ -f "$$isolated_log" ]]; then \
    isolated_bytes="$$(wc -c < "$$isolated_log" | tr -d ' ')"; \
    echo "test-data-dir: isolated-log=$$isolated_log bytes=$$isolated_bytes"; \
  else \
    echo "test-data-dir: isolated-log=none root=$$CODESCRIBE_TEST_DATA_DIR"; \
  fi; \
  case "$$CODESCRIBE_TEST_DATA_DIR" in \
    "$$CODESCRIBE_TEST_TMP_ROOT"/codescribe-test-data.*) \
      rm -rf -- "$$CODESCRIBE_TEST_DATA_DIR"; \
      echo "test-data-dir: cleaned=$$CODESCRIBE_TEST_DATA_DIR"; \
      ;; \
    *) \
      echo "test-data-dir: refusing unsafe cleanup: $$CODESCRIBE_TEST_DATA_DIR" >&2; \
      return 1; \
      ;; \
  esac; \
}; \
trap cleanup_codescribe_test_data_dir EXIT; \
echo "test-data-dir: created=$$CODESCRIBE_TEST_DATA_DIR"
endef'''
if setup_lines[setup_start:setup_start + len(expected_setup.splitlines())] != expected_setup.splitlines():
    refuse("isolation", "TEST_DATA_DIR_SETUP differs from the reviewed creation/export/cleanup production")
for line in lines:
    line = line.strip()
    if re.match(r"(?:(?:override|export)\s+)?(?:define|undefine)\s+(?:TEST_DATA_DIR_SETUP|SHELL)\b", line) and line != "define TEST_DATA_DIR_SETUP":
        refuse("make-shape", "alternate setup/shell definition is unsupported")
    if re.match(r"(?:override\s+|export\s+)?TEST_DATA_DIR_SETUP\s*[:?+!]?=", line):
        refuse("isolation", "TEST_DATA_DIR_SETUP must not be reassigned")
shell_lines = [line.strip() for line in lines if re.match(r"(?:override\s+|export\s+)?SHELL\s*[:?+!]?=", line.strip())]
if shell_lines != ["SHELL := /bin/bash"]:
    refuse("make-shape", "the connected shell must be /bin/bash")

helper = Path("scripts/test-swift.sh")
try:
    body = read_source(helper).splitlines()
except OSError as error:
    refuse("helper-missing", str(error))

stages = [
    ('bindings', r'''set -uo pipefail
PROFILE="$1"
ENGINE_BRIDGE="$2"
SWIFT_TEST_CODESIGN_IDENTITY="$3"
SWIFT_TEST_MAX_SECONDS="$4"
SWIFT_TEST_MAX_TEST_SECONDS="$5"
SWIFT_TEST_LOG="$6"
shift 6'''),
    ('self-test', r'''echo "=== Apple phrase-restart Rust/Swift lockstep self-test ==="
"${ENGINE_BRIDGE}" --phrase-restart-self-test || exit $?'''),
    ('artifact-root', r'''# Match build-app profiles; this test path consumes existing host artifacts.
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
fi'''),
    ('generation', r'''if ! command -v xcodegen >/dev/null 2>&1; then
  echo "test-swift: xcodegen is required because the Xcode project is generated, not committed." >&2
  exit 2
fi
echo "=== Regenerating Xcode project from project.yml ==="
( cd macos && xcodegen generate ) || exit $?'''),
    ('XCTest', r'''echo "=== Swift front-end tests (CodescribeTests) ==="
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
  echo "test-swift: if the host really is loaded: make test-swift SWIFT_TEST_MAX_SECONDS=90" >&2
  rc=4
fi
exit $rc''')
]
cursor = 0
for name, expected in stages:
    while cursor < len(body) and (not body[cursor].strip() or body[cursor].lstrip().startswith("#")):
        cursor += 1
    expected_lines = expected.splitlines()
    # Leading comments in the reviewed stage are documentation, not commands.
    while expected_lines and expected_lines[0].startswith("#"):
        expected_lines.pop(0)
    if body[cursor:cursor + len(expected_lines)] != expected_lines:
        refuse(name, "unsupported or missing ordered helper production at line " + str(cursor + 1)
               + "; self-test and generation must fail-fast before XCTest")
    cursor += len(expected_lines)
if any(line.strip() and not line.lstrip().startswith("#") for line in body[cursor:]):
    refuse("helper-shape", "unexpected executable text after the reviewed helper")
