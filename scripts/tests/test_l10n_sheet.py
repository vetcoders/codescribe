"""Tests for scripts/l10n-sheet.py: the translator worksheet round trip.

A small pair of catalogs is exported to CSV, filled, and imported back; the
refusals are the ones that would otherwise ship a broken string. The last test
holds the JSON writer to the byte layout of the catalogs the app ships, so an
import never reformats them.
"""

import csv
import importlib.util
import io
import json
import os
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "l10n-sheet.py"
SPEC = importlib.util.spec_from_file_location("l10n_sheet", SCRIPT)
sheet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sheet)
lint = sheet.lint

SHIPPED_CATALOG_DIR = sheet.CATALOG_DIR


def unit(value: str, state: str = "translated") -> dict:
    return {"stringUnit": {"state": state, "value": value}}


def plural(**forms: str) -> dict:
    return {"variations": {"plural": {case: unit(value) for case, value in forms.items()}}}


LOCALIZABLE = {
    "Save": {},
    "Commit: %@": {"comment": "About panel; %@ is a commit hash"},
    "%@\nEndpoint: %@": {
        "extractionState": "extracted_with_value",
        "localizations": {"en": unit("%1$@\nEndpoint: %2$@", "new")},
    },
    "%lld files": {"localizations": {"en": plural(one="%lld file", other="%lld files")}},
    "%@, %lld tools": {
        "localizations": {
            "en": {
                **unit("%1$@, %#@tools@"),
                "substitutions": {
                    "tools": {
                        "argNum": 2,
                        "formatSpecifier": "lld",
                        **plural(one="%arg tool", other="%arg tools"),
                    }
                },
            }
        }
    },
    "Space Grotesk — body 18": {"shouldTranslate": False},
    "": {"shouldTranslate": False},
}
INFOPLIST = {
    "NSMicrophoneUsageDescription": {
        "extractionState": "manual",
        "localizations": {"en": unit("Codescribe transcribes your speech locally.")},
    }
}
POLISH = {
    ("Localizable", "Save", "", ""): "Zapisz",
    ("Localizable", "Commit: %@", "", ""): "Commit: %@",
    ("Localizable", "%@\nEndpoint: %@", "", ""): "%1$@\nAdres: %2$@",
    ("Localizable", "%lld files", "", "one"): "%lld plik",
    ("Localizable", "%lld files", "", "few"): "%lld pliki",
    ("Localizable", "%lld files", "", "many"): "%lld plików",
    ("Localizable", "%lld files", "", "other"): "%lld pliku",
    ("Localizable", "%@, %lld tools", "", ""): "%1$@, %#@tools@",
    ("Localizable", "%@, %lld tools", "tools", "one"): "%arg narzędzie",
    ("Localizable", "%@, %lld tools", "tools", "few"): "%arg narzędzia",
    ("Localizable", "%@, %lld tools", "tools", "many"): "%arg narzędzi",
    ("Localizable", "%@, %lld tools", "tools", "other"): "%arg narzędzia",
    ("InfoPlist", "NSMicrophoneUsageDescription", "", ""): "Codescribe transkrybuje Twoją mowę lokalnie.",
}


class WorksheetCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.catalogs = root / "catalogs"
        self.catalogs.mkdir()
        self.out = root / "sheet"
        self.write_catalog("Localizable", LOCALIZABLE)
        self.write_catalog("InfoPlist", INFOPLIST)
        sheet.CATALOG_DIR = self.catalogs
        os.environ["L10N_DERIVED"] = str(root / "no-build")

    def tearDown(self):
        sheet.CATALOG_DIR = SHIPPED_CATALOG_DIR
        os.environ.pop("L10N_DERIVED", None)
        self.tmp.cleanup()

    def write_catalog(self, name: str, strings: dict) -> None:
        catalog = {"sourceLanguage": "en", "strings": strings, "version": "1.0"}
        (self.catalogs / f"{name}.xcstrings").write_text(
            sheet.dump_catalog(catalog), encoding="utf-8"
        )

    def read_catalog(self, name: str) -> dict:
        return json.loads((self.catalogs / f"{name}.xcstrings").read_text(encoding="utf-8"))

    def export(self) -> list[Path]:
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            self.assertEqual(sheet.main(["l10n-sheet", "export", "pl", str(self.out)]), 0)
        return sorted(self.out.glob("*.csv"))

    def fill(self, paths: list[Path], translations: dict, edit=None) -> None:
        for path in paths:
            with path.open(encoding="utf-8") as handle:
                rows = list(csv.reader(handle))
            for row in rows[1:]:
                key = (row[0], row[1], row[2], row[3])
                row[8] = translations.get(key, "")
                if edit:
                    row[8] = edit(key, row[8])
            with path.open("w", newline="", encoding="utf-8") as handle:
                csv.writer(handle).writerows(rows)

    def import_(self, paths: list[Path], check: bool = False) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        args = ["l10n-sheet", "import", "pl", *map(str, paths)] + (["--check"] if check else [])
        with redirect_stdout(out), redirect_stderr(err):
            status = sheet.main(args)
        return status, out.getvalue(), err.getvalue()

    def lint_errors(self) -> list[str]:
        errors: list[str] = []
        paths = sorted(self.catalogs.glob("*.xcstrings"))
        languages = set()
        for path in paths:
            languages |= lint.catalog_languages(path)
        for path in paths:
            lint.lint_catalog(path, errors, [], languages)
        return errors


class RoundTripTests(WorksheetCase):
    def test_export_writes_one_row_per_string_a_translator_owes(self):
        paths = self.export()
        self.assertEqual([p.name for p in paths], ["codescribe-pl-InfoPlist.csv", "codescribe-pl-Localizable.csv"])
        with paths[1].open(encoding="utf-8") as handle:
            rows = list(csv.reader(handle))
        self.assertEqual(rows[0], list(sheet.HEADERS["pl"]))
        slots = [(r[1], r[2], r[3]) for r in rows[1:]]
        # plain, identifier, 4 Polish forms, main row + 4 forms of one substitution; nothing for shouldTranslate: false
        self.assertEqual(len(slots), 1 + 1 + 1 + 4 + 5)
        self.assertIn(("%lld files", "", "few"), slots)
        self.assertIn(("%@, %lld tools", "tools", "many"), slots)
        self.assertNotIn(("Space Grotesk — body 18", "", ""), slots)
        self.assertNotIn(("", "", ""), slots)
        english = {(r[1], r[2], r[3]): r[7] for r in rows[1:]}
        self.assertEqual(english[("%@\nEndpoint: %@", "", "")], "%1$@\nEndpoint: %2$@")
        self.assertEqual(english[("%lld files", "", "many")], "%lld files")  # Polish forms English lacks take `other`

    def test_a_filled_worksheet_imports_whole_and_passes_the_lint(self):
        paths = self.export()
        self.fill(paths, POLISH)
        status, out, err = self.import_(paths)
        self.assertEqual(status, 0, err)
        self.assertIn("Localizable: 5 key(s) imported", out)
        self.assertIn("InfoPlist: 1 key(s) imported", out)
        strings = self.read_catalog("Localizable")["strings"]
        self.assertEqual(strings["Save"]["localizations"]["pl"], unit("Zapisz"))
        self.assertEqual(
            strings["%lld files"]["localizations"]["pl"]["variations"]["plural"]["many"],
            unit("%lld plików"),
        )
        tools = strings["%@, %lld tools"]["localizations"]["pl"]
        self.assertEqual(tools["stringUnit"]["value"], "%1$@, %#@tools@")
        self.assertEqual(tools["substitutions"]["tools"]["argNum"], 2)
        self.assertEqual(tools["substitutions"]["tools"]["formatSpecifier"], "lld")
        self.assertEqual(tools["substitutions"]["tools"]["variations"]["plural"]["few"], unit("%arg narzędzia"))
        self.assertNotIn("localizations", strings["Space Grotesk — body 18"])
        self.assertEqual(strings["%@\nEndpoint: %@"]["localizations"]["en"], unit("%1$@\nEndpoint: %2$@", "new"))
        self.assertEqual(self.read_catalog("InfoPlist")["strings"]["NSMicrophoneUsageDescription"]["localizations"]["pl"]["stringUnit"]["value"], "Codescribe transkrybuje Twoją mowę lokalnie.")
        self.assertEqual(self.lint_errors(), [])

    def test_the_worksheet_of_a_translated_language_carries_its_translations(self):
        paths = self.export()
        self.fill(paths, POLISH)
        self.import_(paths)
        again = self.export()  # overwrites the worksheet from the translated catalog
        with again[1].open(encoding="utf-8") as handle:
            rows = list(csv.reader(handle))
        polish = {(r[1], r[2], r[3]): r[8] for r in rows[1:]}
        self.assertEqual(polish[("Save", "", "")], "Zapisz")
        self.assertEqual(polish[("%lld files", "", "many")], "%lld plików")
        self.assertEqual(polish[("%@, %lld tools", "tools", "few")], "%arg narzędzia")
        self.assertEqual(polish[("%@, %lld tools", "", "")], "%1$@, %#@tools@")
        before = {p.name: p.read_text(encoding="utf-8") for p in self.catalogs.glob("*.xcstrings")}
        status, _, err = self.import_(again)  # the untouched revision sheet imports to the same bytes
        self.assertEqual(status, 0, err)
        after = {p.name: p.read_text(encoding="utf-8") for p in self.catalogs.glob("*.xcstrings")}
        self.assertEqual(before, after)

    def test_check_reports_and_writes_nothing(self):
        paths = self.export()
        before = {p.name: p.read_text(encoding="utf-8") for p in self.catalogs.glob("*.xcstrings")}
        self.fill(paths, POLISH)
        status, out, _ = self.import_(paths, check=True)
        self.assertEqual(status, 0)
        self.assertIn("5 key(s) importable", out)
        after = {p.name: p.read_text(encoding="utf-8") for p in self.catalogs.glob("*.xcstrings")}
        self.assertEqual(before, after)


class RefusalTests(WorksheetCase):
    def test_a_plural_with_one_empty_form_stays_untranslated(self):
        paths = self.export()
        self.fill(paths, POLISH, edit=lambda key, text: "" if key[1:] == ("%lld files", "", "few") else text)
        status, out, err = self.import_(paths)
        self.assertEqual(status, 1)
        self.assertIn("'%lld files' is partly filled", err)
        self.assertIn("Localizable: 4 key(s) imported", out)
        self.assertNotIn("pl", self.read_catalog("Localizable")["strings"]["%lld files"]["localizations"])
        self.assertIn("partly translated", " ".join(self.lint_errors()))

    def test_a_translation_that_drops_an_argument_is_refused(self):
        paths = self.export()
        self.fill(paths, POLISH, edit=lambda key, text: "Commit" if key[1] == "Commit: %@" else text)
        status, _, err = self.import_(paths)
        self.assertEqual(status, 1)
        self.assertIn("'Commit: %@'", err)
        self.assertIn("argument 1 (%@) is missing", err)
        self.assertNotIn("localizations", self.read_catalog("Localizable")["strings"]["Commit: %@"])

    def test_a_substitution_form_that_swaps_the_count_is_refused(self):
        paths = self.export()
        self.fill(paths, POLISH, edit=lambda key, text: "%@ narzędzi" if key[2:] == ("tools", "many") else text)
        status, _, err = self.import_(paths)
        self.assertEqual(status, 1)
        self.assertIn("'%@, %lld tools'", err)
        self.assertIn("row 6:", err)  # header, main row, one, few, many: the refused row is named
        self.assertNotIn("pl", self.read_catalog("Localizable")["strings"]["%@, %lld tools"]["localizations"])

    def test_the_wrong_product_spelling_is_refused(self):
        paths = self.export()
        self.fill(paths, POLISH, edit=lambda key, text: text.replace("Codescribe", "CodeScribe"))
        status, _, err = self.import_(paths)
        self.assertEqual(status, 1)
        self.assertIn("spells `CodeScribe`", err)
        self.assertNotIn("pl", self.read_catalog("InfoPlist")["strings"]["NSMicrophoneUsageDescription"]["localizations"])

    def test_rows_for_a_key_that_left_the_catalog_are_ignored_with_a_note(self):
        paths = self.export()
        self.fill(paths, POLISH)
        path = paths[1]
        with path.open("a", newline="", encoding="utf-8") as handle:
            csv.writer(handle).writerow(["Localizable", "Gone", "", "", "", "", "", "Gone", "Zniknęło", ""])
        status, out, _ = self.import_(paths)
        self.assertEqual(status, 0)
        self.assertIn("'Gone'", out)
        self.assertIn("not in the catalog any more", out)
        self.assertNotIn("Gone", self.read_catalog("Localizable")["strings"])

    def test_a_foreign_worksheet_is_refused(self):
        bad = Path(self.tmp.name) / "other.csv"
        bad.write_text("a,b,c\n1,2,3\n", encoding="utf-8")
        with self.assertRaises(SystemExit):
            with redirect_stderr(io.StringIO()):
                sheet.main(["l10n-sheet", "import", "pl", str(bad)])


class LayoutTests(unittest.TestCase):
    def test_the_writer_reproduces_the_shipped_catalogs_byte_for_byte(self):
        for path in sorted(SHIPPED_CATALOG_DIR.glob("*.xcstrings")):
            text = path.read_text(encoding="utf-8")
            self.assertEqual(sheet.dump_catalog(json.loads(text)), text, path.name)


if __name__ == "__main__":
    unittest.main()
