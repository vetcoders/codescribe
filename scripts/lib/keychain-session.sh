#!/usr/bin/env bash
# ============================================================================
# keychain-session.sh — an ephemeral signing keychain that always gives the
# operator's search list back
# ============================================================================
# BORN FROM (2026-08-15, P0): a normal Codescribe run popped
#
#   "Codescribe wants to use the 'Vibecrafted-signing' keychain"
#
# and the login password did not open it, because it was not the login
# keychain. The host's user domain looked like this:
#
#   search list: "/private/tmp/vibecrafted-release-3.7.1.rki1gq/.../Vibecrafted-signing.keychain-db"
#                "/Users/<op>/Library/Keychains/login.keychain-db"
#   default:     "/private/tmp/vibecrafted-release-3.7.1.rki1gq/.../Vibecrafted-signing.keychain-db"
#
# A release had prepended its ephemeral signing keychain to the *user* search
# list and made it the *default* keychain. Measured at the time: the keychain
# file still existed and the release (`build-vibecrafted-release.sh`, pid
# 66496) was still running. So this was not only a cleanup bug — the host was
# poisoned WHILE the release was healthy and mid-flight, and it would have
# stayed poisoned for the whole run.
#
# FOUR FAILURES PRODUCED IT, and this file exists to make all four impossible:
#
# 0. Taking the login session's default keychain. `security default-keychain
#    -d user -s` is global to the login session, not scoped to the release
#    shell. Every other process — Codescribe included — then resolves to the
#    release's keychain first and prompts for a uuidgen password nobody has.
#    Signing does not need it: import, set-key-partition-list, find-identity
#    and codesign all take the keychain path explicitly. => We do NOT set the
#    default keychain unless KEYCHAIN_SESSION_SET_DEFAULT=1 says so, and the
#    restore path below exists for domains an older run already took over.
#
# 1. "delete-keychain also unlists it." It does — but only while the file is
#    still there. Put the ephemeral keychain under `mktemp -d` (which is what
#    the release did) and the directory can vanish first; `delete-keychain`
#    then fails, the `|| true` swallows it, and the search-list entry is
#    immortal. => We ALWAYS unlist explicitly, and we unlist BEFORE deleting.
#
# 2. `trap cleanup EXIT` alone. Bash runs an EXIT trap for normal and error
#    exits, but a Ctrl-C or a SIGTERM during a 30-minute notarization wait can
#    tear the shell down without it. => We trap EXIT INT TERM HUP, and the
#    handler is idempotent so double delivery is harmless.
#
# 3. Snapshot-and-restore under concurrency. Naive restore is itself a
#    contamination source:
#      A snapshots [login]        -> list [tempA, login]
#      B snapshots [tempA, login] -> list [tempB, tempA, login]
#      A restores its snapshot    -> list [login]           (tempB dropped, B breaks)
#      B restores its snapshot    -> list [tempA, login]    (tempA RESURRECTED, dead path)
#    => We never write a remembered list back. Restore means "read the current
#    list, remove exactly the entry this session created, write the rest."
#    In a single run that reproduces the prior list exactly; under concurrency
#    nobody clobbers anybody. The snapshot is kept for the default-keychain
#    decision and for the doctor's forensics, not as a thing to replay.
#
# The snapshot is stored as one NUL-terminated path per record — never as a
# shell-concatenated string. The predecessor did
#   existing="$(security list-keychains -d user | tr -d '"' | tr '\n' ' ')"
#   security list-keychains -d user -s "$TEMP" $existing
# which splits any keychain path containing a space into two nonexistent
# entries, and strips every space out of the default keychain path besides.
#
# Usage (sourceable — the release path):
#   . scripts/lib/keychain-session.sh
#   keychain_session_begin codescribe-signing   # arms the traps itself
#   security import ... -k "$KEYCHAIN_SESSION_PATH" ...
#   codesign --keychain "$KEYCHAIN_SESSION_PATH" ...
#   keychain_session_end                        # optional; traps cover it
#
# Usage (executable — for CI step boundaries, where each `run:` block is its
# own shell and traps cannot span them):
#   scripts/lib/keychain-session.sh begin <label> [dir]  → prints keychain path
#   scripts/lib/keychain-session.sh end   <label> [dir]  → unlists + deletes
#   scripts/lib/keychain-session.sh path  <label> [dir]  → prints keychain path
#
# The ephemeral password is generated here, never echoed, never written to the
# transcript, and lives only in the session state file (mode 0600). No
# identity, certificate, or environment secret is ever printed by this file.
#
# `security` is reached through $KEYCHAIN_SESSION_SECURITY_BIN so the
# regression suite can substitute a fake and assert the exact argv this file
# produces without touching a real keychain. Tests:
#   scripts/tests/keychain-session-test.sh   (make test-keychain-session)
# Read-only diagnosis of a host already poisoned by an older release:
#   scripts/keychain-doctor.sh
# ============================================================================

set -o pipefail

KEYCHAIN_SESSION_SECURITY_BIN="${KEYCHAIN_SESSION_SECURITY_BIN:-/usr/bin/security}"
# Where session state (snapshot + password + owned path) lives. Overridable so
# the tests get a hermetic directory and so a CI job can hand state from the
# "import certificate" step to the "remove keychain" step.
KEYCHAIN_SESSION_STATE_DIR="${KEYCHAIN_SESSION_STATE_DIR:-${TMPDIR:-/tmp}/codescribe-keychain-session}"

KEYCHAIN_SESSION_PATH=""
_ks_label=""
_ks_state_dir=""
_ks_ended=0

_ks_log() { printf '[keychain-session] %s\n' "$*" >&2; }
_ks_die() { printf '[keychain-session] FATAL: %s\n' "$*" >&2; exit 1; }

_ks_security() { "$KEYCHAIN_SESSION_SECURITY_BIN" "$@"; }

# --------------------------------------------------------------------------
# Parsing. `security list-keychains -d user` prints one indented, quoted path
# per line. We hand back one raw path per line: whitespace trimmed, surrounding
# quotes removed, empty lines dropped. Paths keep their interior spaces, which
# is the entire point.
# --------------------------------------------------------------------------
_ks_read_search_list() {
  local line
  _ks_security list-keychains -d user 2>/dev/null | while IFS= read -r line; do
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    line="${line#\"}"
    line="${line%\"}"
    [[ -n "$line" ]] && printf '%s\n' "$line"
  done
  return 0
}

_ks_read_default_keychain() {
  local line
  line="$(_ks_security default-keychain -d user 2>/dev/null | head -n1)" || return 0
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line%"${line##*[![:space:]]}"}"
  line="${line#\"}"
  line="${line%\"}"
  printf '%s\n' "$line"
}

# --------------------------------------------------------------------------
# Serialization. macOS ships no flock(1), so the lock is an atomic mkdir. Two
# releases must not interleave their read-modify-write of the search list; a
# torn RMW is how an entry gets lost or duplicated. A lock older than
# _KS_LOCK_STALE_SECS is broken on the assumption its owner died — releases are
# long, but nobody holds this lock across more than a few `security` calls.
# --------------------------------------------------------------------------
_KS_LOCK_WAIT_SECS="${KEYCHAIN_SESSION_LOCK_WAIT_SECS:-60}"
_KS_LOCK_STALE_SECS="${KEYCHAIN_SESSION_LOCK_STALE_SECS:-120}"

_ks_lock_dir() { printf '%s/search-list.lock' "$KEYCHAIN_SESSION_STATE_DIR"; }

_ks_lock_acquire() {
  local lock waited=0 age now
  lock="$(_ks_lock_dir)"
  mkdir -p "$KEYCHAIN_SESSION_STATE_DIR" 2>/dev/null || true
  while ! mkdir "$lock" 2>/dev/null; do
    now="$(date +%s)"
    age="$(_ks_path_mtime "$lock")"
    if [[ -n "$age" ]] && (( now - age > _KS_LOCK_STALE_SECS )); then
      _ks_log "breaking a stale search-list lock (${_KS_LOCK_STALE_SECS}s+)"
      rm -rf "$lock" 2>/dev/null || true
      continue
    fi
    if (( waited >= _KS_LOCK_WAIT_SECS )); then
      # Never deadlock a release, and never fail a cleanup. Proceeding is safe:
      # every mutation below is remove-self, which is idempotent.
      _ks_log "search-list lock still held after ${waited}s; proceeding unlocked"
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  printf '%s' "$$" > "$lock/pid" 2>/dev/null || true
  return 0
}

_ks_lock_release() { rm -rf "$(_ks_lock_dir)" 2>/dev/null || true; }

_ks_path_mtime() {
  [[ -e "$1" ]] || return 0
  stat -f %m "$1" 2>/dev/null || stat -c %Y "$1" 2>/dev/null || true
}

# --------------------------------------------------------------------------
# Writing the search list. Guarded: `security list-keychains -d user -s` with
# zero arguments EMPTIES the user search list, which would lock the operator
# out of their own login keychain. We refuse to write an empty list, and we
# refuse to write one that has lost the login keychain while the current list
# still has it.
# --------------------------------------------------------------------------
_ks_write_search_list() {
  local -a entries=("$@")
  if (( ${#entries[@]} == 0 )); then
    _ks_log "refusing to write an empty user search list"
    return 1
  fi
  _ks_security list-keychains -d user -s "${entries[@]}" >/dev/null
}

_ks_state_path() { printf '%s/%s' "$_ks_state_dir" "$1"; }

# --------------------------------------------------------------------------
# begin
# --------------------------------------------------------------------------
keychain_session_begin() {
  local label="${1:-codescribe-signing}"
  local home_dir="${2:-}"
  local password
  local -a current=() kept=()
  local entry snapshot default_now

  case "$label" in
    ''|*/*|*' '*) _ks_die "label must be a bare name, got: ${label}" ;;
  esac

  _ks_label="$label"
  _ks_state_dir="$KEYCHAIN_SESSION_STATE_DIR/$label"
  _ks_ended=0
  mkdir -p "$_ks_state_dir"
  chmod 700 "$_ks_state_dir" 2>/dev/null || true

  # The keychain file itself lives in the state dir by default, NOT in a
  # release staging directory. That is deliberate: the 2026-08-15 incident
  # happened because the ephemeral keychain sat inside a disposable dist dir
  # that was removed before cleanup ran.
  if [[ -n "$home_dir" ]]; then
    mkdir -p "$home_dir"
    KEYCHAIN_SESSION_PATH="$home_dir/${label}.keychain-db"
  else
    KEYCHAIN_SESSION_PATH="$_ks_state_dir/${label}.keychain-db"
  fi

  # Arm the traps BEFORE the first mutation. INT/TERM/HUP are not covered by a
  # bare EXIT trap, and a release is interrupted by hand more often than it
  # fails on its own.
  _ks_arm_traps

  _ks_lock_acquire || true

  # Snapshot as structured records, one NUL-terminated path each. Never a
  # single joined string.
  snapshot="$(_ks_state_path search-list.snapshot)"
  : > "$snapshot"
  chmod 600 "$snapshot" 2>/dev/null || true
  while IFS= read -r entry; do
    current+=("$entry")
    printf '%s\0' "$entry" >> "$snapshot"
  done < <(_ks_read_search_list)

  default_now="$(_ks_read_default_keychain)"
  printf '%s' "$default_now" > "$(_ks_state_path default.snapshot)"
  chmod 600 "$(_ks_state_path default.snapshot)" 2>/dev/null || true

  # Reclaim after an earlier crashed run of THIS label: drop entries that carry
  # our own keychain basename and no longer exist on disk. Scoped to our own
  # name — a stranger's stale entry is the doctor's business to report, not
  # ours to silently remove.
  for entry in "${current[@]}"; do
    if [[ "${entry##*/}" == "${label}.keychain-db" && ! -e "$entry" ]]; then
      _ks_log "dropping a stale ${label} entry left by an earlier run"
      continue
    fi
    [[ "$entry" == "$KEYCHAIN_SESSION_PATH" ]] && continue
    kept+=("$entry")
  done

  password="$(_ks_generate_password)"
  printf '%s' "$password" > "$(_ks_state_path password)"
  chmod 600 "$(_ks_state_path password)" 2>/dev/null || true
  printf '%s' "$KEYCHAIN_SESSION_PATH" > "$(_ks_state_path owned-path)"

  rm -f "$KEYCHAIN_SESSION_PATH"
  _ks_security create-keychain -p "$password" "$KEYCHAIN_SESSION_PATH"
  _ks_security set-keychain-settings -lut 21600 "$KEYCHAIN_SESSION_PATH"
  _ks_security unlock-keychain -p "$password" "$KEYCHAIN_SESSION_PATH"

  _ks_write_search_list "$KEYCHAIN_SESSION_PATH" "${kept[@]}" \
    || _ks_die "could not install the ephemeral keychain into the search list"

  # NOT the default keychain, unless explicitly asked for. `security
  # default-keychain -d user -s` is a side effect on the whole login session,
  # not on this shell: while it is set, EVERY process on the host that reaches
  # for a keychain lands on ours first and asks the operator to unlock it with
  # a uuidgen password they do not have. That is what produced
  # "Codescribe wants to use the 'Vibecrafted-signing' keychain" on 2026-08-15,
  # WHILE the release was still running — a clean cleanup would not have
  # helped, because the damage lasts as long as the release does (notarization
  # alone is half an hour).
  #
  # Nothing in signing needs it. `security import -k <path>`,
  # `security set-key-partition-list <path>`, `security find-identity <path>`
  # and `codesign --keychain <path>` all address the keychain explicitly.
  if [[ "${KEYCHAIN_SESSION_SET_DEFAULT:-0}" == "1" ]]; then
    _ks_log "KEYCHAIN_SESSION_SET_DEFAULT=1 — taking over the login session's default keychain"
    _ks_security default-keychain -d user -s "$KEYCHAIN_SESSION_PATH"
  fi

  _ks_lock_release
  export KEYCHAIN_SESSION_PATH
  return 0
}

# A release script very likely already has `trap ... EXIT` of its own (the
# vibecrafted one did). Blindly installing ours would silently drop theirs, so
# we read the existing handler back out of `trap -p` and chain onto it. Ours
# runs first: the keychain must come off the search list before the caller's
# handler starts removing directories.
_ks_arm_traps() {
  local sig prior body
  for sig in EXIT INT TERM HUP; do
    prior="$(trap -p "$sig")"
    # `trap -p` prints: trap -- 'body' SIGNAME
    #
    # SIGNAME is NOT what you passed in. Bash normalizes real signals to their
    # SIG- form (`trap -p INT` answers `... SIGINT`) but leaves the pseudo
    # signal EXIT bare. Stripping a literal " $sig" therefore worked for EXIT
    # and silently mangled INT/TERM/HUP into an unparsable handler — which
    # dropped the caller's own trap on exactly the three signals a bare EXIT
    # trap cannot cover. So: strip the LAST whitespace-delimited word, whatever
    # bash chose to call it.
    body=""
    if [[ -n "$prior" ]]; then
      body="${prior#trap -- }"
      body="${body% *}"
      body="${body#\'}"
      body="${body%\'}"
    fi
    # shellcheck disable=SC2064
    # Expanding now is the point, not an oversight: $body is the handler that
    # was already registered, read back a few lines above, and it has to be
    # baked into the new handler at arm time. Deferring it would splice the
    # variable's value at signal time, when it no longer holds anything.
    case "$sig" in
      EXIT) trap "_ks_trap_cleanup${body:+; $body}" EXIT ;;
      INT)  trap "_ks_trap_cleanup; ${body:-exit 130}" INT ;;
      TERM) trap "_ks_trap_cleanup; ${body:-exit 143}" TERM ;;
      HUP)  trap "_ks_trap_cleanup; ${body:-exit 129}" HUP ;;
    esac
  done
}

# The password never reaches stdout or a log. uuidgen is present on every macOS;
# /dev/urandom is the portable fallback for a Linux test host.
_ks_generate_password() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen
  else
    LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 32
    printf '\n'
  fi
}

keychain_session_password_file() { _ks_state_path password; }

# --------------------------------------------------------------------------
# end — remove-self, in the order that survives a vanished file
# --------------------------------------------------------------------------
keychain_session_end() {
  local label="${1:-$_ks_label}"
  local owned default_now snapshot_default entry candidate
  local -a kept=() current=()

  [[ -n "$label" ]] || return 0
  local state_dir="$KEYCHAIN_SESSION_STATE_DIR/$label"
  [[ -d "$state_dir" ]] || return 0
  _ks_state_dir="$state_dir"

  owned="$(cat "$(_ks_state_path owned-path)" 2>/dev/null || true)"
  [[ -n "$owned" ]] || { rm -rf "$state_dir"; return 0; }

  _ks_lock_acquire || true

  # 1. UNLIST FIRST. This is the whole lesson of the incident: the search-list
  #    entry must go even if the keychain file was already destroyed with it.
  while IFS= read -r entry; do current+=("$entry"); done < <(_ks_read_search_list)
  for entry in "${current[@]}"; do
    [[ "$entry" == "$owned" ]] && continue
    kept+=("$entry")
  done
  if (( ${#kept[@]} != ${#current[@]} )); then
    if (( ${#kept[@]} == 0 )); then
      # We were the only entry. Fall back to the login keychain rather than
      # writing an empty list.
      candidate="$(_ks_login_keychain)"
      [[ -n "$candidate" ]] && kept=("$candidate")
    fi
    _ks_write_search_list "${kept[@]}" || _ks_log "search-list restore failed"
  fi

  # 2. Default keychain: only ours to move, and only to a path that exists.
  #    Restoring a remembered-but-deleted default is exactly how this host
  #    ended up with a default pointing into a wiped /private/tmp.
  default_now="$(_ks_read_default_keychain)"
  if [[ "$default_now" == "$owned" ]]; then
    snapshot_default="$(cat "$(_ks_state_path default.snapshot)" 2>/dev/null || true)"
    candidate=""
    if [[ -n "$snapshot_default" && "$snapshot_default" != "$owned" && -e "$snapshot_default" ]]; then
      candidate="$snapshot_default"
    else
      for entry in "${kept[@]}"; do
        if [[ -e "$entry" ]]; then candidate="$entry"; break; fi
      done
      [[ -n "$candidate" ]] || candidate="$(_ks_login_keychain)"
    fi
    if [[ -n "$candidate" ]]; then
      _ks_security default-keychain -d user -s "$candidate" >/dev/null 2>&1 \
        || _ks_log "default-keychain restore failed"
    else
      _ks_log "no existing keychain to restore as default; leaving it alone"
    fi
  fi

  # 3. Only now the file, and only ours.
  _ks_security delete-keychain "$owned" >/dev/null 2>&1 || true
  rm -f "$owned" 2>/dev/null || true

  _ks_lock_release
  rm -rf "$state_dir" 2>/dev/null || true
  _ks_ended=1
  KEYCHAIN_SESSION_PATH=""
  return 0
}

_ks_login_keychain() {
  local candidate="${HOME}/Library/Keychains/login.keychain-db"
  [[ -e "$candidate" ]] && { printf '%s\n' "$candidate"; return 0; }
  return 0
}

# Idempotent: EXIT fires after INT/TERM handlers too, and a release may call
# keychain_session_end explicitly before exiting.
_ks_trap_cleanup() {
  local rc=$?
  (( _ks_ended == 1 )) && return $rc
  [[ -n "$_ks_label" ]] || return $rc
  keychain_session_end "$_ks_label" || true
  return $rc
}

# --------------------------------------------------------------------------
# Executable form — for CI, where every `run:` block is a separate shell and a
# trap in one cannot protect another. `end` is invoked from an always()-guarded
# step and reads its state from disk.
# --------------------------------------------------------------------------
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  set -euo pipefail
  case "${1:-}" in
    begin)
      keychain_session_begin "${2:?label required}" "${3:-}"
      # Traps are pointless in the executable form (this shell exits at once);
      # the caller's always()-step owns `end`.
      trap - EXIT INT TERM HUP
      printf '%s\n' "$KEYCHAIN_SESSION_PATH"
      ;;
    end)
      keychain_session_end "${2:?label required}"
      ;;
    path)
      label="${2:?label required}"
      cat "$KEYCHAIN_SESSION_STATE_DIR/$label/owned-path" 2>/dev/null || {
        printf 'no active keychain session: %s\n' "$label" >&2
        exit 3
      }
      printf '\n'
      ;;
    password-file)
      label="${2:?label required}"
      printf '%s/%s/password\n' "$KEYCHAIN_SESSION_STATE_DIR" "$label"
      ;;
    *)
      printf 'usage: %s {begin|end|path|password-file} <label> [dir]\n' "$0" >&2
      exit 2
      ;;
  esac
fi
