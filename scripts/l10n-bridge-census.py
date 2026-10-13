#!/usr/bin/env python3
"""l10n-bridge-census.py - no new English prose may cross the Rust -> Swift bridge.

Rust composes some user-visible sentences and hands them across UniFFI as
finished `String`s; Swift can only show such text as it arrives, in English,
in every language (docs/LOCALIZATION_LEDGER.md §4). The String Catalog cannot
see those sentences, so the translator's worksheet never lists them. This gate
keeps that debt from growing silently.

Every `String` field of every `Cs*` record and every `String` payload of a
`Cs*` enum case in the generated bindings (macos/Codescribe/Bridge/
codescribe_ffi.swift) must be classified in scripts/data/l10n-bridge-fields.txt:

  data   identifiers, paths, wire values, model and vendor names, transcript
         and thread content, codes Swift switches on - never shown as a sentence
  prose  a sentence or label a person reads as it arrives - the seams still to
         be moved into Swift (the rule: Rust sends a code and arguments, Swift
         owns the sentence)

A field the bindings carry and the file does not name fails the gate: the cut
that added it classifies it, and a line the bindings no longer carry fails too,
so the file stays an honest burn-down list. The counts are reported.

Classifying a new field `prose` used to be enough to pass. It is not: the list
is a burn-down, so `--base <git-ref>` reads the same file as that ref has it and
refuses a tree whose `prose` lines outnumber the base's. The seams may be
reworded, renamed or retired; they may not multiply. Removing one lowers the bar
for everybody after. Where the ref is out of reach (a shallow clone, a fetch
that never ran) the gate says so on stderr and checks the rest — it never passes
the ratchet silently.

Usage:
  scripts/l10n-bridge-census.py                    # validate (exit 1 on any failure)
  scripts/l10n-bridge-census.py --report           # also list every prose seam
  scripts/l10n-bridge-census.py --base origin/main # also refuse a grown prose list

Contract: docs/LOCALIZATION.md §2 and LOCALIZATION_LEDGER.md §4.

Created by Vetcoders (c)2026
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BINDINGS = REPO / "macos" / "Codescribe" / "Bridge" / "codescribe_ffi.swift"
CLASSIFICATION = REPO / "scripts" / "data" / "l10n-bridge-fields.txt"
KINDS = ("data", "prose")

RECORD = re.compile(r"public (struct|enum) (Cs\w+)[^{]*\{(.*?)\n\}", re.S)
FIELD = re.compile(r"public var (\w+): (?:String\??|\[String\])(?!\w)")
CASE = re.compile(r"case (\w+)\(([^)]*)\)")
PAYLOAD = re.compile(r"(\w+): String\??(?!\w)")


def bridge_fields(source: str) -> list[str]:
    """Every String-carrying field, as `Record.field` or `Enum.case.payload`,
    in source order."""
    fields: list[str] = []
    for kind, name, body in RECORD.findall(source):
        if kind == "struct":
            fields += [f"{name}.{field}" for field in FIELD.findall(body)]
        else:
            for case, payload in CASE.findall(body):
                fields += [f"{name}.{case}.{slot}" for slot in PAYLOAD.findall(payload)]
    return fields


def read_classification(text: str, errors: list[str]) -> dict[str, str]:
    classified: dict[str, str] = {}
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) != 2 or parts[0] not in KINDS:
            errors.append(f"{CLASSIFICATION.name}:{number}: expected `data|prose <Record.field>`, got {raw!r}")
            continue
        kind, field = parts
        if field in classified:
            errors.append(f"{CLASSIFICATION.name}:{number}: {field} is listed twice")
        classified[field] = kind
    return classified


def census(source: str, classification: str) -> tuple[list[str], list[str], dict[str, int]]:
    errors: list[str] = []
    fields = bridge_fields(source)
    classified = read_classification(classification, errors)
    present = set(fields)
    for field in fields:
        if field not in classified:
            errors.append(
                f"{field} crosses the bridge as a String and is not classified in "
                f"{CLASSIFICATION.relative_to(REPO)}: add `data {field}` if it is an identifier, "
                "path, wire value or content, or `prose {field}` if a person reads it as a sentence "
                "- and then prefer sending a code so Swift owns the sentence (LOCALIZATION_LEDGER.md §4)"
            )
    for field in classified:
        if field not in present:
            errors.append(
                f"{field} is classified in {CLASSIFICATION.relative_to(REPO)} but the bindings "
                "no longer carry it: remove the line"
            )
    counts = {kind: sum(1 for f in fields if classified.get(f) == kind) for kind in KINDS}
    prose = [f for f in fields if classified.get(f) == "prose"]
    return errors, prose, counts


def prose_lines(text: str) -> set[str]:
    """The fields one classification file calls `prose`, malformed lines aside."""
    return {field for field, kind in read_classification(text, []).items() if kind == "prose"}


def base_classification(ref: str) -> str | None:
    """The classification file as `ref` has it, or None when the ref is out of reach.

    Out of reach is ordinary, not exceptional: a shallow clone, a fork whose
    upstream was never fetched, a local branch name CI does not have. The caller
    says so out loud and checks what it still can.
    """
    path = CLASSIFICATION.relative_to(REPO).as_posix()
    try:
        done = subprocess.run(
            ["git", "show", f"{ref}:{path}"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    return done.stdout if done.returncode == 0 else None


def ratchet(classification: str, base: str) -> tuple[list[str], list[str], int]:
    """The prose seams this tree adds over the base, those it retires, and the base count.

    Only the arrivals can fail the gate; the departures are the point of the
    list and are reported so a cut gets credit for them.
    """
    before = prose_lines(base)
    now = prose_lines(classification)
    return sorted(now - before), sorted(before - now), len(before)


def parse(argv: list[str]) -> tuple[bool, str | None] | None:
    """`--report` and `--base <ref>` (or `--base=<ref>`), or None on anything else."""
    verbose = False
    base: str | None = None
    rest = iter(argv)
    for arg in rest:
        if arg == "--report":
            verbose = True
        elif arg == "--base":
            base = next(rest, None)
            if base is None:
                return None
        elif arg.startswith("--base="):
            base = arg.split("=", 1)[1]
            if not base:
                return None
        else:
            return None
    return verbose, base


def main(argv: list[str]) -> int:
    parsed = parse(argv[1:])
    if parsed is None:
        print("usage: scripts/l10n-bridge-census.py [--report] [--base <git-ref>]", file=sys.stderr)
        return 2
    verbose, base_ref = parsed
    try:
        source = BINDINGS.read_text(encoding="utf-8")
    except OSError as error:
        print(f"l10n-bridge-census: cannot read the bindings: {error}", file=sys.stderr)
        return 2
    try:
        classification = CLASSIFICATION.read_text(encoding="utf-8")
    except OSError as error:
        print(f"l10n-bridge-census: cannot read the classification: {error}", file=sys.stderr)
        return 2
    errors, prose, counts = census(source, classification)
    print(
        f"l10n-bridge-census: {counts['data'] + counts['prose']} String fields cross the bridge; "
        f"{counts['prose']} are English prose still composed in Rust, {counts['data']} are data"
    )
    if verbose:
        for field in prose:
            print(f"  prose: {field}")
    if base_ref is not None:
        base = base_classification(base_ref)
        if base is None:
            print(
                f"l10n-bridge-census: cannot read {CLASSIFICATION.relative_to(REPO)} at "
                f"{base_ref!r}; the prose ratchet is NOT enforced in this run "
                "(fetch the ref, or pass a --base the clone has)",
                file=sys.stderr,
            )
        else:
            added, retired, before = ratchet(classification, base)
            print(
                f"l10n-bridge-census: {len(prose)} prose seams here, {before} at {base_ref}"
                + (f"; {len(retired)} retired" if retired else "")
            )
            for field in retired:
                print(f"  retired: {field}")
            if len(prose) > before:
                errors.append(
                    f"the prose list grew against {base_ref} ({before} -> {len(prose)}): "
                    + ", ".join(added or ["(no new line, yet more prose than the base)"])
                    + " - the list is a burn-down, not a ledger that fills up. Send a code "
                    "from Rust and let Swift own the sentence (LOCALIZATION_LEDGER.md §4), "
                    "or retire a seam of the same weight in this cut"
                )
    if errors:
        for line in errors:
            print(f"l10n-bridge-census: {line}", file=sys.stderr)
        print(f"l10n-bridge-census: {len(errors)} problem(s)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
