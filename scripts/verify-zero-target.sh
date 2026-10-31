#!/usr/bin/env bash
# verify-zero-target.sh — disk hygiene guard for fleet worktrees.
#
# Worktrees build into the main checkout's shared target (see Makefile), so a
# `target/` left inside a worktree is stray residue: 4-18 GiB each, and a fleet
# of 36 worktrees is enough to hit ENOSPC.
#
#   --check   exit 1 when this worktree keeps a `target/` above the threshold,
#             or when orphaned /tmp/cs-*-target-* dirs older than 30 minutes exist.
#   --sweep   remove `target/` from fleet worktrees (and stale /tmp orphans)
#             that no running cargo/rustc process is using.
#
# The main checkout's `target/` is the shared build cache and is never touched.
# Overrides (tests): VZT_WORKTREES_ROOT, VZT_TMP_ROOT, VZT_THRESHOLD_MIB,
# VZT_ORPHAN_MIN_AGE_MIN.
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

set -euo pipefail

WORKTREES_ROOT="${VZT_WORKTREES_ROOT:-$HOME/.vibecrafted/worktrees/vetcoders/codescribe}"
TMP_ROOT="${VZT_TMP_ROOT:-/tmp}"
THRESHOLD_MIB="${VZT_THRESHOLD_MIB:-10}"
ORPHAN_MIN_AGE_MIN="${VZT_ORPHAN_MIN_AGE_MIN:-30}"

usage() {
	echo "usage: $0 --check | --sweep" >&2
	exit 2
}

size_mib() {
	# du -sk is portable across macOS and Linux; round up to whole MiB.
	local kib
	kib="$(du -sk "$1" 2>/dev/null | awk '{print $1}')"
	echo $(((${kib:-0} + 1023) / 1024))
}

# True when a running cargo/rustc has its cwd or command line inside $1.
target_in_use() {
	local dir real pid cwd
	dir="$1"
	real="$(cd -P "$dir" 2>/dev/null && pwd -P || echo "$dir")"
	while read -r pid; do
		[[ -n "$pid" ]] || continue
		cwd="$(lsof -a -p "$pid" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -n1)"
		if [[ "$cwd" == "$real"* ]]; then
			return 0
		fi
		if ps -o command= -p "$pid" 2>/dev/null | grep -qF -- "$dir"; then
			return 0
		fi
	done < <(pgrep -x cargo 2>/dev/null; pgrep -x rustc 2>/dev/null)
	return 1
}

# A target dir whose cargo/rustc env points at it is also in use.
tmp_target_in_use() {
	local dir="$1" pid
	while read -r pid; do
		[[ -n "$pid" ]] || continue
		if ps -E -o command= -p "$pid" 2>/dev/null | grep -qF -- "$dir"; then
			return 0
		fi
	done < <(pgrep -x cargo 2>/dev/null; pgrep -x rustc 2>/dev/null)
	return 1
}

is_main_checkout() {
	local git_dir common_dir
	git_dir="$(git rev-parse --absolute-git-dir 2>/dev/null)" || return 1
	common_dir="$(cd "$(git rev-parse --git-common-dir)" && pwd -P)"
	[[ "$(cd "$git_dir" && pwd -P)" == "$common_dir" ]]
}

stale_tmp_orphans() {
	[[ -d "$TMP_ROOT" ]] || return 0
	find "$TMP_ROOT" -maxdepth 1 -type d -name 'cs-*-target-*' -mmin "+$ORPHAN_MIN_AGE_MIN" 2>/dev/null
}

cmd_check() {
	local fail=0 root mib orphan
	root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

	if is_main_checkout; then
		echo "check: main checkout — ./target is the shared build cache, skipped"
	elif [[ -d "$root/target" ]]; then
		mib="$(size_mib "$root/target")"
		if ((mib > THRESHOLD_MIB)); then
			echo "FAIL: $root/target holds ${mib} MiB (limit ${THRESHOLD_MIB} MiB) — rm -rf it or run make clean-worktree-targets" >&2
			fail=1
		else
			echo "check: $root/target ${mib} MiB (<= ${THRESHOLD_MIB} MiB)"
		fi
	else
		echo "check: $root/target absent (0 B)"
	fi

	while IFS= read -r orphan; do
		[[ -n "$orphan" ]] || continue
		echo "FAIL: orphaned ${orphan} ($(size_mib "$orphan") MiB, older than ${ORPHAN_MIN_AGE_MIN} min)" >&2
		fail=1
	done < <(stale_tmp_orphans)

	return "$fail"
}

cmd_sweep() {
	local freed=0 t wt mib orphan
	if [[ -d "$WORKTREES_ROOT" ]]; then
		while IFS= read -r t; do
			wt="$(dirname "$t")"
			if target_in_use "$wt"; then
				echo "skip (cargo/rustc active): $t"
				continue
			fi
			mib="$(size_mib "$t")"
			rm -rf -- "$t"
			echo "swept: $t (${mib} MiB)"
			freed=$((freed + mib))
		done < <(find "$WORKTREES_ROOT" -mindepth 3 -maxdepth 3 -type d -name target -not -path '*/node_modules/*' 2>/dev/null)
	else
		echo "sweep: no worktree root at $WORKTREES_ROOT"
	fi

	while IFS= read -r orphan; do
		[[ -n "$orphan" ]] || continue
		if tmp_target_in_use "$orphan" || target_in_use "$orphan"; then
			echo "skip (cargo/rustc active): $orphan"
			continue
		fi
		mib="$(size_mib "$orphan")"
		rm -rf -- "$orphan"
		echo "swept: $orphan (${mib} MiB)"
		freed=$((freed + mib))
	done < <(stale_tmp_orphans)

	echo "sweep: freed ${freed} MiB"
}

case "${1:-}" in
--check) cmd_check ;;
--sweep) cmd_sweep ;;
*) usage ;;
esac
