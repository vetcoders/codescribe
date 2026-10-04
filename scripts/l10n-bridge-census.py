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
that added it classifies it, and a new `prose` line is a conscious decision to
add debt (the ledger says why). A line the bindings no longer carry fails too,
so the file stays an honest burn-down list. The counts are reported.

Usage:
  scripts/l10n-bridge-census.py            # validate (exit 1 on any failure)
  scripts/l10n-bridge-census.py --report   # also list every prose seam

Contract: docs/LOCALIZATION.md §2 and LOCALIZATION_LEDGER.md §4.

Created by Vetcoders (c)2026
"""

from __future__ import annotations

import re
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


def main(argv: list[str]) -> int:
    verbose = "--report" in argv[1:]
    unknown = [arg for arg in argv[1:] if arg != "--report"]
    if unknown:
        print("usage: scripts/l10n-bridge-census.py [--report]", file=sys.stderr)
        return 2
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
    if errors:
        for line in errors:
            print(f"l10n-bridge-census: {line}", file=sys.stderr)
        print(f"l10n-bridge-census: {len(errors)} problem(s)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
