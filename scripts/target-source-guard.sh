#!/bin/bash
# target-source-guard.sh - Rebuild workspace crates another checkout compiled
#
# Usage:
#   scripts/target-source-guard.sh CARGO_TARGET_DIR
#
# Every worktree of this repository builds into the main checkout's target
# directory, and the workspace crates get the same artifact names in each of
# them: Cargo hands rustc paths relative to the workspace root and judges a
# path crate fresh when its source files are older than its last build. After a
# build in another worktree, this checkout can link that worktree's code
# without compiling a line of its own (2026-10-21: a root test run executed a
# worktree's ledger and reported its results as root's).
#
# rustc still records the absolute directory it compiled in near the start of
# each crate's .rmeta. The guard reads it from every workspace crate's metadata
# written since its last look from this checkout. When one was compiled in
# another checkout and is newer than this checkout's crate root, Cargo would
# take it as fresh here, so the guard touches the source root of every
# workspace target in this checkout and Cargo rebuilds the workspace crates
# from these sources; dependencies stay shared. Nothing is deleted.
#
# Builds started at the same moment from two checkouts can still interleave;
# serialize them through the build announcement on channel 0.

set -euo pipefail

TARGET_DIR="${1:?usage: target-source-guard.sh CARGO_TARGET_DIR}"
CHECKOUT="$(git rev-parse --show-toplevel)"

cargo metadata --no-deps --format-version 1 --manifest-path "$CHECKOUT/Cargo.toml" |
    python3 -I -c '
import glob, hashlib, json, mmap, os, re, sys, time

checkout, target_dir = sys.argv[1], sys.argv[2]
meta = json.load(sys.stdin)
targets = [t for p in meta["packages"] for t in p["targets"]]

seen_dir = os.path.join(target_dir, ".target-source-guard")
seen = os.path.join(seen_dir, hashlib.sha1(checkout.encode()).hexdigest()[:16])
last_look = os.stat(seen).st_mtime if os.path.exists(seen) else 0.0
started = time.time()

foreign = set()
for target in targets:
    if not {"lib", "rlib", "cdylib", "staticlib", "proc-macro"} & set(target["kind"]):
        continue
    crate = target["name"].replace("-", "_")
    root = os.path.relpath(target["src_path"], checkout).encode()
    root_mtime = os.stat(target["src_path"]).st_mtime
    built_in = re.compile(rb"(?<![\x21-\x7e])(/[\x21-\x7e]+?)/" + re.escape(root) + rb"(?![\x21-\x7e])")
    pattern = f"lib{crate}-*.rmeta"
    for rmeta in glob.glob(os.path.join(target_dir, "*", "deps", pattern)) + glob.glob(
        os.path.join(target_dir, "*", "*", "deps", pattern)
    ):
        mtime = os.stat(rmeta).st_mtime
        if mtime <= last_look or mtime <= root_mtime:
            continue
        # A build that links keeps the metadata inside the .rlib and may leave
        # the .rmeta empty; an empty .rmeta alone is nothing Cargo can reuse.
        source = rmeta if os.path.getsize(rmeta) else rmeta[: -len(".rmeta")] + ".rlib"
        if not os.path.exists(source) or not os.path.getsize(source):
            continue
        try:
            with open(source, "rb") as handle, mmap.mmap(handle.fileno(), 0, access=mmap.ACCESS_READ) as data:
                match = built_in.search(data)
                compiled_in = match.group(1).decode() if match else None
        except OSError as error:
            print(f"target-source-guard: cannot read {source}: {error}", file=sys.stderr)
            continue
        if compiled_in and compiled_in != checkout:
            foreign.add(compiled_in)

if foreign:
    for target in targets:
        if os.path.exists(target["src_path"]):
            os.utime(target["src_path"])
    print(
        "target-source-guard: workspace crates in the shared target were compiled in "
        + ", ".join(sorted(foreign))
        + f"; they rebuild from {checkout}",
        file=sys.stderr,
    )

os.makedirs(seen_dir, exist_ok=True)
with open(seen, "a"):
    pass
os.utime(seen, (started, started))
' "$CHECKOUT" "$TARGET_DIR"
