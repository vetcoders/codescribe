#!/usr/bin/env bash
# Counterexample: a fake test writes a Codescribe file in sandbox HOME.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/codescribe-verify-home-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
export CODESCRIBE_TEST_DATA_DIR="$tmp"
export HOME="$tmp/account-home"
mkdir -p "$HOME"
cat > "$tmp/fake-test" <<'SCRIPT'
#!/usr/bin/env bash
mkdir -p "$HOME/.codescribe"
printf 'leak\n' > "$HOME/.codescribe/canary"
SCRIPT
chmod +x "$tmp/fake-test"
if bash "$root/scripts/verify-test-home.sh" --fixture-runner "$tmp/fake-test" > "$tmp/stdout" 2> "$tmp/stderr"; then
    echo "verify HOME leak gate accepted a write" >&2
    exit 1
fi
if ! /usr/bin/grep -Fq "$tmp/home/.codescribe/canary" "$tmp/stderr"; then
    echo "verify HOME leak gate omitted the written path" >&2
    exit 1
fi
