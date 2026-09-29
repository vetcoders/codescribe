#!/bin/bash
# Make recipe selection contract. No Rust execution, models, accounts or audio.
# Run when this shell gate is restored; this does not unlock production gates.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/codescribe-roundtrip-gate.XXXXXX")
# Every command below is foreground and finite. Make waits for recipe shells;
# bash waits for both cargo/tee pipeline children. No watchdog/sleep to orphan.
cleanup() {
    local rc=$?
    if [[ $rc -eq 0 ]]; then
        rm -rf -- "$fixture"
    else
        printf 'roundtrip-gate-contract: retained diagnostics: %s\n' "$fixture" >&2
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$fixture/bin"
case_count=0

cat > "$fixture/bin/cargo" <<'CARGO'
#!/bin/bash
set -euo pipefail
printf '%s|%s\n' "${CODESCRIBE_E2E_ROUNDTRIP-<unset>}" "$*" >> "$ROUNDTRIP_CALLS"
call_count=$(wc -l < "$ROUNDTRIP_CALLS")
printf 'fake-cargo-call=%s\n' "${call_count// /}"
if [[ $call_count -eq $ROUNDTRIP_FAIL_AT ]]; then
    exit "$ROUNDTRIP_FAIL_RC"
fi
CARGO

# A broken discovery guard must hit a fixture tripwire, never the real keychain.
cat > "$fixture/bin/security" <<'SECURITY'
#!/bin/bash
printf 'unexpected security execution\n' >> "$ROUNDTRIP_SHELL_CODES.executed"
exit 97
SECURITY

# GNU make itself maps recipe failures to exit 2. Record the ACTUAL recipe
# shell status as well to prove the original cargo code survived tee unchanged.
# Make 3.81 evaluates overridden := RHS expressions, including $(shell ...).
# Those are setup discovery, NOT recipes. Refuse to execute discovery at all;
# accept only the known setup probes and mark recipe entry in TEST_SETUP.
cat > "$fixture/bin/recipe-shell" <<'SHELL'
#!/bin/bash
set -uo pipefail
printf 'shell argv:' >> "$ROUNDTRIP_SHELL_CODES.argv"
printf ' %q' "$@" >> "$ROUNDTRIP_SHELL_CODES.argv"
printf '\n' >> "$ROUNDTRIP_SHELL_CODES.argv"
if [[ $# -eq 2 && $1 == -c ]]; then
    case "$2" in
        ': roundtrip-recipe; '*) ;;
        'security find-identity -v -p codesigning '*|'./scripts/lib/data-assets.sh dir')
            printf '%s\n' "$2" >> "$ROUNDTRIP_SHELL_CODES.discovery"
            exit 0
            ;;
        *)
            printf '%s\n' "$2" >> "$ROUNDTRIP_SHELL_CODES.rejected"
            exit 97
            ;;
    esac
else
    printf 'unexpected shell arguments\n' >> "$ROUNDTRIP_SHELL_CODES.rejected"
    exit 97
fi
/bin/bash "$@"
recipe_rc=$?
printf '%s\n' "$recipe_rc" >> "$ROUNDTRIP_SHELL_CODES"
exit "$recipe_rc"
SHELL
chmod +x "$fixture/bin/cargo" "$fixture/bin/security" "$fixture/bin/recipe-shell"

fail() {
    printf 'roundtrip-gate-contract: %s\n' "$*" >&2
    exit 1
}

run_case() {
    local target=$1 opt_in=$2 fail_at=$3 fail_rc=$4
    local case_root="$fixture/$target-$opt_in-$fail_at-$fail_rc"
    local make_rc=0
    local -a opt_in_env=("ROUNDTRIP_CASE_OPT_IN=$opt_in")
    mkdir -p "$case_root"
    : > "$case_root/calls"
    : > "$case_root/shell-codes"
    if [[ $opt_in != missing ]]; then
        opt_in_env+=("CODESCRIBE_E2E_ROUNDTRIP=$opt_in")
    fi
    # Consume the real repository Makefile, not an extracted/copied recipe.
    # Override only host setup and eager discovery; keep recipe selection,
    # opt-in assignments, cargo commands, tee and failure handling intact.
    # A clean child environment prevents caller MAKEFLAGS/.env from changing it.
    local -a make_command=(env -i PATH="$fixture/bin:/usr/bin:/bin" \
        ROUNDTRIP_CALLS="$case_root/calls" \
        ROUNDTRIP_SHELL_CODES="$case_root/shell-codes" \
        ROUNDTRIP_FAIL_AT="$fail_at" ROUNDTRIP_FAIL_RC="$fail_rc" \
        "${opt_in_env[@]}" \
        /usr/bin/make --no-print-directory -rR -C "$fixture" -f "$repo_root/Makefile" \
        "SHELL=$fixture/bin/recipe-shell" \
        'CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY=' \
        'CODESCRIBE_DEVELOPER_ID_IDENTITY=' \
        'CODESCRIBE_LICENSE_PUBLIC_KEY_FILE=' 'CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE=' \
        "DATA_ASSETS_DIR=$fixture/absent-corpus" \
        'ENV_LOAD=:' 'TEST_SETUP=: roundtrip-recipe; LOG="$(TEST_LOG)"' \
        "TEST_LOG=$case_root/test.log" "$target")
    printf '%q ' "${make_command[@]}" > "$case_root/command"
    printf '\n' >> "$case_root/command"
    "${make_command[@]}" > "$case_root/make.log" 2>&1 || make_rc=$?
    printf '%s\n' "$make_rc" > "$case_root/make-code"

    local expected_rc=0 expected_make_rc=0
    if [[ $fail_at -ne 0 ]]; then
        expected_rc=$fail_rc
        expected_make_rc=2
    fi
    [[ $make_rc -eq $expected_make_rc ]] || fail "$target make exit=$make_rc, expected=$expected_make_rc"
    [[ ! -e $case_root/shell-codes.rejected ]] || fail "$target attempted unexpected shell discovery"
    [[ ! -e $case_root/shell-codes.executed ]] || fail "$target executed setup discovery"
    printf '%s\n' "$expected_rc" > "$case_root/expected-codes"
    cmp -s "$case_root/expected-codes" "$case_root/shell-codes" || fail "$target lost exact recipe exit $expected_rc"

    if [[ $target == test-e2e-roundtrip ]]; then
        printf '%s\n' '1|test --test e2e_vad_flow -- --ignored --nocapture' > "$case_root/expected-calls"
        if [[ $fail_at -ne 1 ]]; then
            printf '%s\n' '1|test --test e2e_round_trip -- --ignored --nocapture' >> "$case_root/expected-calls"
        fi
    else
        local expected_opt_in=$opt_in
        if [[ $opt_in == missing ]]; then expected_opt_in='<unset>'; fi
        printf '%s|%s\n' "$expected_opt_in" 'test --workspace --all-targets -- --nocapture' > "$case_root/expected-calls"
        if [[ $fail_at -eq 0 ]]; then
            grep -q 'Heavy round-trip NOT RUN' "$case_root/test.log" || fail "$target concealed unrun heavy tests"
        fi
    fi
    cmp -s "$case_root/expected-calls" "$case_root/calls" || fail "$target selected unexpected cargo commands or opt-in"
    grep -q 'fake-cargo-call=1' "$case_root/test.log" || fail "$target did not tee cargo output"
    if [[ $fail_at -ne 0 ]] && grep -q 'Done. Log:' "$case_root/test.log"; then
        fail "$target announced completion after cargo failed"
    fi
    if [[ $fail_at -eq 0 ]]; then
        grep -q 'Done. Log:' "$case_root/test.log" || fail "$target concealed completion"
    fi
    case_count=$((case_count + 1))
    printf 'case=%s target=%s opt_in=%s fail_at=%s recipe_rc=%s make_rc=%s calls=%s PASS\n' \
        "$case_count" "$target" "$opt_in" "$fail_at" "$expected_rc" "$make_rc" \
        "$(wc -l < "$case_root/calls" | tr -d ' ')"
}

# Cause-specific control: discovery must neither execute nor add recipe codes.
# A command after the known probe would create a marker if passed to bash.
control_root="$fixture/discovery-control"
mkdir -p "$control_root"
: > "$control_root/shell-codes"
env -i PATH="$fixture/bin:/usr/bin:/bin" \
    ROUNDTRIP_SHELL_CODES="$control_root/shell-codes" \
    "$fixture/bin/recipe-shell" -c "security find-identity -v -p codesigning ; touch '$control_root/escaped'"
[[ ! -e $control_root/escaped && ! -s $control_root/shell-codes ]] || fail 'discovery executed or polluted recipe status'
[[ ! -e $control_root/shell-codes.executed ]] || fail 'discovery reached security tripwire'
[[ -s $control_root/shell-codes.discovery ]] || fail 'discovery observation missing'
control_rc=0
env -i PATH="$fixture/bin:/usr/bin:/bin" \
    ROUNDTRIP_SHELL_CODES="$control_root/shell-codes" \
    "$fixture/bin/recipe-shell" -c "touch '$control_root/escaped'" || control_rc=$?
[[ $control_rc -eq 97 && ! -e $control_root/escaped && ! -s $control_root/shell-codes ]] || fail 'unknown discovery did not fail closed'
[[ -s $control_root/shell-codes.rejected ]] || fail 'rejected discovery observation missing'
printf 'discovery-control: PASS (setup not executed; unknown command rejected=97; no recipe codes)\n'

for opt_in in missing 0 yes 1 true; do
    for target in test test-all; do
        run_case "$target" "$opt_in" 0 0
        run_case "$target" "$opt_in" 1 37
    done
    # Explicit target is itself consent; both existing heavy suites get opt-in.
    run_case test-e2e-roundtrip "$opt_in" 0 0
    run_case test-e2e-roundtrip "$opt_in" 1 23
    run_case test-e2e-roundtrip "$opt_in" 2 37
done
[[ $case_count -eq 35 ]] || fail "scenario count=$case_count, expected=35"
printf 'roundtrip-gate-contract: PASS (35 scenarios; fake cargo only; no heavy-test evidence)\n'
