#!/bin/bash
# W3 only. Executes the real producer and unmodified release target recipes in
# temporary fixtures. No real build, signing, notary, credentials or app install.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
REAL_MAKE=$(command -v make)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/release-receipt-test.XXXXXXXXXX")
trap 'rm -rf "$TEST_ROOT"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

fail() { echo "FAIL: $*" >&2; exit 1; }

fixture() {
  FIXTURE="$TEST_ROOT/$1 path with spaces"
  mkdir -p "$FIXTURE/scripts/lib" "$FIXTURE/bin" "$FIXTURE/tmp"
  cp "$ROOT/scripts/build-dmg.sh" "$FIXTURE/scripts/producer.sh"
  cp "$ROOT/scripts/lib/release-artifact-receipt.sh" "$FIXTURE/scripts/lib/"
  : > "$FIXTURE/scripts/entitlements.plist"
  printf 'version = "1.2.3"\n' > "$FIXTURE/Cargo.toml"
  printf 'aaaaaaaaa\n' > "$FIXTURE/head"
  # Extract the actual recipes, not a second test-only consumer. Exclude the
  # main Makefile's parse-time signing-key discovery and unrelated prerequisites.
  printf 'SHELL := /bin/bash\ndist-preflight-signed ensure-models:\n\t@:\n' > "$FIXTURE/Makefile"
  awk '/^release-standard:|^release-full:/ {copy=1} copy && /^$/ {copy=0} copy {print}' \
    "$ROOT/Makefile" >> "$FIXTURE/Makefile"
  cat > "$FIXTURE/bin/git" <<'SH'
#!/bin/bash
set -eu
[[ "$*" == *'rev-parse --short=9 HEAD' ]] || exit 90
cat "$FIXTURE/head"
SH
  cat > "$FIXTURE/bin/make" <<'SH'
#!/bin/bash
set -eu
[[ "$*" == 'app PROFILE=release' ]] || exit 91
[[ "${SCENARIO:-}" != build-failure ]] || exit 41
mkdir -p "$FIXTURE/macos/build/Build/Products/Release/Codescribe.app"
# Simulate source advancing after build-dmg froze its identity.
printf 'bbbbbbbbb\n' > "$FIXTURE/head"
printf 'version = "9.9.9"\n' > "$FIXTURE/Cargo.toml"
SH
  cat > "$FIXTURE/bin/hdiutil" <<'SH'
#!/bin/bash
set -eu
[[ "$1" == create ]] || exit 92
[[ "${SCENARIO:-}" != dmg-failure ]] || exit 42
printf 'fixture DMG\n' > "${!#}"
printf '%s\n' "${!#}" > "$FIXTURE/produced-path"
SH
  cat > "$FIXTURE/bin/codesign" <<'SH'
#!/bin/bash
[[ "${SCENARIO:-}" != sign-failure ]] || exit 43
exit 0
SH
  cat > "$FIXTURE/scripts/notarize.sh" <<'SH'
#!/bin/bash
set -eu
[[ "${SCENARIO:-}" != notary-failure ]] || exit 44
# Stapling changes bytes: receipt must hash the result, not the pre-notary DMG.
printf 'fixture staple\n' >> "$1"
SH
  cat > "$FIXTURE/scripts/verify-dmg-payload.sh" <<'SH'
#!/bin/bash
set -eu
printf '%s\n' "$@" > "$FIXTURE/verifier-args"
exit "${VERIFIER_RC:-0}"
SH
  cat > "$FIXTURE/scripts/build-dmg.sh" <<'SH'
#!/bin/bash
set -euo pipefail
args=("$@")
receipt=""
while [[ $# -gt 0 ]]; do
  if [[ "$1" == --receipt ]]; then receipt=$2; shift; fi
  shift
done
printf '%s\n' "$receipt" > "$FIXTURE/receipt-path"
"$(dirname "$0")/producer.sh" "${args[@]}"
cp "$receipt" "$FIXTURE/success-receipt"
# Corrupt the real producer output at its actual consumer boundary. The parser
# remains exclusively production code, and every negative asserts zero calls.
case "${SCENARIO:-}" in
  missing) rm "$receipt" ;;
  malformed) printf 'broken\n' > "$receipt" ;;
  wrong-run) sed '2s/.*/old-invocation/' "$receipt" > "$receipt.tmp"; mv "$receipt.tmp" "$receipt" ;;
  wrong-variant) sed '3s/.*/full/' "$receipt" > "$receipt.tmp"; mv "$receipt.tmp" "$receipt" ;;
  wrong-version) sed '4s/.*/invalid version/' "$receipt" > "$receipt.tmp"; mv "$receipt.tmp" "$receipt" ;;
  extra) printf 'extra\n' >> "$receipt" ;;
  unterminated) printf 'extra' >> "$receipt" ;;
  nul) printf '\000' >> "$receipt" ;;
  shell-data) printf '$(touch "%s/injected")\n' "$FIXTURE" > "$receipt" ;;
  stale) cp "$FIXTURE/older-receipt" "$receipt" ;;
  changed-artifact) printf 'replaced\n' > "$(cat "$FIXTURE/produced-path")" ;;
esac
SH
  chmod +x "$FIXTURE"/bin/* "$FIXTURE"/scripts/*.sh
}

invoke() {
  local target=$1
  # env -i removes inherited make flags and production app/signing overrides.
  # Only the absolute outer make is real; nested make resolves to the stub.
  set +e
  env -i PATH="$FIXTURE/bin:/usr/bin:/bin" FIXTURE="$FIXTURE" \
    TMPDIR="$FIXTURE/tmp" SCENARIO="${SCENARIO:-}" VERIFIER_RC="${VERIFIER_RC:-0}" \
    "$REAL_MAKE" --no-print-directory -C "$FIXTURE" "$target" \
    CODESCRIBE_DIST_CODESIGN_IDENTITY=fixture-identity \
    CODESCRIBE_DIST_LICENSE_KEY=fixture-key CODESCRIBE_DIST_SPARKLE_KEY=fixture-key \
    > "$FIXTURE/output" 2>&1
  RESULT=$?
  set -e
}

success() {
  local target=$1 variant=$2
  SCENARIO="" VERIFIER_RC=0
  invoke "$target"
  [[ "$RESULT" -eq 0 ]] || { cat "$FIXTURE/output"; fail "$target exit $RESULT"; }
  local artifact
  artifact=$(cat "$FIXTURE/produced-path")
  [[ "$artifact" == *'Codescribe_1.2.3-'*'-aaaaaaaaa'*'.dmg' ]] || fail 'producer did not freeze identity'
  [[ $(cat "$FIXTURE/head") == bbbbbbbbb ]] || fail 'HEAD race not exercised'
  printf '%s\n' "$artifact" --variant "$variant" --version 1.2.3 > "$FIXTURE/expected"
  cmp "$FIXTURE/expected" "$FIXTURE/verifier-args" || fail 'verifier argument contract'
  [[ ! -e $(cat "$FIXTURE/receipt-path") ]] || fail 'private receipt not cleaned'
}

for variant in slim full; do
  fixture "$variant"
  target=release-standard
  [[ "$variant" != full ]] || target=release-full
  success "$target" "$variant"
  echo "PASS: $variant exact artifact/version despite HEAD drift; spaces; post-notary hash"
done

negative_count=0
for SCENARIO in build-failure dmg-failure sign-failure notary-failure missing malformed wrong-run wrong-variant wrong-version extra unterminated nul shell-data stale changed-artifact; do
  scenario=$SCENARIO
  fixture "$scenario"
  # A prior success and its DMG really exist before the later failing invocation.
  success release-standard slim
  cp "$FIXTURE/success-receipt" "$FIXTURE/older-receipt"
  rm "$FIXTURE/verifier-args" "$FIXTURE/success-receipt"
  printf 'version = "1.2.3"\n' > "$FIXTURE/Cargo.toml"
  printf 'aaaaaaaaa\n' > "$FIXTURE/head"
  SCENARIO=$scenario
  invoke release-standard
  [[ "$RESULT" -ne 0 ]] || fail "$scenario accepted"
  [[ ! -e "$FIXTURE/verifier-args" ]] || fail "$scenario reached verifier"
  [[ ! -e "$FIXTURE/injected" ]] || fail 'receipt executed as shell'
  [[ ! -e $(cat "$FIXTURE/receipt-path") ]] || fail "$scenario leaked receipt"
  case "$scenario" in
    build-failure|dmg-failure|sign-failure|notary-failure)
      [[ ! -e "$FIXTURE/success-receipt" ]] || fail "$scenario published success" ;;
  esac
  negative_count=$((negative_count + 1))
  echo "PASS: $scenario rejected before verifier (older success present)"
done

fixture verifier-failure
SCENARIO="" VERIFIER_RC=47
invoke release-standard
[[ "$RESULT" -ne 0 && -f "$FIXTURE/verifier-args" ]] || fail 'verifier failure swallowed or not exercised'
# make maps recipe failure to its own exit code; the helper must preserve 47.
set +e
env -i PATH=/usr/bin:/bin FIXTURE="$FIXTURE" VERIFIER_RC=47 \
  bash "$FIXTURE/scripts/lib/release-artifact-receipt.sh" verify \
  "$FIXTURE/success-receipt" "$(sed -n '2p' "$FIXTURE/success-receipt")" slim
helper_rc=$?
set -e
[[ "$helper_rc" -eq 47 ]] || fail "verifier exit became $helper_rc"
negative_count=$((negative_count + 1))
echo 'PASS: actual target fails and helper preserves verifier exit 47'
echo "SCENARIO_CENSUS positive=2 negative=$negative_count"
