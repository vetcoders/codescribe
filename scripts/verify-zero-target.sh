#!/usr/bin/env bash
# Account for isolated Cargo targets without claiming cleanup authority.
# The canonical main-checkout target is the sanctioned shared cache, never
# residue. A temporary filename, size, age or absent cargo PID proves neither
# ownership nor that a source worker/provider has finished.
#
# --check   fail only for an oversized isolated target in the current worktree;
#           report other registered fleet targets and temp names as inventory.
# --dry-run report the same inventory without a size-policy failure or deletion.
# --sweep   refuse: this guard has no authoritative terminal-run/cleanup receipt.
#
# Overrides for accounting: VZT_WORKTREES_ROOT, VZT_TMP_ROOT, VZT_THRESHOLD_MIB.
# No mode deletes, takes leases, kills holders, or modifies cleanup policy.
set -euo pipefail

WORKTREES_ROOT="${VZT_WORKTREES_ROOT:-$HOME/.vibecrafted/worktrees/vetcoders/codescribe}"
TMP_ROOT="${VZT_TMP_ROOT:-/tmp}"
THRESHOLD_MIB="${VZT_THRESHOLD_MIB:-10}"

usage() {
	echo "usage: $0 --check | --dry-run | --sweep" >&2
	exit 2
}

refuse_sweep() {
	cat >&2 <<'REASON'
REFUSED: --sweep has no verified cleanup admission; nothing was removed.
Removal requires a registered canonical worktree, terminal run receipt,
recorded source commit/report, clean index and worktree, no live worker or
provider, no holder of any file/cwd/lease, and admission by the existing cleanup
policy (~/.vc-cleanup.toml) and lease owner, revalidated immediately before
deletion. Unknown or active state refuses. Use the existing guarded cleanup
workflow; this accounting guard cannot infer those facts from names or age.
REASON
	return 2
}

physical_directory() {
	(cd -P "$1" 2>/dev/null && pwd -P)
}

size_mib() {
	local kib
	# Failure stays unknown; an unreadable directory must never become zero bytes.
	kib="$(du -sk "$1" 2>/dev/null | awk 'NR == 1 {print $1}')" || return 1
	[[ "$kib" =~ ^[0-9]+$ ]] || return 1
	echo $(((kib + 1023) / 1024))
}

account_target() {
	local worktree="$1" target resolved mib
	target="$worktree/target"
	if [[ "$target" == "$shared_target" ]]; then
		echo "excluded shared cache: $shared_target"
		return
	fi
	if [[ -L "$target" ]]; then
		# Resolve only for shared-cache identity; a link is never cleanup admission.
		resolved="$(physical_directory "$target")" || resolved=""
		if [[ -n "$resolved" && "$resolved" == "$shared_target" ]]; then
			echo "excluded shared cache: $target -> $shared_target"
		else
			echo "unknown target symlink (untouched): $target"
			[[ "$worktree" != "$source_root" ]] || unknown=1
		fi
		return
	fi
	if [[ ! -e "$target" ]]; then
		[[ "$worktree" != "$source_root" ]] || echo "current isolated target absent: $target"
		return
	fi
	if [[ ! -d "$target" ]]; then
		echo "unknown non-directory target (untouched): $target"
		[[ "$worktree" != "$source_root" ]] || unknown=1
		return
	fi
	resolved="$(physical_directory "$target")" || {
		echo "unknown target location (untouched): $target"
		[[ "$worktree" != "$source_root" ]] || unknown=1
		return
	}
	if [[ "$resolved" == "$shared_target" ]]; then
		echo "excluded shared cache: $shared_target"
		return
	fi
	if ! mib="$(size_mib "$resolved")"; then
		echo "unknown target size (untouched): $resolved"
		[[ "$worktree" != "$source_root" ]] || unknown=1
		return
	fi
	echo "isolated target: $resolved (${mib} MiB; activity/cleanup admission not assessed)"
	if [[ "$worktree" == "$source_root" ]] && ((mib > THRESHOLD_MIB)); then
		echo "source policy drift: current worktree target exceeds ${THRESHOLD_MIB} MiB; use the canonical shared cache" >&2
		oversized=1
	fi
}

mode="${1:-}"
[[ $# == 1 ]] || usage
case "$mode" in
--sweep) refuse_sweep; exit 2 ;;
--check | --dry-run) ;;
*) usage ;;
esac
[[ "$THRESHOLD_MIB" =~ ^[0-9]+$ ]] || usage
THRESHOLD_MIB=$((10#$THRESHOLD_MIB))

source_root="$(git rev-parse --show-toplevel)" || exit 2
source_root="$(physical_directory "$source_root")" || exit 2
common_dir="$(git rev-parse --git-common-dir)" || exit 2
common_dir="$(physical_directory "$common_dir")" || exit 2
main_root="$(physical_directory "$common_dir/..")" || exit 2
shared_target="$main_root/target"
if [[ -e "$shared_target" || -L "$shared_target" ]]; then
	shared_target="$(physical_directory "$shared_target")" || {
		echo "REFUSED: cannot establish canonical shared target" >&2
		exit 2
	}
fi
fleet_root="$(physical_directory "$WORKTREES_ROOT")" || fleet_root=""
oversized=0
unknown=0

echo "accounting only; canonical shared target excluded: $shared_target"
account_target "$source_root"

# Git registration is the inventory source. No recursive scan of arbitrary
# target-shaped directories, and no inference that absent worktrees were merged.
registered="$(git -c core.quotePath=false worktree list --porcelain)" || exit 2
while IFS= read -r line; do
	case "$line" in
	worktree\ *)
		worktree="${line#worktree }"
		worktree="$(physical_directory "$worktree")" || continue
		[[ "$worktree" != "$source_root" ]] || continue
		if [[ -n "$fleet_root" && "$worktree" == "$fleet_root/"* ]]; then
			account_target "$worktree"
		fi
		;;
	esac
done <<< "$registered"

# /tmp is a symlink on macOS. Canonicalize it before a bounded, read-only name
# inventory. These paths have unknown ownership and never fail another cut.
if tmp_root="$(physical_directory "$TMP_ROOT")"; then
	while IFS= read -r -d '' candidate; do
		resolved="$(physical_directory "$candidate")" || resolved=""
		[[ -z "$resolved" || "$resolved" != "$shared_target" ]] || continue
		echo "temp-name candidate (ownership/activity unknown, untouched): $candidate"
	done < <(find "$tmp_root" -mindepth 1 -maxdepth 1 -type d -name 'cs-*-target-*' -print0 2>/dev/null)
fi

((unknown == 0)) || exit 2
if [[ "$mode" == --check ]]; then
	((oversized == 0)) || exit 1
fi
