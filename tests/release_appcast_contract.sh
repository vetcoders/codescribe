#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/codescribe-appcast-test.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT INT TERM

mkdir -p "$TEST_ROOT/artifacts" "$TEST_ROOT/bin"
touch "$TEST_ROOT/artifacts/Codescribe_1.2.3-test.dmg"
touch "$TEST_ROOT/artifacts/Codescribe_1.2.3-test_full.dmg"

cat > "$TEST_ROOT/bin/generate_appcast" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
OUT=""
while [ $# -gt 1 ]; do
  case "$1" in
    -o) OUT="$2"; shift 2 ;;
    --ed-key-file|--download-url-prefix) shift 2 ;;
    *) shift ;;
  esac
done
ARCHIVES="$1"
dmgs=("$ARCHIVES"/*.dmg)
[ ${#dmgs[@]} -eq 1 ]
[[ "${dmgs[0]}" != *_full.dmg ]]
printf '<rss><channel><item><enclosure url="%s"/></item></channel></rss>\n' \
  "$(basename "${dmgs[0]}")" > "$OUT"
STUB
chmod +x "$TEST_ROOT/bin/generate_appcast"

PRIVATE_KEY="nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A="
PUBLIC_KEY="11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo="
SPARKLE_BIN="$TEST_ROOT/bin" \
SPARKLE_ED_PRIVATE_KEY="$PRIVATE_KEY" \
SPARKLE_ED_PUBLIC_KEY="$PUBLIC_KEY" \
  "$REPO_ROOT/scripts/generate-appcast.sh" "$TEST_ROOT/artifacts" \
    -o "$TEST_ROOT/appcast.xml"

[ "$(grep -c '<enclosure ' "$TEST_ROOT/appcast.xml")" -eq 1 ]
grep -q 'Codescribe_1.2.3-test.dmg' "$TEST_ROOT/appcast.xml"
! grep -q '_full.dmg' "$TEST_ROOT/appcast.xml"
echo "release appcast contract: PASS (one stable slim enclosure from slim+full input)"
