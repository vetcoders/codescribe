#!/bin/bash
# validate-gates.sh - Validate the Makefile GATE LEDGER against the Makefile and CI
#
# The repo has one rule about verification: a command is authoritative only for
# what it executes, and no surface may cite it as proof of something it does not
# run. That rule is unenforceable while the verification surface is a pile of
# targets nobody classified — which is how `make check` came to print "Quality
# gate passed" having run zero tests, and how `.github/workflows/rust.yml` came
# to call `make check` the "full local gate (incl. real-API / heavy e2e tests)".
#
# This script makes the classification machine-checked, in both directions:
#
#   1. every verification target in the Makefile has a GATE LEDGER entry
#      (an unclassified gate cannot exist);
#   2. every ledger entry names a target that still exists (no stale rows);
#   3. class and ci fields carry legal values;
#   4. the `ci=` claim matches what .github/workflows/ actually invokes —
#      so wiring a target into CI without updating the ledger goes red, and so
#      does claiming CI coverage that no workflow provides.
#
# Known limits of check 4, stated rather than implied. It matches `make <target>`
# literally: it does not follow a target reached transitively through $(MAKE)
# inside another recipe, and it does not see a workflow that runs the same
# underlying script directly (release.yml invokes scripts/verify-dmg-payload.sh
# without going through `make verify-dmg`). Those cases belong in the row's reach
# text; widening the field to "is this check covered somehow" would make it a
# judgement call, and a gate that needs judgement is prose.
#
# Same shape as validate-envs.sh, one level up: that one keeps env vars honest
# against docs/ENV_REGISTRY.toml, this one keeps gates honest against CI.
# `tests/gate_registry.rs` runs it under `cargo test`, which is what puts it in
# front of CI — CI never invokes a Makefile quality target directly.
#
# PORTABILITY: written for bash 3.2, which is what /bin/bash still is on macOS
# (verified 2026-08-08: 3.2.57, no `mapfile`, no `declare -A`). This host has
# bash 5 first on PATH and a runner may not, so no bash-4 construct is used —
# a gate that passes or fails on PATH order is not a gate.
#
# Usage:
#   ./scripts/validate-gates.sh          # validate (exit 1 on drift)
#   ./scripts/validate-gates.sh --list   # print the classified surface
#
# Exit codes:
#   0 - ledger, Makefile and CI agree
#   1 - drift (unclassified target, stale row, bad field, or CI mismatch)
#
# Created by Vetcoders (c)2026

set -eo pipefail

MAKEFILE="Makefile"
WORKFLOW_DIR=".github/workflows"
LIST_MODE=0
ERRORS=0

# Targets whose whole job is to verify something. A target whose name matches
# this and carries no ledger row is the failure this script exists to catch.
VERIFICATION_TARGET_RE='^(check|lint|semgrep|verify|verify-.+|test|test-.+|smoke-.+)$'
LEGAL_CLASSES="static hermetic operator"

usage() {
    cat <<EOF
Usage: $0 [--list]

  --list   Print the classified verification surface and exit 0.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --list)
            LIST_MODE=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown arg: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ ! -f "$MAKEFILE" ]]; then
    echo "validate-gates: $MAKEFILE not found (run from the repo root)" >&2
    exit 2
fi

fail() {
    echo "  ✗ $*" >&2
    ERRORS=$((ERRORS + 1))
}

# The app logger is process-global (`Once`), so test isolation must exist in
# the parent shell before the first cargo/xcodebuild child starts. Keep this
# semantic guard here because this script already runs inside `make verify` via
# tests/gate_registry.rs; a Make dry-run would execute recursive recipes and is
# unsafe for several operator targets.
make_block() {
    local start="$1"
    local end="$2"
    sed -n "/^${start}$/,/^${end}$/p" "$MAKEFILE"
}

make_target_block() {
    local target="$1"
    sed -n "/^${target}:/,/^[a-zA-Z0-9_][a-zA-Z0-9_.-]*:/p" "$MAKEFILE"
}

test_data_setup="$(make_block 'define TEST_DATA_DIR_SETUP' 'endef')"
test_setup="$(make_block 'define TEST_SETUP' 'endef')"
verify_recipe="$(sed -n '/^verify:/,/^[a-zA-Z0-9_][a-zA-Z0-9_.-]*:/p' "$MAKEFILE")"

if [[ "$test_data_setup" != *'mktemp -d'* ]]; then
    fail "TEST_DATA_DIR_SETUP must create a unique directory with mktemp -d"
fi
if [[ "$test_data_setup" != *'export CODESCRIBE_DATA_DIR='* ]]; then
    fail "TEST_DATA_DIR_SETUP must export CODESCRIBE_DATA_DIR to every child process"
fi
if [[ "$test_data_setup" != *'trap cleanup_codescribe_test_data_dir EXIT'* ||
      "$test_data_setup" != *'rm -rf -- "$$CODESCRIBE_TEST_DATA_DIR"'* ]]; then
    fail "TEST_DATA_DIR_SETUP must clean its exact mktemp directory on shell exit"
fi
if [[ "$test_setup" != *'$(TEST_DATA_DIR_SETUP)'* ]]; then
    fail "TEST_SETUP must establish process-wide test data isolation"
fi
if [[ "$verify_recipe" != *'$(TEST_DATA_DIR_SETUP)'* ]]; then
    fail "verify must establish process-wide test data isolation before cargo"
elif [[ "${verify_recipe%%cargo test*}" != *'$(TEST_DATA_DIR_SETUP)'* ]]; then
    fail "verify must export its isolated data directory before the first cargo test"
fi
if ! grep -Fq 'export CODESCRIBE_DATA_DIR="$$CODESCRIBE_TEST_DATA_DIR_GUARD"' "$MAKEFILE"; then
    fail "ENV_LOAD must restore the harness-owned CODESCRIBE_DATA_DIR after sourcing operator dotenv"
fi

# These are the operator targets whose direct cargo children share TEST_SETUP.
# Naming them makes removal of the setup fail closed instead of disappearing
# from a dynamic search together with the protection it was meant to enforce.
for target in test test-quick test-e2e test-e2e-real test-sse test-formatting \
              test-engine-apple-channel test-engine-candle test-all; do
    target_recipe="$(make_target_block "$target")"
    if [[ -z "$target_recipe" ]]; then
        fail "$target is missing while test isolation still expects it"
    elif [[ "${target_recipe%%cargo test*}" != *'$(TEST_SETUP)'* ]]; then
        fail "$target must invoke TEST_SETUP before its first cargo test"
    fi
done

swift_recipe="$(make_target_block 'test-swift')"
if [[ "$swift_recipe" != *'test-swift: $(ENGINE_BRIDGE)'* ]]; then
    fail "test-swift must build the canonical Apple STT bridge dependency"
fi
if [[ "${swift_recipe%%xcodebuild test*}" != *'$(ENGINE_BRIDGE) --phrase-restart-self-test || exit $$?'* ]]; then
    fail "test-swift must fail-fast on the phrase-restart Swift lockstep self-test before XCTest"
fi
if [[ "${swift_recipe%%xcodebuild test*}" != *'$(TEST_DATA_DIR_SETUP)'* ]]; then
    fail "test-swift must export its isolated data directory before xcodebuild test"
fi

# ---------------------------------------------------------------------------
# Collect: verification targets, ledger rows
# ---------------------------------------------------------------------------

# Target definition lines start at column 0 and are not .PHONY / variable
# assignments. Parallel indexed arrays throughout — bash 3.2 has no `declare -A`.
VERIFICATION_TARGETS=""
while IFS= read -r t; do
    if [[ "$t" =~ $VERIFICATION_TARGET_RE ]]; then
        VERIFICATION_TARGETS="$VERIFICATION_TARGETS $t"
    fi
done < <(sed -nE 's/^([a-zA-Z0-9_][a-zA-Z0-9_.-]*):([^=].*)?$/\1/p' "$MAKEFILE" | sort -u)

is_verification_target() {
    case " $VERIFICATION_TARGETS " in
        *" $1 "*) return 0 ;;
        *) return 1 ;;
    esac
}

LEDGER_NAMES=""
LEDGER_CLASS_OF=()
LEDGER_CI_OF=()
LEDGER_REACH_OF=()
LEDGER_NAME_OF=()

while IFS= read -r row; do
    [[ -n "$row" ]] || continue
    if [[ ! "$row" =~ ^([a-zA-Z0-9_.-]+)[[:space:]]+class=([a-z]+)[[:space:]]+ci=([a-z]+)[[:space:]]+--[[:space:]]+(.+)$ ]]; then
        fail "malformed ledger row: '# gate: $row'"
        echo "    expected: # gate: <target> class=<${LEGAL_CLASSES// /|}> ci=<yes|no> -- <what it runs>" >&2
        continue
    fi
    name="${BASH_REMATCH[1]}"
    class="${BASH_REMATCH[2]}"
    ci="${BASH_REMATCH[3]}"
    reach="${BASH_REMATCH[4]}"

    case " $LEDGER_NAMES " in
        *" $name "*)
            fail "duplicate ledger row for '$name'"
            continue
            ;;
    esac
    case " $LEGAL_CLASSES " in
        *" $class "*) ;;
        *) fail "'$name' has class=$class; legal classes: $LEGAL_CLASSES" ;;
    esac
    if [[ "$ci" != "yes" && "$ci" != "no" ]]; then
        fail "'$name' has ci=$ci; legal values: yes, no"
    fi

    LEDGER_NAMES="$LEDGER_NAMES $name"
    LEDGER_NAME_OF[${#LEDGER_NAME_OF[@]}]="$name"
    LEDGER_CLASS_OF[${#LEDGER_CLASS_OF[@]}]="$class"
    LEDGER_CI_OF[${#LEDGER_CI_OF[@]}]="$ci"
    LEDGER_REACH_OF[${#LEDGER_REACH_OF[@]}]="$reach"
done < <(sed -nE 's/^# gate: (.*)$/\1/p' "$MAKEFILE")

# What CI actually invokes. Workflows call make by name (`run: make release-dmgs`),
# so a literal word-boundary match on `make <target>` is the ground truth here.
#
# Comment lines are stripped first, on purpose: a workflow that *mentions* a
# target in prose is not a workflow that runs it, and that exact confusion is
# what this script exists to remove — rust.yml carried the sentence "Full local
# gate (incl. real-API / heavy e2e tests) still via: make check" above a job
# that ran cargo directly, pointing every reader at a target that runs no tests.
ci_invokes() {
    local target="$1"
    [[ -d "$WORKFLOW_DIR" ]] || return 1
    find "$WORKFLOW_DIR" -type f \( -name '*.yml' -o -name '*.yaml' \) -exec cat {} + 2>/dev/null |
        sed -E 's/^[[:space:]]*#.*$//' |
        grep -Eq "(^|[^A-Za-z0-9_-])make[[:space:]]+${target}([[:space:]]|\$)"
}

# ---------------------------------------------------------------------------
# Validate
# ---------------------------------------------------------------------------

for t in $VERIFICATION_TARGETS; do
    case " $LEDGER_NAMES " in
        *" $t "*) ;;
        *)
            fail "'$t' is a verification target with no GATE LEDGER row"
            echo "    add to the ledger block in $MAKEFILE:" >&2
            echo "    # gate: $t class=<${LEGAL_CLASSES// /|}> ci=<yes|no> -- <what it runs, what it does not>" >&2
            ;;
    esac
done

i=0
while [[ $i -lt ${#LEDGER_NAME_OF[@]} ]]; do
    name="${LEDGER_NAME_OF[$i]}"
    if ! is_verification_target "$name"; then
        fail "ledger row '$name' names no verification target in $MAKEFILE (stale row, or the target was renamed)"
        i=$((i + 1))
        continue
    fi

    claimed="${LEDGER_CI_OF[$i]}"
    if ci_invokes "$name"; then
        actual="yes"
    else
        actual="no"
    fi
    if [[ "$claimed" != "$actual" ]]; then
        if [[ "$actual" == "yes" ]]; then
            fail "'$name' claims ci=no but $WORKFLOW_DIR invokes 'make $name'"
        else
            fail "'$name' claims ci=yes but no workflow in $WORKFLOW_DIR invokes 'make $name'"
        fi
    fi
    i=$((i + 1))
done

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

printf '%-28s %-10s %-5s %s\n' "TARGET" "CLASS" "CI" "RUNS"
i=0
while [[ $i -lt ${#LEDGER_NAME_OF[@]} ]]; do
    printf '%-28s %-10s %-5s %s\n' \
        "${LEDGER_NAME_OF[$i]}" "${LEDGER_CLASS_OF[$i]}" "${LEDGER_CI_OF[$i]}" "${LEDGER_REACH_OF[$i]}"
    i=$((i + 1))
done

if [[ "$LIST_MODE" -eq 1 ]]; then
    exit 0
fi

if [[ "$ERRORS" -gt 0 ]]; then
    echo "" >&2
    echo "validate-gates: $ERRORS problem(s). The ledger is the repo's answer to" >&2
    echo "validate-gates: \"what did green mean?\" — fix the row or fix the target." >&2
    exit 1
fi

echo ""
echo "validate-gates: ${#LEDGER_NAME_OF[@]} verification targets classified, ledger matches Makefile and CI"
