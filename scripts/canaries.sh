#!/bin/bash
# ============================================================================
# canaries.sh — the canary catalog: claims this repo makes vs. execution truth
# ============================================================================
# WHY
# ---
# The repo already keeps two machine-checked registries honest:
# ENV_REGISTRY.toml says which env vars EXIST (validate-envs.sh) and the GATE
# LEDGER says what each gate EXECUTES (validate-gates.sh). Neither checks
# whether a VALUE the repo claims is still the value the code runs. That gap
# is where every recent expensive surprise lived: docs/env.md and CHANGELOG
# said the idle-unload default was 300 s while the code ran 2700 s; the
# cold-load benchmark measured a hard-coded q8 path while the operator thought
# it measured the model under test; a locally cut release died on its LAST
# gate because SUPublicEDKey had no local source; the live corrections store
# was 61 % Swift-test fixtures.
#
# A canary here is a claim/execution pair born from a NAMED incident. It does
# not test features — `make verify` does that. It asks: "is the lie that once
# cost us a day still dead?" Rows follow the smoke-macos27.sh idiom: PASS,
# FAIL, SKIP (honest, never a failure), INFO (evidence, no verdict).
#
# CATALOG DISCIPLINE
# ------------------
# Every canary carries: an id, the LIE it watches for, and BORN — the date and
# incident that created it. A canary nobody can trace to an incident is noise;
# delete it. New incident → new canary, in the same commit as the fix when
# possible. `--list` prints the catalog without running anything.
#
# USAGE
#   scripts/canaries.sh            # hermetic canaries only (repo files, no host state)
#   scripts/canaries.sh --host     # + host canaries (secrets, network, live stores)
#   scripts/canaries.sh --list     # print the catalog, run nothing
#
# EXIT: 0 when no row FAILed · 1 otherwise. SKIP never fails the run.
#
# PORTABILITY: bash 3.2 (macOS /bin/bash) — same rule as validate-gates.sh.
#
# Created by Vetcoders (c)2026

set -eo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

HOST=0
LIST=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --host) HOST=1; shift ;;
        --list) LIST=1; shift ;;
        -h | --help) sed -n '3,38p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

FAILED=0
record() {
    local row="$1" status="$2" evidence="$3"
    [[ "$status" == "FAIL" ]] && FAILED=1
    printf '%-26s %-5s %s\n' "$row" "$status" "$evidence"
}

# One catalog entry per canary; `--list` reads these, the runner executes them.
# Format: id | class | lie | born
CATALOG="\
idle-default-truth|hermetic|docs claim one idle-unload default, the code runs another|2026-08-09: docs/env.md and CHANGELOG said 300 s, both singletons ran 2700 s — 805 s of reloads misattributed
measure-uses-product-path|hermetic|the latency benchmark measures a hard-coded model, not the one under test|2026-08-09: F16-vs-q8 comparison silently transcribed q8 because transcribe_file.rs ignored CODESCRIBE_MODEL_PATH
dist-inputs|host|a release chain discovers a missing key minutes (or one notarisation) too late|2026-08-09: 'VAR=x make a && make b' scoped the licence key to the first command; SUPublicEDKey failed AFTER notarisation
appcast-feed-live|host|the shipped SUFeedURL points at a feed that does not exist|2026-08-08: appcast never published — SUFeedURL 404, updater dead on arrival for every shipped build
store-purity|host|the test suite writes fixtures into the operator's live corrections store|2026-08-04 audit: 61 % of corrections.jsonl was Swift-test fixtures; leak closed by d2dec16a on 2026-08-09
sparkle-key-parity|host|the installed app verifies updates against a different key than the one we sign with|2026-08-09: local release had no SUPublicEDKey source at all; parity was unverifiable
keychain-domain-clean|host|a release borrowed the operator's user keychain domain and did not give it back|2026-08-15: the user search list AND the default keychain both pointed at /private/tmp/vibecrafted-release-3.7.1.*/dist/Vibecrafted-signing.keychain-db while build-vibecrafted-release.sh was still running — every app in the login session, Codescribe included, resolved there first and asked for a uuidgen password no human has"

if [[ "$LIST" == "1" ]]; then
    echo "== Canary catalog =="
    printf '%s\n' "$CATALOG" | while IFS='|' read -r id class lie born; do
        printf '%-26s %-9s %s\n' "$id" "$class" "$lie"
        printf '%-26s %-9s BORN %s\n' "" "" "$born"
    done
    exit 0
fi

echo "== Codescribe · canaries (claims vs. execution truth) =="
printf '%-26s %-5s %s\n' "CANARY" "STATE" "EVIDENCE"
echo "--------------------------------------------------------------------------"

# ---------------------------------------------------------------------------
# idle-default-truth (hermetic)
# Each residency default has one source of truth: its ENV_REGISTRY entry, its
# singleton constant, and its prose. Whisper and embedder may intentionally use
# different values, so this canary compares each component with its own contract.
# ---------------------------------------------------------------------------
canary_idle_default_truth() {
    local reg_whisper reg_embedder code_whisper code_embedder doc_claim
    reg_whisper="$(awk '/^\[vars\.CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS\]/{f=1;next} f&&/^default/{gsub(/[^0-9]/,"");print;exit}' docs/ENV_REGISTRY.toml)"
    reg_embedder="$(awk '/^\[vars\.CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS\]/{f=1;next} f&&/^default/{gsub(/[^0-9]/,"");print;exit}' docs/ENV_REGISTRY.toml)"
    code_whisper="$(sed -nE 's/^const DEFAULT_IDLE_UNLOAD_SECS: u64 = ([0-9]+);.*/\1/p' core/stt/whisper/singleton.rs | head -1)"
    code_embedder="$(sed -nE 's/^const DEFAULT_IDLE_UNLOAD_SECS: u64 = ([0-9]+);.*/\1/p' core/embedder/singleton.rs | head -1)"
    # docs/env.md states Whisper's default in prose; extract its numeric contract.
    doc_claim="$(sed -nE 's/.*WHISPER_IDLE_UNLOAD_SECS.*default[^0-9]*([0-9]+).*/\1/p' docs/env.md | head -1)"

    if [[ -z "$reg_whisper" || -z "$code_whisper" ]]; then
        record "idle-default-truth" "FAIL" "could not extract defaults (registry='$reg_whisper' code='$code_whisper') — the canary itself broke, fix the extraction"
        return
    fi
    if [[ "$reg_whisper" == "$code_whisper" && "$reg_embedder" == "$code_embedder" && "$doc_claim" == "$code_whisper" ]]; then
        record "idle-default-truth" "PASS" "whisper registry=$reg_whisper code=$code_whisper docs=$doc_claim; embedder registry=$reg_embedder code=$code_embedder — each residency default is coherent"
    else
        record "idle-default-truth" "FAIL" "registry=$reg_whisper/$reg_embedder code=$code_whisper/$code_embedder docs=${doc_claim:-none} — somebody changed one surface and not the others"
    fi
}

# ---------------------------------------------------------------------------
# measure-uses-product-path (hermetic)
# The instrument used for model benchmarks must honour the same override the
# product honours, or every A/B measurement silently measures the default.
# ---------------------------------------------------------------------------
canary_measure_uses_product_path() {
    if grep -q 'CODESCRIBE_MODEL_PATH' examples/transcribe_file.rs; then
        record "measure-uses-product-path" "PASS" "examples/transcribe_file.rs reads CODESCRIBE_MODEL_PATH before its hard-coded candidate list"
    else
        record "measure-uses-product-path" "FAIL" "examples/transcribe_file.rs ignores CODESCRIBE_MODEL_PATH — benchmarks compare the default model against itself"
    fi
}

# ---------------------------------------------------------------------------
# dist-inputs (host)
# The whole point of dist-preflight-signed is to be run BEFORE money is spent;
# this row keeps it honest as a standing readiness signal, not just a recipe dep.
# ---------------------------------------------------------------------------
canary_dist_inputs() {
    local out
    if out="$(make -s dist-preflight-signed 2>&1)"; then
        record "dist-inputs" "PASS" "licence key + Developer ID + Sparkle key all resolve locally"
    else
        record "dist-inputs" "FAIL" "$(printf '%s' "$out" | grep -m1 'ERROR' || echo 'dist-preflight-signed failed')"
    fi
}

# ---------------------------------------------------------------------------
# appcast-feed-live (host, network)
# SUFeedURL is baked into every shipped bundle; a 404 there means the updater
# is dead for every user while all local gates stay green.
# ---------------------------------------------------------------------------
canary_appcast_feed_live() {
    local feed http
    feed="$(sed -nE 's/^ *SUFeedURL: *([^ ]+).*/\1/p' macos/project.yml | head -1)"
    if [[ -z "$feed" ]]; then
        record "appcast-feed-live" "FAIL" "no SUFeedURL in macos/project.yml — the bundle would ship without a feed"
        return
    fi
    http="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 -I "$feed" 2>/dev/null || echo 000)"
    case "$http" in
        200) record "appcast-feed-live" "PASS" "$feed → 200" ;;
        000) record "appcast-feed-live" "SKIP" "no network reply for $feed — cannot judge the feed from here" ;;
        *) record "appcast-feed-live" "FAIL" "$feed → $http — every shipped build carries this URL and the updater fail-closes on it" ;;
    esac
}

# ---------------------------------------------------------------------------
# store-purity (host)
# Fixture rows written BEFORE the d2dec16a fix are history and stay; the
# canary fails only on fixture-marker rows stamped after the fix landed —
# that is the leak reopening, not the old wound.
# ---------------------------------------------------------------------------
canary_store_purity() {
    local store="$HOME/.codescribe/quality/corrections.jsonl"
    # 2026-08-09T15:00:00Z, epoch ms — after d2dec16a (test-suite leak fix) landed.
    local fix_epoch_ms=1786287600000
    if [[ ! -f "$store" ]]; then
        record "store-purity" "SKIP" "no live corrections store at $store"
        return
    fi
    local fresh
    fresh="$(awk -v cut="$fix_epoch_ms" '
        /uni agentka here|Junie here|integration-fixture/ {
            if (match($0, /"timestamp_ms":[0-9]+/)) {
                ts = substr($0, RSTART + 16, RLENGTH - 16) + 0
                if (ts > cut) n++
            }
        }
        END { print n + 0 }' "$store")"
    if [[ "$fresh" == "0" ]]; then
        record "store-purity" "PASS" "no fixture-marker rows newer than the d2dec16a fix (store: $(wc -l < "$store" | tr -d ' ') rows total)"
    else
        record "store-purity" "FAIL" "$fresh fixture-marker row(s) written AFTER the leak fix — the test suite is writing into the live store again"
    fi
}

# ---------------------------------------------------------------------------
# sparkle-key-parity (host)
# The key we sign updates with must be the key the installed app verifies with.
# ---------------------------------------------------------------------------
canary_sparkle_key_parity() {
    local keyfile="$HOME/.vibecrafted/secrets/codescribe/sparkle-public.b64"
    local app="" bundle_key local_key
    for candidate in "/Applications/Codescribe.app" "$HOME/Applications/Codescribe.app"; do
        [[ -d "$candidate" ]] && { app="$candidate"; break; }
    done
    if [[ ! -f "$keyfile" ]]; then
        record "sparkle-key-parity" "FAIL" "no local Sparkle public key at $keyfile — dist-inputs should have caught this"
        return
    fi
    if [[ -z "$app" ]]; then
        record "sparkle-key-parity" "SKIP" "no installed Codescribe.app to compare against"
        return
    fi
    local_key="$(tr -d '[:space:]' < "$keyfile")"
    bundle_key="$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$app/Contents/Info.plist" 2>/dev/null || echo '')"
    if [[ -z "$bundle_key" ]]; then
        record "sparkle-key-parity" "SKIP" "installed app has empty SUPublicEDKey (pre-canary local build) — parity judgeable after the next keyed install"
    elif [[ "$bundle_key" == "$local_key" ]]; then
        record "sparkle-key-parity" "PASS" "installed app verifies with the key we sign with"
    else
        record "sparkle-key-parity" "FAIL" "installed app key differs from $keyfile — its updates would reject our signatures"
    fi
}

# ---------------------------------------------------------------------------
# keychain-domain-clean (host)
# A signing run borrows the operator's user keychain domain: it prepends an
# ephemeral keychain to the search list and makes it the default. If it does
# not hand both back, every later keychain access on this host asks for a
# password to a file that may no longer exist. scripts/keychain-doctor.sh is
# the read-only judge; this row is the standing alarm. It never mutates.
# ---------------------------------------------------------------------------
canary_keychain_domain_clean() {
    local out rc=0
    # `out="$(cmd)"; rc=$?` never reaches the second statement under `set -e`:
    # the assignment takes the substitution's exit status and aborts the whole
    # canary run. The FAIL branch below would have been unreachable — the exact
    # class of silent hole these canaries exist to catch.
    out="$(bash scripts/keychain-doctor.sh 2>&1)" || rc=$?
    case "$rc" in
        0) record "keychain-domain-clean" "PASS" "search list holds only the operator's own keychains and the default is one of them — no build keychain borrowed the domain" ;;
        1) record "keychain-domain-clean" "FAIL" "$(printf '%s' "$out" | awk '/STALE|FOREIGN|HIJACKED/{print tolower($1) ": " $2; exit}') — run scripts/keychain-doctor.sh for the derived recovery line" ;;
        *) record "keychain-domain-clean" "SKIP" "keychain domain unreadable (no /usr/bin/security?) — not a macOS host" ;;
    esac
}

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------
canary_idle_default_truth
canary_measure_uses_product_path

if [[ "$HOST" == "1" ]]; then
    canary_dist_inputs
    canary_appcast_feed_live
    canary_store_purity
    canary_sparkle_key_parity
    canary_keychain_domain_clean
else
    record "host-canaries" "SKIP" "run with --host for dist-inputs, appcast-feed-live, store-purity, sparkle-key-parity, keychain-domain-clean"
fi

echo "--------------------------------------------------------------------------"
if [[ "$FAILED" == "1" ]]; then
    echo "canaries: at least one claim no longer matches execution truth."
    exit 1
fi
echo "canaries: every watched claim still matches execution truth."
