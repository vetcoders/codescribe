#!/usr/bin/env python3
"""l10n-lint.py - static checks on the String Catalogs (no build, no Xcode).

Reads the catalogs as JSON and fails on structural damage that would ship as a
wrong or missing string:

  1. the source language is English and every catalog parses;
  2. no key is `stale` — a translation whose key left the code is orphaned and
     must be removed or re-keyed on purpose, never carried silently;
  3. every string reads the arguments its source provides, by number and by
     type — a translation against English, and English against the key the
     code passes arguments for. `%2$@ … %1$lld` against a source of
     `%@ … %lld` swaps an object and an integer and is refused; `%2$lld … %1$@`
     reorders them and passes. A `%#@name@` substitution counts as the argument
     it formats. Nothing may be added. Only a plural form may leave an argument
     out, and only the count that selects the form ("One recording"); a
     substitution without plural forms is refused, as the compiler refuses it;
  4. plural variations carry every form the language has, at every depth of a
     string and for each of its substitutions. The forms are the cardinal
     categories Unicode CLDR lists for the language (scripts/data/cldr), so
     this file names no language but the source; one CLDR has no rules for is
     refused, not held to less. A string varied as a whole has exactly one
     integer argument, since nothing else can name the count;
  5. no language is declared without translations — a bundle that claims a
     language it does not carry gives a mixed interface;
  6. the permission prompts in InfoPlist.xcstrings match macos/project.yml word
     for word, so the catalog cannot drift from the plist it overrides.

It reports, without failing, translation coverage per language and count-bearing
keys that have no plural variations yet.

This is authoritative for the catalog files only. Whether the catalog matches
the Swift sources is a different question, answered by scripts/l10n-sync.sh
against a real build.

Usage:
  scripts/l10n-lint.py            # validate (exit 1 on any failure)
  scripts/l10n-lint.py --report   # also list untranslated keys and plural candidates

Contract: docs/LOCALIZATION.md.

Created by Vetcoders (c)2026
"""

from __future__ import annotations

import functools
import json
import re
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CATALOG_DIR = REPO / "macos" / "Codescribe" / "Resources" / "Localization"
PROJECT_SPEC = REPO / "macos" / "project.yml"
SOURCE_LANGUAGE = "en"

# Unicode CLDR cardinal plural rules, byte for byte as published in cldr-json
# (cldr-core/supplemental/plurals.json). Provenance and licence: NOTICE.
PLURAL_RULES = REPO / "scripts" / "data" / "cldr" / "plurals.json"

SPECIFIER = re.compile(
    r"%(?:(?P<number>\d+)\$)?"
    r"(?:#@(?P<rule>\w+)@"
    r"|(?P<own>arg)"
    r"|[-+ 0#]*\d*(?:\.\d+)?(?P<type>lld|llu|ld|lu|lf|d|u|f|@|s|a|c|x))"
)
INTEGER_TYPES = frozenset({"lld", "llu", "ld", "lu", "d", "u"})
COUNT_THEN_WORD = re.compile(r"%(?:\d+\$)?ll[du] [A-Za-z]")
USAGE_DESCRIPTION = re.compile(r'^\s+(NS\w+UsageDescription):\s*"(.*)"\s*$')
SUBSTITUTION = re.compile(r"%(?:\d+\$)?#@(\w+)@")


@functools.lru_cache(maxsize=None)
def plural_rules() -> dict[str, tuple[str, ...]]:
    """The plural categories of every locale CLDR has rules for, in CLDR order."""
    data = json.loads(PLURAL_RULES.read_text(encoding="utf-8"))
    return {
        locale.lower(): tuple(rule.removeprefix("pluralRule-count-") for rule in rules)
        for locale, rules in data["supplemental"]["plurals-type-cardinal"].items()
    }


def plural_categories(language: str) -> tuple[str, ...] | None:
    """The plural forms a language must supply, or None when CLDR has no rules.

    A catalog language may carry a script or a region (`zh-Hans`, `pt-BR`). The
    longest prefix CLDR knows decides: `pt-PT` keeps its own rules and `pt-BR`
    takes those of `pt`.
    """
    subtags = language.replace("_", "-").lower().split("-")
    while subtags:
        categories = plural_rules().get("-".join(subtags))
        if categories:
            return categories
        subtags.pop()
    return None


def arguments(
    text: str, substitutions: dict, own: tuple[int, str] | None = None
) -> tuple[dict[int, str], list[str]]:
    """The arguments a format string reads, as {argument number: conversion type}.

    Unnumbered specifiers take positions 1, 2, … in reading order, which is how
    the key the Swift compiler extracts numbers them. `%#@name@` reads the
    argument its substitution formats. `own` is the argument of the enclosing
    substitution: there `%arg` is that argument, and every other specifier must
    carry its number, because an unnumbered one has no position of its own.

    Returns the arguments and the reasons the string cannot be read reliably;
    with any reason present the arguments are not worth comparing.
    """
    found: dict[int, str] = {}
    problems: list[str] = []
    implicit = 0
    numbered = unnumbered = False
    for match in SPECIFIER.finditer(text.replace("%%", "")):
        number = int(match["number"]) if match["number"] else None
        if match["rule"]:
            rule = substitutions.get(match["rule"])
            if rule is None:
                continue  # reported as "used but not defined"
            kind = rule.get("formatSpecifier", "")
            declared = rule.get("argNum")
            if number is None:
                number = declared
            elif declared is not None and declared != number:
                problems.append(
                    f"`{match[0]}` names argument {number}, its substitution formats argument {declared}"
                )
        elif match["own"]:
            if own is None:
                problems.append("`%arg` is only meaningful inside a substitution")
                continue
            number, kind = own
        else:
            kind = match["type"]
        if number is None:
            if own is not None:
                problems.append(
                    f"`{match[0]}` inside a substitution has no argument number "
                    "(write the count as `%arg` and any other argument as `%N$…`)"
                )
                continue
            unnumbered = True
            implicit += 1
            number = implicit
        else:
            numbered = True
        if found.setdefault(number, kind) != kind:
            problems.append(f"argument {number} is read as both %{found[number]} and %{kind}")
    if numbered and unnumbered:
        # The positions guessed above are meaningless then; say only this.
        return found, ["numbered and unnumbered arguments are mixed (number every one)"]
    return found, problems


def differences(
    found: dict[int, str], expected: dict[int, str], optional: frozenset[int] = frozenset()
) -> list[str]:
    """How `found` departs from the source arguments; `optional` may be left out."""
    out = []
    for number in sorted(found.keys() | expected.keys()):
        have, want = found.get(number), expected.get(number)
        if have == want:
            continue
        if want is None:
            out.append(f"argument {number} (%{have}) does not exist in the source")
        elif have is None:
            if number not in optional:
                out.append(f"argument {number} (%{want}) is missing")
        else:
            out.append(f"argument {number} is %{have}, the source has %{want}")
    return out


def leaf_units(localization: dict) -> list[tuple[str, dict]]:
    """Every string unit of one localization, labelled by its variation path."""
    units = []
    if "stringUnit" in localization:
        units.append(("", localization["stringUnit"]))
    for kind, cases in localization.get("variations", {}).items():
        for case, nested in cases.items():
            for label, unit in leaf_units(nested):
                units.append((f"{kind}.{case}{'.' + label if label else ''}", unit))
    return units


def plural_sets(node: dict, path: str = "") -> list[tuple[str, dict]]:
    """Every set of plural cases under one localization or substitution."""
    sets = []
    for kind, cases in node.get("variations", {}).items():
        if kind == "plural":
            sets.append((path, cases))
        for case, nested in cases.items():
            sets.extend(plural_sets(nested, f"{path}{'.' if path else ''}{kind}.{case}"))
    return sets


def in_plural(label: str) -> bool:
    return label.startswith("plural.") or ".plural." in label


def source_format(key: str, entry: dict) -> tuple[str, dict]:
    """The format string the code passes its arguments to, and its substitutions.

    An extracted key doubles as that string. A key written with `defaultValue:`
    is an identifier, so the English text stands in: the whole string, or its
    `other` form when English itself varies.
    """
    if entry.get("extractionState") != "extracted_with_value":
        return key, {}
    source = entry.get("localizations", {}).get(SOURCE_LANGUAGE, {})
    units = leaf_units(source)
    for label, unit in units:
        if label == "" or label.split(".")[-1] == "other":
            return unit.get("value", ""), source.get("substitutions", {})
    return key, {}


def lint_arguments(key: str, entry: dict, complain) -> None:
    """Check every string of every language against the arguments of the source."""
    source_text, source_rules = source_format(key, entry)
    expected, problems = arguments(source_text, source_rules)
    for problem in problems:
        complain(SOURCE_LANGUAGE, "", f"source: {problem}")
    if problems:
        return
    everything = frozenset(expected)
    integers = [number for number, kind in expected.items() if kind in INTEGER_TYPES]

    def check(language, label, text, rules, own=None, optional=frozenset()):
        found, unreadable = arguments(text, rules, own)
        for problem in unreadable or differences(found, expected, optional):
            complain(language, label, problem)

    for language, localization in entry.get("localizations", {}).items():
        rules = localization.get("substitutions", {})
        units = leaf_units(localization)
        count = frozenset()
        if any(in_plural(label) for label, _ in units):
            if len(integers) == 1:
                count = frozenset(integers)
            else:
                complain(
                    language,
                    "",
                    f"plural forms on a string with {len(integers)} integer arguments "
                    "(use one substitution per count)",
                )
        for label, unit in units:
            optional = count if in_plural(label) else frozenset()
            check(language, label, unit.get("value", ""), rules, optional=optional)

        used = set(SUBSTITUTION.findall("\n".join(unit.get("value", "") for _, unit in units)))
        for name in sorted(used - rules.keys()):
            complain(language, "", f"substitution '{name}' is used but not defined")
        for name, rule in rules.items():
            number, kind = rule.get("argNum"), rule.get("formatSpecifier")
            if not isinstance(number, int) or not kind:
                complain(language, name, "substitution needs `argNum` and `formatSpecifier`")
                continue
            if name not in used:
                complain(language, name, "substitution is defined but not used")
            if not plural_sets(rule):
                # xcstringstool refuses this shape: "Cannot reference 'name'
                # from here because it is not a plural variation".
                complain(language, name, "substitution has no plural forms")
            # The rest of the sentence carries the other arguments, so a form
            # owes only its own count — and only a plural form may spell it out.
            others = everything - {number}
            for label, unit in leaf_units(rule):
                check(
                    language,
                    f"{name}.{label}" if label else name,
                    unit.get("value", ""),
                    rules,
                    own=(number, kind),
                    optional=everything if in_plural(label) else others,
                )


def lint_catalog(path: Path, errors: list[str], report: list[str]) -> None:
    name = path.name
    try:
        catalog = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        errors.append(f"{name}: unreadable catalog: {error}")
        return
    if catalog.get("sourceLanguage") != SOURCE_LANGUAGE:
        errors.append(f"{name}: sourceLanguage must be '{SOURCE_LANGUAGE}'")
    strings = catalog.get("strings", {})

    translatable = 0
    translated = Counter()
    untranslated: dict[str, list[str]] = {}
    languages = {
        language for entry in strings.values() for language in entry.get("localizations", {})
    }
    plural_candidates = []
    unruled = set()

    for key, entry in strings.items():
        if entry.get("extractionState") == "stale":
            errors.append(f"{name}: stale key (no longer in code): {key!r}")
        localizations = entry.get("localizations", {})
        if entry.get("shouldTranslate") is False:
            continue
        translatable += 1

        source = localizations.get(SOURCE_LANGUAGE, {})
        source_text, _ = source_format(key, entry)
        varies = "plural" in source.get("variations", {}) or source.get("substitutions")
        if not varies and COUNT_THEN_WORD.search(source_text):
            plural_candidates.append(key)

        def complain(language: str, label: str, problem: str, key: str = key) -> None:
            where = f" [{label}]" if label else ""
            errors.append(f"{name}: {language}{where}: {problem} for {key!r}")

        lint_arguments(key, entry, complain)

        for language, localization in localizations.items():
            units = leaf_units(localization)
            required = plural_categories(language)
            sets = plural_sets(localization)
            for label, rule in localization.get("substitutions", {}).items():
                units.extend((f"{label}.{path}", unit) for path, unit in leaf_units(rule))
                sets.extend(
                    (f"{label}{'.' + path if path else ''}", cases)
                    for path, cases in plural_sets(rule)
                )
            if sets and required is None:
                unruled.add(language)
            for where, plural in sets:
                absent = [case for case in required or () if case not in plural]
                if absent:
                    complain(language, where, f"plural forms missing ({', '.join(absent)})")
            if language != SOURCE_LANGUAGE:
                if units and all(unit.get("state") == "translated" for _, unit in units):
                    translated[language] += 1

        for language in languages - {SOURCE_LANGUAGE}:
            localization = localizations.get(language)
            units = leaf_units(localization) if localization else []
            if not units or any(unit.get("state") != "translated" for _, unit in units):
                untranslated.setdefault(language, []).append(key)

    for language in sorted(unruled):
        errors.append(
            f"{name}: language '{language}' has plural forms, and "
            f"{PLURAL_RULES.relative_to(REPO)} has no plural rules for it "
            "(check the language code, or refresh the CLDR data)"
        )
    for language in sorted(languages - {SOURCE_LANGUAGE}):
        done = translated[language]
        if done == 0:
            errors.append(
                f"{name}: language '{language}' is declared but carries no translations"
            )
        report.append(f"{name}: {language}: {done}/{translatable} translated")
        for key in sorted(untranslated.get(language, [])):
            report.append(f"  untranslated ({language}): {key!r}")
    report.append(f"{name}: {len(strings)} keys, {translatable} translatable")
    for key in sorted(plural_candidates):
        report.append(f"  count without plural variations: {key!r}")


def lint_usage_descriptions(errors: list[str]) -> None:
    """InfoPlist.xcstrings overrides the plist at runtime; keep the two identical."""
    try:
        catalog = json.loads((CATALOG_DIR / "InfoPlist.xcstrings").read_text(encoding="utf-8"))
        spec = PROJECT_SPEC.read_text(encoding="utf-8")
    except (OSError, ValueError) as error:
        errors.append(f"InfoPlist.xcstrings: cannot compare with project.yml: {error}")
        return
    declared = {}
    for line in spec.splitlines():
        match = USAGE_DESCRIPTION.match(line)
        if match:
            declared[match.group(1)] = match.group(2)
    strings = catalog.get("strings", {})
    for key, text in declared.items():
        unit = strings.get(key, {}).get("localizations", {}).get(SOURCE_LANGUAGE, {})
        value = unit.get("stringUnit", {}).get("value")
        if value is None:
            errors.append(f"InfoPlist.xcstrings: missing {key} (declared in project.yml)")
        elif value != text:
            errors.append(f"InfoPlist.xcstrings: {key} differs from project.yml")
    for key in strings:
        if key.endswith("UsageDescription") and key not in declared:
            errors.append(f"InfoPlist.xcstrings: {key} is not declared in project.yml")


def main(argv: list[str]) -> int:
    verbose = "--report" in argv[1:]
    unknown = [arg for arg in argv[1:] if arg != "--report"]
    if unknown:
        print("usage: scripts/l10n-lint.py [--report]", file=sys.stderr)
        return 2

    try:
        plural_rules()
    except (OSError, ValueError, KeyError, AttributeError) as error:
        rules = PLURAL_RULES.relative_to(REPO)
        print(f"l10n-lint: cannot read the plural rules in {rules}: {error}", file=sys.stderr)
        return 1

    errors: list[str] = []
    report: list[str] = []
    catalogs = sorted(CATALOG_DIR.glob("*.xcstrings"))
    if not catalogs:
        errors.append(f"no catalogs under {CATALOG_DIR.relative_to(REPO)}")
    for path in catalogs:
        lint_catalog(path, errors, report)
    lint_usage_descriptions(errors)

    for line in report:
        if verbose or not line.startswith("  "):
            print(line)
    if errors:
        for line in errors:
            print(f"l10n-lint: {line}", file=sys.stderr)
        print(f"l10n-lint: {len(errors)} problem(s)", file=sys.stderr)
        return 1
    print("l10n-lint: catalogs are structurally sound.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
