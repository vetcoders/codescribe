#!/bin/sh
set -eu

unset SKIP

source_root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/codescribe-embargo-selftest.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

fail() {
  echo "embargo-selftest: FAIL: $*" >&2
  exit 1
}

command -v pre-commit >/dev/null 2>&1 || fail "pre-commit is required"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"

repo="$test_root/repo"
mkdir -p "$repo/scripts/git-hooks" "$repo/src" "$repo/.vibecrafted"
cp "$source_root/.pre-commit-config.yaml" "$repo/.pre-commit-config.yaml"
cp "$source_root/scripts/git-hooks/embargo-guard.sh" "$repo/scripts/git-hooks/"
cp "$source_root/scripts/commit-msg-provenance.sh" "$repo/scripts/"
chmod +x "$repo/scripts/git-hooks/embargo-guard.sh" "$repo/scripts/commit-msg-provenance.sh"

git -C "$repo" init -q -b cut/W1-selftest
git -C "$repo" config user.name selftest
git -C "$repo" config user.email selftest@example.invalid
printf '%s\n' \
  '[package]' \
  'name = "codescribe-embargo-selftest"' \
  'version = "0.1.0"' \
  'edition = "2021"' >"$repo/Cargo.toml"
printf '%s\n' 'fn main(){ missing_symbol();}' >"$repo/src/main.rs"
printf '%s\n' '{ invalid json' >"$repo/payload.json"
git -C "$repo" add .pre-commit-config.yaml Cargo.toml payload.json scripts src/main.rs

write_marker() {
  printf '%s\n' \
    'plan_id = "overlay-canvas-v1"' \
    'phase = "W1"' \
    'deferred_gates = ["cargo-check", "cargo-fmt", "cargo-clippy", "prettier"]' \
    'attestation = "open"' \
    'recovery_ref = "embargo/overlay-canvas-v1"' \
    >"$repo/.vibecrafted/embargo.toml"
}

write_closed_marker() {
  printf '%s\n' \
    'plan_id = "overlay-canvas-v1"' \
    'phase = "W2"' \
    'deferred_gates = ["cargo-check", "cargo-fmt", "cargo-clippy", "prettier"]' \
    'attestation = "W2_STRUCTURALLY_CLOSED"' \
    'recovery_ref = "embargo/overlay-canvas-v1"' \
    >"$repo/.vibecrafted/embargo.toml"
}

run_pre_commit() {
  (cd "$repo" && pre-commit "$@")
}

# Scenario A: every named gate is wired to the guard and demonstrably avoids
# executing against invalid Rust/JSON while the W1 marker is open.
write_marker
rust_before=$(shasum -a 256 "$repo/src/main.rs" | awk '{print $1}')
run_pre_commit run cargo-check --hook-stage pre-commit --all-files >/dev/null 2>&1 ||
  fail "scenario A cargo-check was not deferred"
run_pre_commit run cargo-fmt --hook-stage pre-commit --all-files >/dev/null 2>&1 ||
  fail "scenario A cargo-fmt was not deferred"
run_pre_commit run cargo-clippy --hook-stage pre-push --all-files >/dev/null 2>&1 ||
  fail "scenario A cargo-clippy was not deferred"
run_pre_commit run prettier --hook-stage pre-commit --all-files >/dev/null 2>&1 ||
  fail "scenario A prettier was not deferred"
rust_after=$(shasum -a 256 "$repo/src/main.rs" | awk '{print $1}')
[ "$rust_before" = "$rust_after" ] || fail "scenario A cargo-fmt changed Rust"
guard_log=$(cd "$repo" && scripts/git-hooks/embargo-guard.sh cargo-check -- sh -c 'exit 91' 2>&1) ||
  fail "scenario A direct guard rejected an active marker"
case "$guard_log" in
  *"phase=W1"*"SKIP=cargo-check,cargo-fmt,cargo-clippy,prettier"*"gate=cargo-check deferred"*) ;;
  *) fail "scenario A did not log phase and exact SKIP set" ;;
esac
echo "SCENARIO A PASS: W1 deferred cargo-check, cargo-fmt, cargo-clippy, and prettier by effect"

# Scenario B: hard guards still reject/execute under the same active marker.
{
  printf '%s%s\n' '-----BEGIN ' 'PRIVATE KEY-----'
  printf '%s\n' 'c2VsZnRlc3Qtbm90LWEtcmVhbC1rZXk='
  printf '%s%s\n' '-----END ' 'PRIVATE KEY-----'
} >"$repo/selftest-private.pem"
git -C "$repo" add selftest-private.pem
if run_pre_commit run detect-private-key --hook-stage pre-commit --files selftest-private.pem >/dev/null 2>&1; then
  fail "scenario B detect-private-key accepted a private-key fixture"
fi
printf '%s\n' 'bad commit message' >"$repo/bad-message"
if run_pre_commit run commit-msg-provenance --hook-stage commit-msg --commit-msg-filename bad-message >/dev/null 2>&1; then
  fail "scenario B commit provenance accepted a bad message"
fi
hard_status=0
(cd "$repo" && scripts/git-hooks/embargo-guard.sh semgrep -- sh -c 'exit 41' >/dev/null 2>&1) || hard_status=$?
[ "$hard_status" -eq 41 ] || fail "scenario B non-deferred semgrep sentinel did not execute"
printf '%s\n' 'unexpected = true' >"$repo/.vibecrafted/embargo.toml"
marker_status=0
(cd "$repo" && scripts/git-hooks/embargo-guard.sh cargo-check -- true >/dev/null 2>&1) || marker_status=$?
[ "$marker_status" -eq 2 ] || fail "scenario B malformed marker did not fail closed"
write_closed_marker
closed_status=0
(cd "$repo" && scripts/git-hooks/embargo-guard.sh cargo-check -- sh -c 'exit 42' >/dev/null 2>&1) || closed_status=$?
[ "$closed_status" -eq 42 ] || fail "scenario B closed attestation did not restore the command"
echo "SCENARIO B PASS: hard gates ran; malformed marker failed closed; closed attestation restored commands"

# Scenario C: deleting the marker restores each original command. Compilation
# gates fail on the invalid program, while formatters visibly touch/fail on the
# deliberately malformed fixtures.
rm "$repo/.vibecrafted/embargo.toml"
if run_pre_commit run cargo-check --hook-stage pre-commit --all-files >/dev/null 2>&1; then
  fail "scenario C cargo-check did not return"
fi
if run_pre_commit run cargo-clippy --hook-stage pre-push --all-files >/dev/null 2>&1; then
  fail "scenario C cargo-clippy did not return"
fi
run_pre_commit run cargo-fmt --hook-stage pre-commit --all-files >/dev/null 2>&1 ||
  fail "scenario C cargo-fmt did not execute successfully"
rust_after=$(shasum -a 256 "$repo/src/main.rs" | awk '{print $1}')
[ "$rust_before" != "$rust_after" ] || fail "scenario C cargo-fmt produced no file effect"
if run_pre_commit run prettier --hook-stage pre-commit --all-files >/dev/null 2>&1; then
  fail "scenario C prettier accepted invalid JSON"
fi
echo "SCENARIO C PASS: marker removal restored cargo-check, cargo-fmt, cargo-clippy, and prettier"

echo "embargo-selftest: 3/3 scenarios passed"
