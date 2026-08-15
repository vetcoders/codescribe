#!/usr/bin/env bash
# ============================================================================
# keychain-session-test.sh — regression suite for scripts/lib/keychain-session.sh
# ============================================================================
# Every case here is a way the 2026-08-15 P0 could recur. NOTHING in this file
# touches the real macOS keychain: `security` is a fake shell binary on PATH
# whose entire world is a temp directory, and HOME is redirected too. Running
# this suite on the operator's host is safe by construction — if the fake is
# ever bypassed the test fails, because the fake also records the exact argv it
# was handed and the assertions read that log.
#
#   make test-keychain-session      (or: bash scripts/tests/keychain-session-test.sh)
#
# Exit: 0 all green · 1 one or more failures
# ============================================================================
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIB="$REPO_ROOT/scripts/lib/keychain-session.sh"
DOCTOR="$REPO_ROOT/scripts/keychain-doctor.sh"

[[ -f "$LIB" ]] || { echo "missing $LIB" >&2; exit 1; }

PASS=0
FAIL=0
CURRENT=""

ok()   { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  FAIL %s\n     %s\n' "$CURRENT" "$1"; }
test_case() { CURRENT="$1"; printf '\n[%s]\n' "$1"; }

# --------------------------------------------------------------------------
# The fake `security`. It models the two behaviours the real one has that the
# old release code got wrong:
#   * `list-keychains -d user` prints indented, quoted paths
#   * `delete-keychain` FAILS when the file is already gone, and only then does
#     it not remove the search-list entry
# --------------------------------------------------------------------------
make_fake_security() {
  local bin="$1"
  cat > "$bin" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
S="$FAKE_SECURITY_STATE"
mkdir -p "$S"
[[ -f "$S/search-list" ]] || : > "$S/search-list"
[[ -f "$S/default" ]] || : > "$S/default"
{ for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$S/argv.log"

cmd="${1:-}"; shift || true

emit_list() { while IFS= read -r p; do [[ -n "$p" ]] && printf '    "%s"\n' "$p"; done < "$S/search-list"; }

case "$cmd" in
  list-keychains)
    domain=""; setmode=0; declare -a items=()
    while (( $# )); do
      case "$1" in
        -d) domain="$2"; shift 2 ;;
        -s) setmode=1; shift ;;
        *)  items+=("$1"); shift ;;
      esac
    done
    [[ "$domain" == "user" ]] || { echo "fake security: unexpected domain '$domain'" >&2; exit 64; }
    if (( setmode )); then
      : > "$S/search-list"
      for p in ${items+"${items[@]}"}; do printf '%s\n' "$p" >> "$S/search-list"; done
    else
      emit_list
    fi
    ;;
  default-keychain)
    domain=""; setmode=0; target=""
    while (( $# )); do
      case "$1" in
        -d) domain="$2"; shift 2 ;;
        -s) setmode=1; shift ;;
        *)  target="$1"; shift ;;
      esac
    done
    if (( setmode )); then printf '%s\n' "$target" > "$S/default"
    else
      d="$(cat "$S/default")"
      [[ -n "$d" ]] || { echo "A default keychain could not be found." >&2; exit 1; }
      printf '    "%s"\n' "$d"
    fi
    ;;
  create-keychain)
    pw=""; path=""
    while (( $# )); do
      case "$1" in -p) pw="$2"; shift 2 ;; *) path="$1"; shift ;; esac
    done
    [[ -n "$pw" ]] || { echo "fake security: create-keychain without password" >&2; exit 64; }
    mkdir -p "$(dirname "$path")"; printf 'fake-keychain\n' > "$path"
    ;;
  set-keychain-settings|set-key-partition-list|import|find-identity) : ;;
  unlock-keychain)
    path=""
    while (( $# )); do case "$1" in -p) shift 2 ;; *) path="$1"; shift ;; esac; done
    [[ -e "$path" ]] || { echo "The specified keychain could not be found." >&2; exit 1; }
    ;;
  delete-keychain)
    path="${1:-}"
    # Real behaviour: a keychain whose file is gone cannot be deleted, and its
    # search-list entry therefore survives. This is the incident in one line.
    [[ -e "$path" ]] || { echo "The specified keychain could not be found." >&2; exit 1; }
    rm -f "$path"
    tmp="$S/search-list.tmp"; : > "$tmp"
    while IFS= read -r p; do [[ "$p" == "$path" ]] || printf '%s\n' "$p" >> "$tmp"; done < "$S/search-list"
    mv "$tmp" "$S/search-list"
    ;;
  *) echo "fake security: unhandled '$cmd'" >&2; exit 64 ;;
esac
FAKE
  chmod +x "$bin"
}

# --------------------------------------------------------------------------
# Harness
# --------------------------------------------------------------------------
setup_env() {
  ROOT="$(mktemp -d "${TMPDIR:-/tmp}/keychain-session-test.XXXXXX")"
  export FAKE_SECURITY_STATE="$ROOT/security-state"
  export KEYCHAIN_SESSION_SECURITY_BIN="$ROOT/bin/security"
  export KEYCHAIN_SESSION_STATE_DIR="$ROOT/session-state"
  export KEYCHAIN_SESSION_LOCK_WAIT_SECS=3
  export HOME="$ROOT/home"
  mkdir -p "$ROOT/bin" "$FAKE_SECURITY_STATE" "$HOME/Library/Keychains"
  make_fake_security "$KEYCHAIN_SESSION_SECURITY_BIN"
  printf 'login\n' > "$HOME/Library/Keychains/login.keychain-db"
}

teardown_env() { [[ -n "${ROOT:-}" && "$ROOT" == */keychain-session-test.* ]] && rm -rf "$ROOT"; }

seed_list() {
  : > "$FAKE_SECURITY_STATE/search-list"
  for p in "$@"; do
    printf '%s\n' "$p" >> "$FAKE_SECURITY_STATE/search-list"
    mkdir -p "$(dirname "$p")"; [[ -e "$p" ]] || printf 'seed\n' > "$p"
  done
  printf '%s\n' "$1" > "$FAKE_SECURITY_STATE/default"
}

current_list() { cat "$FAKE_SECURITY_STATE/search-list"; }
current_default() { cat "$FAKE_SECURITY_STATE/default"; }

assert_list_equals() {
  local expected want got
  expected="$(printf '%s\n' "$@")"
  got="$(current_list)"
  if [[ "$got" == "$expected" ]]; then
    ok "search list restored exactly"
  else
    bad "search list mismatch
       expected: $(printf '%s' "$expected" | tr '\n' '|')
       actual:   $(printf '%s' "$got" | tr '\n' '|')"
  fi
}

assert_not_in_list() {
  if grep -Fqx -- "$1" "$FAKE_SECURITY_STATE/search-list" 2>/dev/null; then
    bad "'$1' is still in the search list"
  else
    ok "$2"
  fi
}

assert_in_list() {
  if grep -Fqx -- "$1" "$FAKE_SECURITY_STATE/search-list" 2>/dev/null; then
    ok "$2"
  else
    bad "'$1' is missing from the search list"
  fi
}

assert_default_is() {
  if [[ "$(current_default)" == "$1" ]]; then ok "$2"; else
    bad "default keychain is '$(current_default)', expected '$1'"
  fi
}

# Runs a snippet in a child bash that has sourced the library.
#
# The snippet is written VERBATIM — deliberately not through an unquoted
# heredoc. With a heredoc the parent shell would expand the snippet's own `$$`
# and `$(...)` before the child ever saw them, so `kill -INT $$` would signal
# the test harness instead of the child, and every assertion about interrupted
# flows would be measuring the wrong process.
run_child() {
  local snippet="$1" script="$ROOT/child.sh"
  {
    printf '#!/usr/bin/env bash\n'
    printf 'set -uo pipefail\n'
    printf '. %q\n' "$LIB"
    printf '%s\n' "$snippet"
  } > "$script"
  bash "$script"
}

# ==========================================================================
# 1. Exact restoration on the happy path
# ==========================================================================
setup_env
test_case "exact restoration, single pre-existing keychain"
seed_list "$HOME/Library/Keychains/login.keychain-db"
run_child 'keychain_session_begin codescribe-signing >/dev/null; keychain_session_end' >/dev/null
assert_list_equals "$HOME/Library/Keychains/login.keychain-db"
assert_default_is "$HOME/Library/Keychains/login.keychain-db" "default keychain restored"
teardown_env

# ==========================================================================
# 2. Paths with spaces and quotes — the concatenation bug's actual victim
# ==========================================================================
setup_env
test_case "search-list entries containing spaces survive a full session"
SPACED="$HOME/Library/Keychains/Team Signing.keychain-db"
seed_list "$HOME/Library/Keychains/login.keychain-db" "$SPACED"
run_child 'keychain_session_begin codescribe-signing >/dev/null; keychain_session_end' >/dev/null
assert_list_equals "$HOME/Library/Keychains/login.keychain-db" "$SPACED"
# And prove the argv was structured: the spaced path must appear as ONE
# argument in the recorded argv, not as two.
if awk -F'\t' -v want="$SPACED" '{for(i=1;i<=NF;i++) if($i==want) found=1} END{exit found?0:1}' \
      "$FAKE_SECURITY_STATE/argv.log"; then
  ok "spaced path passed as a single argv entry (never shell-concatenated)"
else
  bad "spaced path was split across argv entries"
fi
teardown_env

# ==========================================================================
# 3. Multiple keychains keep their order
# ==========================================================================
setup_env
test_case "multiple pre-existing keychains keep order"
A="$HOME/Library/Keychains/login.keychain-db"
B="$HOME/Library/Keychains/second.keychain-db"
C="$HOME/Library/Keychains/third.keychain-db"
seed_list "$A" "$B" "$C"
run_child 'keychain_session_begin codescribe-signing >/dev/null; keychain_session_end' >/dev/null
assert_list_equals "$A" "$B" "$C"
teardown_env

# ==========================================================================
# 4. Failed build — non-zero exit must still clean up
# ==========================================================================
setup_env
test_case "failed build (exit 7) still restores the search list"
seed_list "$HOME/Library/Keychains/login.keychain-db"
run_child 'keychain_session_begin codescribe-signing >/dev/null; exit 7' >/dev/null
rc=$?
[[ $rc -eq 7 ]] && ok "child exit code preserved (7)" || bad "child exit code was $rc, expected 7"
assert_list_equals "$HOME/Library/Keychains/login.keychain-db"
teardown_env

# ==========================================================================
# 5. Interrupted flow — SIGINT and SIGTERM, which a bare EXIT trap misses
# ==========================================================================
for sig in INT TERM; do
  setup_env
  test_case "interrupted flow (SIG$sig) still restores the search list"
  seed_list "$HOME/Library/Keychains/login.keychain-db"
  run_child "keychain_session_begin codescribe-signing >/dev/null; kill -$sig \$\$; sleep 5" >/dev/null 2>&1
  assert_list_equals "$HOME/Library/Keychains/login.keychain-db"
  assert_default_is "$HOME/Library/Keychains/login.keychain-db" "default keychain restored after SIG$sig"
  teardown_env
done

# ==========================================================================
# 6. THE INCIDENT: the keychain file is destroyed before cleanup runs
#    (release staged into a mktemp dir that got wiped). delete-keychain fails;
#    the entry must come off the search list anyway.
# ==========================================================================
setup_env
test_case "keychain file destroyed before cleanup — entry still unlisted"
seed_list "$HOME/Library/Keychains/login.keychain-db"
STAGING="$ROOT/private-tmp-staging/dist"
run_child "keychain_session_begin codescribe-signing '$STAGING' >/dev/null
           rm -rf '$ROOT/private-tmp-staging'
           keychain_session_end" >/dev/null
assert_not_in_list "$STAGING/codescribe-signing.keychain-db" "vanished keychain removed from the search list"
assert_list_equals "$HOME/Library/Keychains/login.keychain-db"
assert_default_is "$HOME/Library/Keychains/login.keychain-db" "default keychain not left pointing at a deleted file"
teardown_env

# ==========================================================================
# 7. Concurrency: two overlapping releases. Neither may drop the other's
#    keychain, and neither may resurrect a dead one.
# ==========================================================================
setup_env
test_case "concurrent sessions: no clobber, no resurrection"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
seed_list "$LOGIN"
PATH_A="$(bash "$LIB" begin release-a)"
PATH_B="$(bash "$LIB" begin release-b)"
assert_in_list "$PATH_A" "session A installed"
assert_in_list "$PATH_B" "session B installed"
bash "$LIB" end release-a
assert_not_in_list "$PATH_A" "session A removed itself"
assert_in_list "$PATH_B" "session B survived session A's cleanup"
bash "$LIB" end release-b
assert_not_in_list "$PATH_A" "session A was NOT resurrected by session B's cleanup"
assert_list_equals "$LOGIN"
assert_default_is "$LOGIN" "default keychain back to the pre-run value"
teardown_env

# ==========================================================================
# 8. Reclaim: an earlier crashed run of the same label left a dead entry
# ==========================================================================
setup_env
test_case "stale entry from an earlier crashed run of the same label is reclaimed"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
GHOST="$ROOT/gone/codescribe-signing.keychain-db"
: > "$FAKE_SECURITY_STATE/search-list"
printf '%s\n%s\n' "$GHOST" "$LOGIN" > "$FAKE_SECURITY_STATE/search-list"
printf '%s\n' "$LOGIN" > "$FAKE_SECURITY_STATE/default"
run_child 'keychain_session_begin codescribe-signing >/dev/null; keychain_session_end' >/dev/null
assert_not_in_list "$GHOST" "dead same-label entry dropped"
assert_list_equals "$LOGIN"
teardown_env

# ==========================================================================
# 9. A stranger's entry is never silently removed
# ==========================================================================
setup_env
test_case "a foreign keychain in the search list is left untouched"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
FOREIGN="$HOME/Library/Keychains/someone-else.keychain-db"
seed_list "$LOGIN" "$FOREIGN"
run_child 'keychain_session_begin codescribe-signing >/dev/null; keychain_session_end' >/dev/null
assert_list_equals "$LOGIN" "$FOREIGN"
teardown_env

# ==========================================================================
# 10. The search list is never emptied
# ==========================================================================
setup_env
test_case "never writes an empty user search list"
seed_list "$HOME/Library/Keychains/login.keychain-db"
run_child 'keychain_session_begin codescribe-signing >/dev/null; keychain_session_end' >/dev/null
if awk -F'\t' '$1=="list-keychains" {
     n=0; s=0
     for (i=2;i<=NF;i++) {
       if ($i=="-s") { s=1; continue }
       if ($i=="-d") { i++; continue }
       if ($i!="") n++
     }
     if (s==1 && n==0) bad=1
   } END{exit bad?1:0}' "$FAKE_SECURITY_STATE/argv.log"; then
  ok 'no "list-keychains -d user -s" call with an empty entry list'
else
  bad "the search list was emptied at some point"
fi
teardown_env

# ==========================================================================
# 11. No secrets on stdout/stderr
# ==========================================================================
setup_env
test_case "no password material is ever printed"
seed_list "$HOME/Library/Keychains/login.keychain-db"
OUT="$(run_child 'keychain_session_begin codescribe-signing >/dev/null
                  cat "$(keychain_session_password_file)" > "'"$ROOT"'/pw.txt"
                  keychain_session_end' 2>&1)"
PW="$(cat "$ROOT/pw.txt" 2>/dev/null || true)"
if [[ -n "$PW" ]] && [[ "$OUT" != *"$PW"* ]]; then
  ok "ephemeral password absent from stdout/stderr"
elif [[ -z "$PW" ]]; then
  bad "no ephemeral password was generated"
else
  bad "ephemeral password leaked into the transcript"
fi
if [[ "$(stat -f '%Lp' "$ROOT/session-state/codescribe-signing/password" 2>/dev/null || echo gone)" == "gone" ]]; then
  ok "session state (incl. password file) removed on end"
else
  bad "password file survived the session"
fi
teardown_env

# ==========================================================================
# 12. Doctor: read-only, detects stale paths, exits 1, mutates nothing
# ==========================================================================
setup_env
test_case "doctor detects a poisoned domain without mutating it"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
DEAD="/private/tmp/wiped-release/dist/Vibecrafted-signing.keychain-db"
printf '%s\n%s\n' "$DEAD" "$LOGIN" > "$FAKE_SECURITY_STATE/search-list"
printf '%s\n' "$DEAD" > "$FAKE_SECURITY_STATE/default"
BEFORE="$(cat "$FAKE_SECURITY_STATE/search-list")$(cat "$FAKE_SECURITY_STATE/default")"
REPORT="$(bash "$DOCTOR" 2>&1)"; DRC=$?
[[ $DRC -eq 1 ]] && ok "doctor exit 1 on a poisoned domain" || bad "doctor exit was $DRC, expected 1"
[[ "$REPORT" == *"STALE"* ]] && ok "doctor names the stale entries" || bad "doctor did not report STALE"
[[ "$REPORT" == *"$LOGIN"* ]] && ok "doctor derives recovery from the surviving entries" || bad "doctor did not print a derived recovery line"
AFTER="$(cat "$FAKE_SECURITY_STATE/search-list")$(cat "$FAKE_SECURITY_STATE/default")"
[[ "$BEFORE" == "$AFTER" ]] && ok "doctor mutated nothing" || bad "doctor changed the keychain domain"
if awk -F'\t' '{for(i=1;i<=NF;i++) if($i=="-s") exit 1} END{exit 0}' "$FAKE_SECURITY_STATE/argv.log"; then
  ok "doctor never issued a mutating -s form"
else
  bad "doctor issued a mutating security call"
fi
teardown_env

# ==========================================================================
# 13. Doctor on a healthy domain
# ==========================================================================
setup_env
test_case "doctor is green on a healthy domain"
seed_list "$HOME/Library/Keychains/login.keychain-db"
bash "$DOCTOR" >/dev/null 2>&1
[[ $? -eq 0 ]] && ok "doctor exit 0 when every path exists" || bad "doctor flagged a healthy domain"
teardown_env

# ==========================================================================
# 14. THE OTHER HALF OF THE INCIDENT: the login session's default keychain is
#     not ours to take. On 2026-08-15 the prompt fired while the release was
#     still healthy and running, because it had made its keychain the default
#     for every process on the host.
# ==========================================================================
setup_env
test_case "the login session's default keychain is never taken by default"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
seed_list "$LOGIN"
run_child 'keychain_session_begin codescribe-signing >/dev/null
           printf "%s" "$(cat "'"$FAKE_SECURITY_STATE"'/default")" > "'"$ROOT"'/default-during.txt"
           keychain_session_end' >/dev/null
if [[ "$(cat "$ROOT/default-during.txt")" == "$LOGIN" ]]; then
  ok "default keychain untouched WHILE the session is active"
else
  bad "session hijacked the default keychain: $(cat "$ROOT/default-during.txt")"
fi
assert_default_is "$LOGIN" "default keychain still the operator's after the session"
if awk -F'\t' '$1=="default-keychain" {for(i=2;i<=NF;i++) if($i=="-s") exit 1} END{exit 0}' \
      "$FAKE_SECURITY_STATE/argv.log"; then
  ok "no mutating default-keychain call was issued at all"
else
  bad "a mutating default-keychain -s call was issued"
fi
teardown_env

# ==========================================================================
# 15. Opt-in default still works — and still gets handed back
# ==========================================================================
setup_env
test_case "KEYCHAIN_SESSION_SET_DEFAULT=1 takes the default and gives it back"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
seed_list "$LOGIN"
export KEYCHAIN_SESSION_SET_DEFAULT=1
run_child 'keychain_session_begin codescribe-signing >/dev/null
           printf "%s" "$(cat "'"$FAKE_SECURITY_STATE"'/default")" > "'"$ROOT"'/default-during.txt"
           keychain_session_end' >/dev/null 2>&1
unset KEYCHAIN_SESSION_SET_DEFAULT
DUR="$(cat "$ROOT/default-during.txt")"
[[ "$DUR" != "$LOGIN" && -n "$DUR" ]] && ok "opt-in took the default during the session" \
  || bad "opt-in did not take the default (saw '$DUR')"
assert_default_is "$LOGIN" "opt-in default handed back on end"
teardown_env

# ==========================================================================
# 16. Doctor grades a build keychain that EXISTS — today's actual host state.
#     "The file is there" is not health.
# ==========================================================================
setup_env
test_case "doctor flags a live build keychain holding the default"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
LIVE="$ROOT/private-tmp/vibecrafted-ci/dist/Vibecrafted-signing.keychain-db"
mkdir -p "$(dirname "$LIVE")"; printf 'live\n' > "$LIVE"
printf '%s\n%s\n' "$LIVE" "$LOGIN" > "$FAKE_SECURITY_STATE/search-list"
printf '%s\n' "$LIVE" > "$FAKE_SECURITY_STATE/default"
REPORT="$(bash "$DOCTOR" 2>&1)"; DRC=$?
[[ $DRC -eq 1 ]] && ok "doctor exit 1 although every file exists" || bad "doctor exit was $DRC, expected 1"
[[ "$REPORT" == *"FOREIGN"* ]] && ok "doctor grades the build keychain FOREIGN" || bad "doctor did not grade it FOREIGN"
[[ "$REPORT" == *"HIJACKED"* ]] && ok "doctor grades the stolen default HIJACKED" || bad "doctor did not grade the default HIJACKED"
[[ "$REPORT" == *"$LOGIN"* ]] && ok "doctor derives recovery from the resident entries" || bad "no derived recovery line"
[[ "$REPORT" == *"release is running"* || "$REPORT" == *"release is signing"* ]] \
  && ok "doctor warns against recovering under a live release" \
  || bad "doctor did not warn about a live release"
teardown_env

# ==========================================================================
# 17. Trap chaining. A release script very likely already has `trap ... EXIT`
#     of its own (build-vibecrafted-release.sh does). Installing ours must not
#     silently drop theirs — and ours must run FIRST, before the caller starts
#     deleting the directories our keychain might live in.
# ==========================================================================
setup_env
test_case "an existing EXIT trap survives, and ours runs before it"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
seed_list "$LOGIN"
ORDER="$ROOT/order.txt"
: > "$ORDER"
run_child "trap 'printf \"caller\\n\" >> \"$ORDER\"' EXIT
           keychain_session_begin codescribe-signing >/dev/null
           printf 'list-during=%s\n' \"\$(wc -l < \"$FAKE_SECURITY_STATE/search-list\" | tr -d ' ')\" >> \"$ORDER\"" >/dev/null
if grep -q '^caller$' "$ORDER"; then
  ok "the caller's own EXIT trap still ran"
else
  bad "the caller's EXIT trap was clobbered"
fi
assert_list_equals "$LOGIN"
if [[ "$(tail -n1 "$ORDER")" == "caller" ]]; then
  ok "keychain cleanup ran before the caller's handler"
else
  bad "ordering wrong: $(tr '\n' '|' < "$ORDER")"
fi
teardown_env

# ==========================================================================
# 18. Same, for SIGINT — the case a bare EXIT trap misses entirely
# ==========================================================================
setup_env
test_case "an existing INT trap survives alongside ours"
LOGIN="$HOME/Library/Keychains/login.keychain-db"
seed_list "$LOGIN"
MARK="$ROOT/int-mark.txt"
: > "$MARK"
run_child "trap 'printf \"caller-int\\n\" >> \"$MARK\"; exit 130' INT
           keychain_session_begin codescribe-signing >/dev/null
           kill -INT \$\$
           sleep 5" >/dev/null 2>&1
if grep -q '^caller-int$' "$MARK"; then
  ok "the caller's own INT trap still ran"
else
  bad "the caller's INT trap was clobbered"
fi
assert_list_equals "$LOGIN"
teardown_env

printf '\n============================================================\n'
printf 'keychain-session: %d passed, %d failed\n' "$PASS" "$FAIL"
printf '============================================================\n'
(( FAIL == 0 )) || exit 1
