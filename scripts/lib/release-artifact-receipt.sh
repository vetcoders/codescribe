#!/bin/bash
# Private invocation handoff, not signing or payload acceptance. Never source
# the receipt: it is exactly six newline-terminated data fields.
set -euo pipefail
export LC_ALL=C

refuse() { echo "ERROR: release artifact receipt: $*" >&2; exit 1; }

[[ $# -ge 1 ]] || refuse "missing operation"
operation=$1
shift
case "$operation" in
  write)
    [[ $# -eq 5 ]] || refuse "write needs receipt, run, variant, version, artifact"
    receipt=$1 run=$2 variant=$3 version=$4 artifact=$5
    [[ ! -e "$receipt" && ! -L "$receipt" ]] || refuse "receipt already exists"
    ;;
  verify)
    [[ $# -eq 3 ]] || refuse "verify needs receipt, run, variant"
    receipt=$1 expected_run=$2 expected_variant=$3
    [[ -f "$receipt" && ! -L "$receipt" ]] || refuse "missing or nonregular receipt"
    # Bash read drops NUL bytes. Refuse them before parsing rather than accepting
    # a corrupted record whose visible fields happen to match.
    [[ $(LC_ALL=C tr -d '\000' < "$receipt" | wc -c) -eq $(wc -c < "$receipt") ]] || refuse "NUL byte"
    {
      IFS= read -r schema && IFS= read -r run && IFS= read -r variant &&
        IFS= read -r version && IFS= read -r artifact && IFS= read -r digest
    } < "$receipt" || refuse "incomplete record"
    [[ $(wc -l < "$receipt") -eq 6 && $(tail -c 1 "$receipt" | od -An -tu1) -eq 10 ]] || refuse "extra or unterminated data"
    [[ "$schema" == codescribe-release-artifact-v1 ]] || refuse "unknown schema"
    [[ "$run" == "$expected_run" ]] || refuse "wrong invocation"
    [[ "$variant" == "$expected_variant" ]] || refuse "wrong variant"
    [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || refuse "invalid digest"
    ;;
  *) refuse "unknown operation" ;;
esac

[[ "$run" =~ ^[a-zA-Z0-9._-]+$ ]] || refuse "invalid invocation"
[[ "$variant" == slim || "$variant" == full ]] || refuse "invalid variant"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.+-]+)?$ ]] || refuse "invalid version"
[[ "$artifact" == /*.dmg && "$artifact" != *[[:cntrl:]]* ]] || refuse "invalid artifact path"
[[ -f "$artifact" && ! -L "$artifact" ]] || refuse "missing or nonregular artifact"
actual_digest=$(shasum -a 256 < "$artifact")
actual_digest=${actual_digest%% *}
[[ "$actual_digest" =~ ^[0-9a-f]{64}$ ]] || refuse "hash failed"

if [[ "$operation" == write ]]; then
  # noclobber prevents reuse; an interrupted write cannot pass the strict reader.
  (umask 077; set -C; printf '%s\n' codescribe-release-artifact-v1 "$run" "$variant" "$version" "$artifact" "$actual_digest" > "$receipt")
else
  [[ "$actual_digest" == "$digest" ]] || refuse "artifact changed after production"
  root=$(cd "$(dirname "$0")/../.." && pwd)
  exec "$root/scripts/verify-dmg-payload.sh" "$artifact" --variant "$variant" --version "$version"
fi
