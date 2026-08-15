#!/usr/bin/env bash
# ============================================================================
# keychain-doctor.sh — READ-ONLY diagnosis of the macOS user keychain domain
# ============================================================================
# Born from the same 2026-08-15 P0 as scripts/lib/keychain-session.sh: a
# release left its ephemeral signing keychain in the user search list and as
# the default keychain, then the directory holding it was wiped. Every later
# keychain access — including Codescribe's own — prompted the operator to
# unlock a file that no longer existed, with a password that could not work.
#
# This tool NEVER mutates. It runs `security list-keychains -d user` and
# `security default-keychain -d user` (both read-only forms — no `-s`), grades
# what it finds, and prints the exact recovery command line derived from the
# ACTUAL current list. It deliberately does not offer to run that line: fixing
# a poisoned keychain domain is an operator button, not an agent's.
#
# Note the recovery is *derived*, not canned. "Just set login.keychain-db" is
# wrong for any operator who legitimately keeps a second keychain in the search
# list — it would silently delete their entry. The doctor prints the surviving
# entries, in order, exactly as they should be re-installed.
#
#   scripts/keychain-doctor.sh          → human report
#   scripts/keychain-doctor.sh --quiet  → exit code only
#
# Exit: 0 healthy · 1 stale/missing paths found · 2 could not read the domain
#
# Contract tests: scripts/tests/keychain-session-test.sh
# ============================================================================
set -uo pipefail

SECURITY_BIN="${KEYCHAIN_SESSION_SECURITY_BIN:-/usr/bin/security}"
QUIET=0
[[ "${1:-}" == "--quiet" ]] && QUIET=1

say() { (( QUIET )) || printf '%s\n' "$*"; }

unquote() {
  local line="$1"
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line%"${line##*[![:space:]]}"}"
  line="${line#\"}"
  line="${line%\"}"
  printf '%s' "$line"
}

command -v "$SECURITY_BIN" >/dev/null 2>&1 || {
  printf 'keychain-doctor: %s is not executable\n' "$SECURITY_BIN" >&2
  exit 2
}

# A keychain is "resident" when it lives where the operator's keychains live.
# Anything else in the user search list came from a build: a release staging
# directory, $RUNNER_TEMP, /private/tmp. Existing on disk does NOT make it
# healthy — the 2026-08-15 prompt fired against a keychain whose file was
# perfectly intact, because a release was holding it open as the default.
is_resident() {
  case "$1" in
    "$HOME/Library/Keychains/"*) return 0 ;;
    /Library/Keychains/*)        return 0 ;;
    /System/Library/Keychains/*) return 0 ;;
    *) return 1 ;;
  esac
}

declare -a entries=() resident=() stale=() foreign=()
while IFS= read -r raw; do
  entry="$(unquote "$raw")"
  [[ -n "$entry" ]] || continue
  entries+=("$entry")
  if [[ ! -e "$entry" ]]; then
    stale+=("$entry")
  elif is_resident "$entry"; then
    resident+=("$entry")
  else
    foreign+=("$entry")
  fi
done < <("$SECURITY_BIN" list-keychains -d user 2>/dev/null)

(( ${#entries[@]} > 0 )) || {
  printf 'keychain-doctor: user search list is empty or unreadable\n' >&2
  exit 2
}

default_raw="$("$SECURITY_BIN" default-keychain -d user 2>/dev/null | head -n1)"
default="$(unquote "$default_raw")"
default_grade=ok
if [[ -n "$default" && ! -e "$default" ]]; then
  default_grade=STALE
elif [[ -n "$default" ]] && ! is_resident "$default"; then
  default_grade=HIJACKED
fi

say "=== user keychain search list (${#entries[@]} entr$( ((${#entries[@]}==1)) && echo y || echo ies)) ==="
for entry in "${entries[@]}"; do
  if [[ ! -e "$entry" ]]; then
    say "  STALE    $entry   (no such file)"
  elif is_resident "$entry"; then
    say "  ok       $entry"
  else
    say "  FOREIGN  $entry   (exists, but lives outside the operator's keychain directory — a build left it here)"
  fi
done
say ""
say "=== default keychain ==="
case "$default_grade" in
  STALE)    say "  STALE    $default   (no such file)" ;;
  HIJACKED) say "  HIJACKED $default" ;;
  *)        say "  ok       ${default:-<none>}" ;;
esac

if (( ${#stale[@]} == 0 && ${#foreign[@]} == 0 )) && [[ "$default_grade" == "ok" ]]; then
  say ""
  say "keychain-doctor: healthy — the search list holds only the operator's own"
  say "keychains and the default keychain is one of them."
  exit 0
fi

if [[ "$default_grade" == "HIJACKED" ]]; then
  say ""
  say "  The DEFAULT keychain is a build keychain. This is the state that makes"
  say "  unrelated apps pop \"<App> wants to use the '<name>' keychain\": every"
  say "  process in this login session resolves to it first, and its password is"
  say "  a value generated inside the release — no human knows it. If a release"
  say "  is running right now, that is the cause and it will end with the run;"
  say "  if none is, the run died without giving the domain back."
fi

# The recovery is derived from what is actually here, not canned. Hardcoding
# "just set login.keychain-db" would silently delete the entry of any operator
# who legitimately keeps a second keychain.
replacement=""
if (( ${#resident[@]} > 0 )); then
  replacement="${resident[0]}"
elif [[ -e "${HOME}/Library/Keychains/login.keychain-db" ]]; then
  replacement="${HOME}/Library/Keychains/login.keychain-db"
fi

say ""
say "=== recovery (OPERATOR RUNS THIS — the doctor does not) ==="
say "  Do NOT run this while a release is signing; it would pull the keychain"
say "  out from under it. Check first:  pgrep -fl 'release|codesign|notarytool'"
say ""
if (( ${#resident[@]} == 0 )); then
  say "  Nothing of the operator's own survives in the list. Restore the login keychain:"
  say "    security list-keychains -d user -s \"\$HOME/Library/Keychains/login.keychain-db\""
  say "    security default-keychain -d user -s \"\$HOME/Library/Keychains/login.keychain-db\""
else
  printf -v args '%q ' "${resident[@]}"
  say "  Re-install only the operator's own entries, in their current order:"
  say "    security list-keychains -d user -s ${args% }"
  [[ "$default_grade" == "ok" ]] || \
    say "    security default-keychain -d user -s $(printf '%q' "$replacement")"
fi
say "    security unlock-keychain \"\$HOME/Library/Keychains/login.keychain-db\""
say ""
say "  Cause is a release/signing run that borrowed the user keychain domain."
say "  scripts/lib/keychain-session.sh is the hardened path: it never takes the"
say "  default keychain, and it unlists before deleting. Anything still"
say "  hand-rolling 'security list-keychains -s' should move onto it."

exit 1
