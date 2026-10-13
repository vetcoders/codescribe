#!/usr/bin/env python3
"""l10n-bridge-census.py - no new English prose may cross the Rust -> Swift bridge.

Rust composes some user-visible sentences and hands them across UniFFI as
finished `String`s; Swift can only show such text as it arrives, in English,
in every language (docs/LOCALIZATION_LEDGER.md §4). The String Catalog cannot
see those sentences, so the translator's worksheet never lists them. This gate
keeps that debt from growing silently.

Records were not the only way across. The bindings carry Rust's text in four
shapes, and the census reads all four — direction decides which half of a
signature it looks at, because only the half travelling from Rust can arrive in
English:

  record      `public struct Cs*` field, `public enum Cs*` case payload
  callback    a parameter of `public protocol Cs*` (no `Protocol` suffix): Rust
              calls Swift, so the arguments are Rust's words
  object      a `-> String` of `public protocol *Protocol` (the interface behind
              `open class *`): Swift calls Rust, so the return is Rust's words
  free        a `-> String` of a top-level `public func`

The other halves — what Swift passes down and what Swift hands back — are the
app's own strings and are not the gate's business.

Every such `String` in the generated bindings (macos/Codescribe/Bridge/
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
is a burn-down, so `--base <git-ref>` reads that ref's classification and
bindings and refuses a new `prose` line. The seams may be reworded, renamed or
retired; they may not multiply, and removing one lowers the bar for everybody
after. One arrival is forgiven: a seam the base's own bindings already carried
and its classification never named — debt the census has only just learned to
see, so widening the census does not have to break the ratchet to land. A field
the base did not carry is new, and a field the base called `data` is a
reclassification; both are refused, and both answers come from `git show` rather
than from a claim in a commit message. Where the ref is out of reach (a shallow
clone, a fetch that never ran) the gate says so on stderr and checks the rest —
it never passes the ratchet silently.

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
# UniFFI writes an object interface as `protocol XProtocol` beside `open class X`,
# and a callback interface as `protocol X` beside `open class XImpl`. The suffix
# is therefore the direction: `Protocol` means Rust implements it and Swift
# calls in; its absence means Swift implements it and Rust calls out.
PROTOCOL = re.compile(r"public protocol (\w+): AnyObject, Sendable \{(.*?)\n\}", re.S)
DECLARED = re.compile(r"^ {4}func (\w+)\(([^)]*)\)([^\n]*)$", re.M)
FREE = re.compile(r"^public func ([a-z]\w*)\(([^)]*)\)([^{]*)\{", re.M)
RETURNS_STRING = re.compile(r"->\s*(?:String\??|\[String\])\s*$")
ARGUMENT = re.compile(r"(\w+): (?:String\??|\[String\])(?!\w)")


def bridge_fields(source: str) -> list[str]:
    """Every String that can arrive from Rust, by the name the classification
    file knows it under, grouped by shape: records first (`Record.field`,
    `Enum.case.payload`), then callback arguments (`Protocol.func.argument`),
    then the returns Swift reads back (`Object.func()`, `func()`)."""
    fields: list[str] = []
    for kind, name, body in RECORD.findall(source):
        if kind == "struct":
            fields += [f"{name}.{field}" for field in FIELD.findall(body)]
        else:
            for case, payload in CASE.findall(body):
                fields += [f"{name}.{case}.{slot}" for slot in PAYLOAD.findall(payload)]
    callbacks, returns = [], []
    for name, body in PROTOCOL.findall(source):
        for function, arguments, tail in DECLARED.findall(body):
            if name.endswith("Protocol"):
                if RETURNS_STRING.search(tail.strip()):
                    returns.append(f"{name[: -len('Protocol')]}.{function}()")
            else:
                callbacks += [f"{name}.{function}.{slot}" for slot in ARGUMENT.findall(arguments)]
    returns += [
        f"{function}()" for function, _, tail in FREE.findall(source)
        if RETURNS_STRING.search(tail.strip())
    ]
    return fields + callbacks + returns


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
                f"{field} carries a String from Rust and is not classified in "
                f"{CLASSIFICATION.relative_to(REPO)}: add `data {field}` if it is an identifier, "
                f"path, wire value or content, or `prose {field}` if a person reads it as a sentence "
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


def at_ref(ref: str, path: Path) -> str | None:
    """One repository file as `ref` has it, or None when the ref is out of reach.

    Out of reach is ordinary, not exceptional: a shallow clone, a fork whose
    upstream was never fetched, a local branch name CI does not have. The caller
    says so out loud and checks what it still can.
    """
    try:
        done = subprocess.run(
            ["git", "show", f"{ref}:{path.relative_to(REPO).as_posix()}"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    return done.stdout if done.returncode == 0 else None


def ratchet(
    classification: str, base: str, base_bindings: str | None
) -> tuple[list[str], list[str], list[str], int]:
    """What this tree does to the prose list: what it adds, what it may add, what
    it retires, and what the base carried.

    Two kinds of line arrive, and only one of them is debt. A seam the base's own
    bindings already carried, and its classification never named, is debt the
    census has only just learned to see — writing it down is the opposite of
    adding it, so widening the census does not have to break the ratchet to land.
    Everything else is refused: a field the base did not carry is new, and a
    field the base classified `data` and this tree calls `prose` is a
    reclassification, which is how the debt would be laundered. Both answers come
    out of `git show`, so neither is a claim anybody can make in a commit message.

    Without the base's bindings nothing is forgiven, which is the safe way to be
    uncertain.
    """
    before = prose_lines(base)
    now = prose_lines(classification)
    classified = set(read_classification(base, []))
    carried = set(bridge_fields(base_bindings)) if base_bindings is not None else set()
    arrived = sorted(now - before)
    newly_visible = [f for f in arrived if f not in classified and f in carried]
    return (
        [f for f in arrived if f not in newly_visible],
        newly_visible,
        sorted(before - now),
        len(before),
    )


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
        base = at_ref(base_ref, CLASSIFICATION)
        if base is None:
            print(
                f"l10n-bridge-census: cannot read {CLASSIFICATION.relative_to(REPO)} at "
                f"{base_ref!r}; the prose ratchet is NOT enforced in this run "
                "(fetch the ref, or pass a --base the clone has)",
                file=sys.stderr,
            )
        else:
            added, visible, retired, before = ratchet(
                classification, base, at_ref(base_ref, BINDINGS)
            )
            print(
                f"l10n-bridge-census: {len(prose)} prose seams here, {before} at {base_ref}"
                + (f"; {len(retired)} retired" if retired else "")
                + (f"; {len(visible)} newly visible" if visible else "")
            )
            for field in retired:
                print(f"  retired: {field}")
            for field in visible:
                print(f"  newly visible: {field}")
            if added:
                errors.append(
                    f"the prose list grew against {base_ref} ({before} -> {len(prose)}) by a "
                    "seam that base already classified or never carried: "
                    + ", ".join(added)
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
