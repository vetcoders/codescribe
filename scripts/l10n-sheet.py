#!/usr/bin/env python3
"""l10n-sheet.py - the translator worksheet: catalog -> CSV -> catalog.

A String Catalog is JSON a translator cannot work in. This tool turns the two
catalogs into one flat worksheet per language (one row per string a person has
to write) and folds the filled worksheet back into the catalogs.

  scripts/l10n-sheet.py export <lang> <out-dir> [--pending]
      Writes codescribe-<lang>-Localizable.csv and codescribe-<lang>-InfoPlist.csv.
      A plural key becomes one row per plural form the language owes (CLDR);
      a sentence with several counts becomes its main row plus one row per form
      of every substitution. Keys marked `shouldTranslate: false` are left out.
      When a Debug build is present under L10N_DERIVED (default macos/build),
      each row names the screens that use the key. Translations the catalog
      already holds are written into the translation column, so the worksheet
      of a translated language is a revision sheet and new copy shows up as
      the empty rows. `--pending` writes only the keys that still need the
      translator: untranslated ones and drafts (state `needs_review`, see
      import --draft); the context column of a draft row starts with
      "Draft —", so a reviewer never has to read the whole catalog again.

  scripts/l10n-sheet.py import <lang> <csv>... [--check] [--draft]
      Reads the rows back (each row says which catalog it belongs to), builds
      the language's localization for every key whose rows are all filled, and
      writes both catalogs in the byte layout `xcstringstool` uses, so a sync
      after the import shows no spurious diff. A key is imported whole or not
      at all: a plural with one empty form stays untranslated. A row is refused
      - and its key left untouched - when its text does not read the arguments
      the English reads (the same rule l10n-lint.py applies to the catalog), or
      when it carries a spelling the product does not use. `--check` reports
      and writes nothing. `--draft` stores the rows as `needs_review`: a first
      translation written by an agent or a developer with the cut that added
      the English, good enough to ship and to pass the coverage gate, still
      owed a reviewer. A plain import of the same keys later marks them
      `translated` — that is how a review round closes.

      Exit status is 1 whenever anything was refused or left untranslated, so
      a partial import is never mistaken for a complete one. The completeness
      gate itself is `make verify-l10n-catalog`.

Columns (fixed order; the header is written in the translator's language where
the tool knows it, English otherwise, and the importer accepts either):

  catalog, key, member, form, example count, where, context, English, <lang>, notes

  member  name of the `%#@member@` substitution this row is a plural form of
          (empty for the main row and for a string varied as a whole)
  form    plural category this row translates (empty for a plain string)

Only the translation column is written by the translator; `notes` is theirs.

Contract: docs/LOCALIZATION.md §6.

Created by Vetcoders (c)2026
"""

from __future__ import annotations

import csv
import importlib.util
import json
import os
import re
import sys
from collections import OrderedDict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CATALOG_DIR = REPO / "macos" / "Codescribe" / "Resources" / "Localization"
CATALOGS = ("Localizable", "InfoPlist")

_SPEC = importlib.util.spec_from_file_location("l10n_lint", REPO / "scripts" / "l10n-lint.py")
lint = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(lint)

SOURCE = lint.SOURCE_LANGUAGE

COLUMNS = (
    "catalog",
    "key",
    "member",
    "form",
    "example count",
    "where",
    "context",
    "English",
    "<translation>",
    "notes",
)
# The translator reads the header in their own language; the Polish sheet
# was born with this one, so it is the format the importer recognises.
HEADERS = {
    "pl": (
        "katalog", "klucz", "człon", "forma", "przykład liczby",
        "gdzie", "kontekst", "angielski", "polski", "uwagi",
    ),
}
EXAMPLES = {
    "zero": "0",
    "one": "1",
    "two": "2",
    "few": "2, 3, 4, 22",
    "many": "0, 5, 12, 100",
    "other": "1.5 (fractions)",
}
SUBSTITUTION_HINT = "The %#@…@ markers stay as they are; their forms are the rows below."
DRAFT_HINT = "Draft — written with the code, not reviewed yet"
MEMBER_HINT = "Form of %#@{name}@ in the sentence above; %arg is the count."


# ── catalog shape ───────────────────────────────────────────────────────────


def unit_value(node: dict) -> str:
    return node["stringUnit"]["value"]


def plural_values(node: dict) -> dict[str, str]:
    return {form: unit_value(body) for form, body in node["variations"]["plural"].items()}


def source_slots(key: str, entry: dict, forms: tuple[str, ...]) -> list[tuple[str, str, str]]:
    """The rows a translator owes for one key: (member, form, English text).

    The order is the order `export` writes and `import` expects.
    """
    english = entry.get("localizations", {}).get(SOURCE)
    if english is None:
        return [("", "", key)]
    if "variations" in english:
        if "plural" not in english["variations"]:
            raise SystemExit(f"l10n-sheet: unsupported variation on {key!r}")
        source = plural_values(english)
        return [("", form, source.get(form, source["other"])) for form in forms]
    slots = [("", "", unit_value(english))]
    for name, body in english.get("substitutions", {}).items():
        source = plural_values(body)
        slots += [(name, form, source.get(form, source["other"])) for form in forms]
    return slots


def translated_slots(entry: dict, lang: str) -> dict[tuple[str, str], str]:
    """{(member, form): text} the catalog already holds for `lang`, in the
    shape `source_slots` names rows; empty when the key is untranslated."""
    current = entry.get("localizations", {}).get(lang)
    if current is None:
        return {}
    if "variations" in current:
        return {("", form): text for form, text in plural_values(current).items()}
    values = {("", ""): unit_value(current)}
    for name, body in current.get("substitutions", {}).items():
        values.update({(name, form): text for form, text in plural_values(body).items()})
    return values


def build_localization(
    entry: dict, values: dict[tuple[str, str], str], state: str = "translated"
) -> dict:
    """The language's localization for one key, from {(member, form): text}."""
    english = entry.get("localizations", {}).get(SOURCE)

    def unit(text: str) -> dict:
        return {"stringUnit": {"state": state, "value": text}}

    if english is None or "stringUnit" in english and not english.get("substitutions"):
        return unit(values[("", "")])
    if "variations" in english:
        forms = sorted({form for member, form in values if member == ""})
        return {"variations": {"plural": {form: unit(values[("", form)]) for form in forms}}}
    localization = unit(values[("", "")])
    localization["substitutions"] = {}
    for name, body in english["substitutions"].items():
        forms = sorted({form for member, form in values if member == name})
        localization["substitutions"][name] = {
            "argNum": body["argNum"],
            "formatSpecifier": body["formatSpecifier"],
            "variations": {"plural": {form: unit(values[(name, form)]) for form in forms}},
        }
    return localization


def plural_forms(lang: str) -> tuple[str, ...]:
    forms = lint.plural_categories(lang)
    if forms is None:
        raise SystemExit(
            f"l10n-sheet: no CLDR plural rules for {lang!r} in "
            f"{lint.PLURAL_RULES.relative_to(REPO)} (check the language code)"
        )
    return forms


# ── catalog files ───────────────────────────────────────────────────────────


def catalog_path(name: str) -> Path:
    return CATALOG_DIR / f"{name}.xcstrings"


def load_catalog(name: str) -> dict:
    return json.loads(catalog_path(name).read_text(encoding="utf-8"))


_EMPTY_OBJECT = re.compile(r"^(?P<indent>\s*)(?P<head>.*)\{\}(?P<tail>,?)$")


def dump_catalog(catalog: dict) -> str:
    """The JSON layout xcstringstool writes: two-space indent, ` : ` after
    keys, keys sorted, non-ASCII kept, and an empty object spread over three
    lines. Byte-identical to the shipped catalogs, so an import leaves a clean
    diff and a later sync does not reformat the file."""
    text = json.dumps(catalog, ensure_ascii=False, indent=2, separators=(",", " : "), sort_keys=True)
    lines = []
    for line in text.split("\n"):
        match = _EMPTY_OBJECT.match(line)
        if match:
            indent = match["indent"]
            lines.append(f"{indent}{match['head']}{{\n\n{indent}}}{match['tail']}")
        else:
            lines.append(line)
    return "\n".join(lines) + "\n"


# ── export ──────────────────────────────────────────────────────────────────


def build_locations(derived: Path) -> dict[str, list[tuple[str, int, str]]]:
    """key -> [(screen, line, comment)] from the .stringsdata of the last Debug build."""
    where: dict[str, list[tuple[str, int, str]]] = {}
    objects = derived / "Build/Intermediates.noindex/Codescribe.build/Debug/Codescribe.build/Objects-normal"
    for path in sorted(objects.rglob("*.stringsdata")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        source = data.get("source", path.name)
        marker = "macos/Codescribe/"
        screen = source.split(marker, 1)[1] if marker in source else os.path.basename(source)
        screen = screen.removesuffix(".swift")
        for row in data.get("tables", {}).get("Localizable", []):
            line = row.get("location", {}).get("startingLine", 0)
            where.setdefault(row["key"], []).append((screen, line, row.get("comment", "")))
    return where


def review_state(entry: dict, lang: str, forms: tuple[str, ...]) -> str:
    """'missing' (no complete translation), 'draft' (some unit is not yet
    `translated`, i.e. `needs_review`) or 'done' for one key in `lang`."""
    localization = entry.get("localizations", {}).get(lang)
    if not localization:
        return "missing"
    held = translated_slots(entry, lang)
    if any((m, f) not in held for m, f, _ in source_slots("", entry, forms)):
        return "missing"
    states = [unit.get("state") for _, unit in lint.leaf_units(localization)]
    for rule in localization.get("substitutions", {}).values():
        states += [unit.get("state") for _, unit in lint.leaf_units(rule)]
    return "done" if all(state == "translated" for state in states) else "draft"


def export_rows(
    catalog: str, key: str, entry: dict, forms, where, lang: str, draft: bool = False
) -> list[list[str]]:
    spots = where.get(key, [])
    screens = list(dict.fromkeys(screen for screen, _, _ in spots))
    place = ", ".join(screens[:3]) + (" …" if len(screens) > 3 else "")
    comments = [entry.get("comment", "")] + [comment for _, _, comment in spots]
    context = " | ".join(
        dict.fromkeys(c.strip().replace("\n", " / ") for c in comments if c and c.strip())
    )
    slots = source_slots(key, entry, forms)
    current = translated_slots(entry, lang)
    rows = []
    for member, form, text in slots:
        note = context
        if draft:
            note = " — ".join(filter(None, [DRAFT_HINT, note]))
        if member:
            note = " — ".join(filter(None, [note, MEMBER_HINT.format(name=member)]))
        elif form == "" and len(slots) > 1:
            note = " — ".join(filter(None, [note, SUBSTITUTION_HINT]))
        translated = current.get((member, form), "")
        rows.append([catalog, key, member, form, EXAMPLES.get(form, ""), place, note, text, translated, ""])
    return rows


def export(lang: str, out_dir: Path, pending: bool = False) -> int:
    forms = plural_forms(lang)
    derived = Path(os.environ.get("L10N_DERIVED", REPO / "macos" / "build"))
    where = build_locations(derived)
    if not where:
        print(
            f"l10n-sheet: no Debug build under {derived} — the 'where' column stays empty",
            file=sys.stderr,
        )
    header = list(HEADERS.get(lang, COLUMNS))
    if lang not in HEADERS:
        header[8] = lang
    out_dir.mkdir(parents=True, exist_ok=True)
    for catalog in CATALOGS:
        strings = load_catalog(catalog)["strings"]
        groups = []
        for key, entry in strings.items():
            if not entry.get("shouldTranslate", True):
                continue
            state = review_state(entry, lang, forms)
            if pending and state == "done":
                continue
            spots = where.get(key, []) if catalog == "Localizable" else []
            order = (spots[0][0], spots[0][1]) if spots else ("~", 0)
            rows = export_rows(catalog, key, entry, forms, where, lang, draft=state == "draft")
            groups.append((order, key, rows))
        groups.sort(key=lambda g: (g[0], g[1]))
        path = out_dir / f"codescribe-{lang}-{catalog}.csv"
        with path.open("w", newline="", encoding="utf-8") as handle:
            writer = csv.writer(handle)
            writer.writerow(header)
            for _, _, rows in groups:
                writer.writerows(rows)
        total = sum(len(rows) for _, _, rows in groups)
        unplaced = sum(1 for order, _, _ in groups if order[0] == "~")
        print(f"l10n-sheet: {path.relative_to(Path.cwd()) if path.is_relative_to(Path.cwd()) else path}: "
              f"{len(groups)} {'pending ' if pending else ''}keys -> {total} rows"
              + (f"; {unplaced} without a source location" if where else ""))
    return 0


# ── import ──────────────────────────────────────────────────────────────────


class Row:
    __slots__ = ("file", "number", "catalog", "key", "member", "form", "english", "text")

    def __init__(self, file: str, number: int, cells: list[str]):
        cells = list(cells) + [""] * (len(COLUMNS) - len(cells))
        self.file = file
        self.number = number  # 1-based data row; row 1 of the sheet is the header
        self.catalog, self.key, self.member, self.form = cells[:4]
        self.english = cells[7]
        self.text = cells[8].strip()

    def where(self) -> str:
        return f"{self.file} row {self.number + 1}"


def read_rows(paths: list[Path]) -> list[Row]:
    rows = []
    for path in paths:
        with path.open(newline="", encoding="utf-8") as handle:
            reader = csv.reader(handle)
            header = next(reader, None)
            known = [tuple(h[:4]) for h in (COLUMNS, *HEADERS.values())]
            if header is None or tuple(h.strip().lower() for h in header[:4]) not in known:
                raise SystemExit(
                    f"l10n-sheet: {path}: not a worksheet this tool exported "
                    f"(expected columns {', '.join(COLUMNS[:4])}, …)"
                )
            for number, cells in enumerate(reader, 1):
                if not any(cell.strip() for cell in cells):
                    continue
                rows.append(Row(path.name, number, cells))
    return rows


def forbidden_spelling(text: str) -> str | None:
    for spelling, reason in lint.FORBIDDEN_SPELLINGS.items():
        if spelling in text:
            return f"spells `{spelling}` ({reason})"
    return None


def import_rows(lang: str, paths: list[Path], write: bool, draft: bool = False) -> int:
    forms = plural_forms(lang)
    rows = read_rows(paths)
    by_key: dict[tuple[str, str], list[Row]] = OrderedDict()
    for row in rows:
        by_key.setdefault((row.catalog, row.key), []).append(row)

    refused: list[str] = []
    untranslated: list[str] = []
    notices: list[str] = []
    imported = {name: 0 for name in CATALOGS}
    seen: set[tuple[str, str]] = set()

    for catalog in CATALOGS:
        document = load_catalog(catalog)
        strings = document["strings"]
        for key, entry in strings.items():
            if not entry.get("shouldTranslate", True):
                if (catalog, key) in by_key:
                    notices.append(f"{catalog}: {key!r} is not for translation; its rows were ignored")
                    seen.add((catalog, key))
                continue
            slots = source_slots(key, entry, forms)
            group = by_key.get((catalog, key))
            if group is None:
                untranslated.append(f"{catalog}: {key!r} has no rows in the worksheet")
                continue
            seen.add((catalog, key))
            given = {(row.member, row.form): row for row in group}
            missing = [(m, f) for m, f, _ in slots if (m, f) not in given]
            extra = [slot for slot in given if slot not in {(m, f) for m, f, _ in slots}]
            if extra:
                refused.append(
                    f"{catalog}: {key!r} has rows the English does not call for: "
                    + ", ".join(f"({m or '-'}, {f or '-'})" for m, f in extra)
                )
                continue
            if missing:
                untranslated.append(
                    f"{catalog}: {key!r} lacks rows for "
                    + ", ".join(f"({m or '-'}, {f or '-'})" for m, f in missing)
                )
                continue
            empty = [given[(m, f)] for m, f, _ in slots if not given[(m, f)].text]
            if empty:
                if len(empty) < len(slots):
                    untranslated.append(
                        f"{catalog}: {key!r} is partly filled "
                        f"({empty[0].where()} is empty); the key stays untranslated"
                    )
                else:
                    untranslated.append(f"{catalog}: {key!r} ({empty[0].where()})")
                continue
            values = {(m, f): given[(m, f)].text for m, f, _ in slots}
            bad = []
            for (m, f), text in values.items():
                reason = forbidden_spelling(text)
                if reason:
                    bad.append(f"{given[(m, f)].where()}: {reason}")
            if bad:
                refused.append(f"{catalog}: {key!r}: " + "; ".join(bad))
                continue
            localization = build_localization(entry, values, "needs_review" if draft else "translated")
            candidate = json.loads(json.dumps(entry))
            candidate.setdefault("localizations", {})[lang] = localization
            complaints: list[str] = []

            def complain(language, label, problem, _rows=given):
                if language != lang:
                    return
                head = label.split(".")[0] if label else ""
                member = head if head in entry.get("localizations", {}).get(SOURCE, {}).get("substitutions", {}) else ""
                form = label.split(".")[-1] if label and (label.startswith("plural.") or ".plural." in label) else ""
                row = _rows.get((member, form)) or _rows.get(("", ""))
                complaints.append(f"{row.where() if row else '?'}: {problem}")

            lint.lint_arguments(key, candidate, complain)
            if complaints:
                refused.append(f"{catalog}: {key!r}: " + "; ".join(complaints))
                continue
            strings[key].setdefault("localizations", {})[lang] = localization
            imported[catalog] += 1
        if write:
            catalog_path(catalog).write_text(dump_catalog(document), encoding="utf-8")

    for (catalog, key), group in by_key.items():
        if (catalog, key) not in seen:
            if catalog not in CATALOGS:
                refused.append(f"{group[0].where()}: unknown catalog {catalog!r}")
            else:
                notices.append(
                    f"{catalog}: {key!r} ({group[0].where()}) is not in the catalog any more; "
                    "its rows were ignored"
                )

    for catalog in CATALOGS:
        verb = "importable" if not write else "imported as drafts" if draft else "imported"
        print(f"l10n-sheet: {catalog}: {imported[catalog]} key(s) {verb} for '{lang}'")
    for line in notices:
        print(f"l10n-sheet: note: {line}")
    for line in untranslated:
        print(f"l10n-sheet: untranslated: {line}", file=sys.stderr)
    for line in refused:
        print(f"l10n-sheet: refused: {line}", file=sys.stderr)
    if refused or untranslated:
        print(
            f"l10n-sheet: {len(refused)} refused, {len(untranslated)} untranslated"
            + ("" if write else " (nothing written)"),
            file=sys.stderr,
        )
        return 1
    if not write:
        print("l10n-sheet: worksheet is complete and valid (nothing written).")
    return 0


# ── entry point ─────────────────────────────────────────────────────────────


USAGE = (
    "usage: scripts/l10n-sheet.py export <lang> <out-dir> [--pending]"
    " | import <lang> <csv>... [--check] [--draft]"
)


def main(argv: list[str]) -> int:
    args = argv[1:]
    if len(args) >= 3 and args[0] == "export":
        pending = "--pending" in args
        rest = [a for a in args[1:] if a != "--pending"]
        if len(rest) != 2:
            print(USAGE, file=sys.stderr)
            return 2
        return export(rest[0], Path(rest[1]), pending=pending)
    if len(args) >= 3 and args[0] == "import":
        check = "--check" in args
        draft = "--draft" in args
        paths = [Path(a) for a in args[2:] if a not in ("--check", "--draft")]
        if not paths:
            print(USAGE, file=sys.stderr)
            return 2
        return import_rows(args[1], paths, write=not check, draft=draft)
    print(USAGE, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
