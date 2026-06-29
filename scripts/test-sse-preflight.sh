#!/bin/bash
# Guard real SSE tests from starting while the machine is already under
# enough pressure that a release cargo test can make macOS unresponsive.

set -euo pipefail

MAX_LOAD="${CODESCRIBE_TEST_MAX_LOAD:-32}"
MIN_FREE_GIB="${CODESCRIBE_TEST_MIN_FREE_GIB:-4}"
ALLOW_BUSY="${CODESCRIBE_ALLOW_BUSY_TESTS:-0}"

echo "SSE preflight: checking machine pressure..."

load_1m="$(sysctl -n vm.loadavg | awk '{ gsub(/[{}]/, ""); print $1 }')"
free_pages="$(vm_stat | awk -F: '/Pages free/ { gsub(/[^0-9]/, "", $2); print $2 }')"
page_size="$(vm_stat | awk '/page size of/ { gsub(/[^0-9]/, "", $8); print $8 }')"

free_bytes=$((free_pages * page_size))
free_gib=$((free_bytes / 1024 / 1024 / 1024))

echo "SSE preflight: load_1m=${load_1m}, free_gib=${free_gib}, cargo_jobs=${CARGO_BUILD_JOBS:-unset}"

if awk "BEGIN { exit !(${load_1m} > ${MAX_LOAD}) }"; then
  echo "SSE preflight refused: load_1m=${load_1m} > CODESCRIBE_TEST_MAX_LOAD=${MAX_LOAD}" >&2
  exit 1
fi

if (( free_gib < MIN_FREE_GIB )); then
  echo "SSE preflight refused: free_gib=${free_gib} < CODESCRIBE_TEST_MIN_FREE_GIB=${MIN_FREE_GIB}" >&2
  exit 1
fi

if [[ "${TEST_SSE_PROFILE:-debug}" == "release" && "${CODESCRIBE_ALLOW_RELEASE_SSE:-0}" != "1" ]]; then
  echo "SSE preflight refused: release SSE requires CODESCRIBE_ALLOW_RELEASE_SSE=1" >&2
  exit 1
fi

if [[ "${ALLOW_BUSY}" != "1" ]]; then
  busy="$(ps -axo command | grep -E '(^|/)cargo (test|build)|(^|/)rustc ' | grep -v 'grep -E' || true)"
  if [[ -n "${busy}" ]]; then
    echo "SSE preflight refused: another cargo/rustc job is active." >&2
    echo "${busy}" >&2
    echo "Set CODESCRIBE_ALLOW_BUSY_TESTS=1 only if you intentionally want overlap." >&2
    exit 1
  fi
fi

echo "SSE preflight: ok"
