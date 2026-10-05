"""Tests for scripts/l10n-lint.py: argument numbers and types, plural forms,
and the plural categories each language owes.

Each refusal below was once accepted. The catalogs are built in a temp
directory; the last test runs the linter over the catalogs the app ships.
"""

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "l10n-lint.py"
SPEC = importlib.util.spec_from_file_location("l10n_lint", SCRIPT)
lint = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(lint)

HAS_FILES = "%@ has %lld files"
TRASH = "Moves %lld Agent threads and %lld Agent files to Trash."
FILES = "%lld files"
POLISH_CASES = ("one", "few", "many", "other")
ARABIC_CASES = ("zero", "one", "two", "few", "many", "other")
# cldr-json 48.2.3, cldr-core/supplemental/plurals.json
PLURAL_RULES_SHA256 = "6c0a48e9bcfc25856f90202f703c2c7f89c105d6868f7712a943a6ed2dcbe8f4"


def unit(value: str) -> dict:
    return {"stringUnit": {"state": "translated", "value": value}}


def plural(**forms: str) -> dict:
    return {"variations": {"plural": {case: unit(value) for case, value in forms.items()}}}


def substitution(number: int, **forms: str) -> dict:
    return {"argNum": number, "formatSpecifier": "lld", **plural(**forms)}


def trash_entry(files_other: str = "%arg Agent files") -> dict:
    """The shipped shape of a sentence with two counts."""
    return {
        "localizations": {
            "en": {
                **unit("Moves %#@threads@ and %#@files@ to Trash."),
                "substitutions": {
                    "threads": substitution(1, one="%arg Agent thread", other="%arg Agent threads"),
                    "files": substitution(2, one="%arg Agent file", other=files_other),
                },
            }
        }
    }


def problems(strings: dict) -> list[str]:
    errors: list[str] = []
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "Localizable.xcstrings"
        catalog = {"sourceLanguage": "en", "strings": strings, "version": "1.0"}
        path.write_text(json.dumps(catalog), encoding="utf-8")
        lint.lint_catalog(path, errors, [])
    return errors


class ArgumentTests(unittest.TestCase):
    def test_swapped_types_are_refused(self):
        errors = problems({HAS_FILES: {"localizations": {"pl": unit("%2$@ ma %1$lld plików")}}})
        self.assertEqual(len(errors), 2, errors)
        self.assertIn("argument 1 is %lld, the source has %@", errors[0])
        self.assertIn("argument 2 is %@, the source has %lld", errors[1])

    def test_reordering_with_matching_types_passes(self):
        errors = problems({HAS_FILES: {"localizations": {"pl": unit("%2$lld plików — %1$@")}}})
        self.assertEqual(errors, [])

    def test_a_plural_form_cannot_drop_a_non_count_argument(self):
        forms = plural(**{case: "%lld plików" for case in POLISH_CASES})
        errors = problems({HAS_FILES: {"localizations": {"pl": forms}}})
        self.assertEqual(len(errors), 4, errors)
        for case in POLISH_CASES:
            expected = f"pl [plural.{case}]: argument 1 is %lld, the source has %@"
            self.assertTrue(any(expected in error for error in errors), errors)

        numbered = plural(**{case: "%2$lld plików" for case in POLISH_CASES})
        errors = problems({HAS_FILES: {"localizations": {"pl": numbered}}})
        self.assertEqual(len(errors), 4, errors)
        self.assertTrue(all("argument 1 (%@) is missing" in error for error in errors), errors)

    def test_a_plural_form_may_spell_out_its_count(self):
        forms = plural(
            one="%1$@ ma jeden plik",
            few="%1$@ ma %2$lld pliki",
            many="%1$@ ma %2$lld plików",
            other="%1$@ ma %2$lld pliku",
        )
        self.assertEqual(problems({HAS_FILES: {"localizations": {"pl": forms}}}), [])
        english = plural(one="One recording", other="%lld recordings")
        self.assertEqual(problems({"%lld recordings": {"localizations": {"en": english}}}), [])

    def test_dropping_the_count_does_not_renumber_what_follows(self):
        # Unnumbered, `%@` is argument 1 — the integer — once the count is gone.
        forms = plural(one="One file in %@", other="%lld files in %@")
        errors = problems({"%lld files in %@": {"localizations": {"en": forms}}})
        self.assertEqual(len(errors), 2, errors)
        self.assertIn("en [plural.one]: argument 1 is %@, the source has %lld", errors[0])
        self.assertIn("en [plural.one]: argument 2 (%@) is missing", errors[1])

        forms = plural(one="One file in %2$@", other="%lld files in %@")
        self.assertEqual(problems({"%lld files in %@": {"localizations": {"en": forms}}}), [])

    def test_whole_string_plural_needs_exactly_one_count(self):
        forms = plural(one="%lld of %lld", other="%lld of %lld")
        errors = problems({"%lld of %lld": {"localizations": {"en": forms}}})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("plural forms on a string with 2 integer arguments", errors[0])


class SubstitutionTests(unittest.TestCase):
    def test_shipped_shape_passes(self):
        self.assertEqual(problems({TRASH: trash_entry()}), [])

    def test_several_counts_and_a_numbered_argument_pass(self):
        key = "%lld recordings from %lld days, %lld threads (%@ MB)"
        entry = {
            "localizations": {
                "en": {
                    **unit("%#@recordings@ from %#@days@, %#@threads@ (%4$@ MB)"),
                    "substitutions": {
                        "recordings": substitution(1, one="%arg recording", other="%arg recordings"),
                        "days": substitution(2, one="%arg day", other="%arg days"),
                        "threads": substitution(3, one="One thread", other="%arg threads"),
                    },
                }
            }
        }
        self.assertEqual(problems({key: entry}), [])

    def test_a_form_cannot_add_an_argument(self):
        errors = problems({TRASH: trash_entry("%arg Agent files from %@")})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("en [files.plural.other]", errors[0])
        self.assertIn("inside a substitution has no argument number", errors[0])

        errors = problems({TRASH: trash_entry("%arg Agent files from %3$@")})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("argument 3 (%@) does not exist in the source", errors[0])

    def test_a_form_cannot_read_an_argument_as_another_type(self):
        errors = problems({TRASH: trash_entry("%arg Agent files from %1$@")})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("argument 1 is %@, the source has %lld", errors[0])

    def test_a_substitution_must_format_the_argument_the_source_has(self):
        entry = trash_entry()
        entry["localizations"]["en"]["substitutions"]["files"]["argNum"] = 1
        errors = problems({TRASH: entry})
        self.assertTrue(any("argument 2 (%lld) is missing" in error for error in errors), errors)

        entry = trash_entry()
        entry["localizations"]["en"]["substitutions"]["files"]["formatSpecifier"] = "@"
        errors = problems({TRASH: entry})
        self.assertTrue(
            any("argument 2 is %@, the source has %lld" in error for error in errors), errors
        )

    def test_a_substitution_without_plural_forms_cannot_drop_its_count(self):
        # xcstringstool: "Cannot reference 'files' from here because it is not
        # a plural variation".
        entry = trash_entry()
        rules = entry["localizations"]["en"]["substitutions"]
        rules["files"] = {"argNum": 2, "formatSpecifier": "lld", **unit("Agent files")}
        errors = problems({TRASH: entry})
        self.assertEqual(len(errors), 2, errors)
        self.assertIn("en [files]: substitution has no plural forms", errors[0])
        self.assertIn("en [files]: argument 2 (%lld) is missing", errors[1])

        rules["files"] = {"argNum": 2, "formatSpecifier": "lld", **unit("%arg Agent files")}
        errors = problems({TRASH: entry})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("en [files]: substitution has no plural forms", errors[0])

    def test_arg_outside_a_substitution_is_refused(self):
        errors = problems({"%lld files": {"localizations": {"pl": unit("%arg plików")}}})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("`%arg` is only meaningful inside a substitution", errors[0])

    def test_mixed_numbering_is_refused(self):
        errors = problems({HAS_FILES: {"localizations": {"pl": unit("%1$@ ma %lld plików")}}})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("numbered and unnumbered arguments are mixed", errors[0])

    def test_plural_forms_are_required_per_language_inside_a_substitution(self):
        entry = trash_entry()
        entry["localizations"]["pl"] = {
            **unit("Przenosi %#@threads@ i %#@files@ do Kosza."),
            "substitutions": {
                "threads": substitution(1, one="%arg wątek", other="%arg wątku"),
                "files": substitution(
                    2, one="%arg plik", few="%arg pliki", many="%arg plików", other="%arg pliku"
                ),
            },
        }
        errors = problems({TRASH: entry})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("pl [threads]: plural forms missing (few, many)", errors[0])


class PluralCategoryTests(unittest.TestCase):
    """The forms a language owes come from CLDR, not from a list in the lint."""

    def test_a_language_owes_every_category_cldr_gives_it(self):
        def missing(language: str, *cases: str) -> list[str]:
            forms = plural(**{case: "%lld" for case in cases})
            return problems({FILES: {"localizations": {language: forms}}})

        # More forms than English or Polish, fewer, and a different set.
        errors = missing("ar", "one", "few", "many", "other")
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("ar: plural forms missing (zero, two)", errors[0])
        self.assertEqual(missing("ar", *ARABIC_CASES), [])

        self.assertEqual(missing("ja", "other"), [])

        errors = missing("fr", "one", "other")
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("fr: plural forms missing (many)", errors[0])
        self.assertEqual(missing("fr", "one", "many", "other"), [])

    def test_other_alone_no_longer_passes_for_a_language_with_more_forms(self):
        for language, absent in (("ru", "one, few, many"), ("cs", "one, few, many"), ("he", "one, two")):
            forms = plural(other="%lld")
            errors = problems({FILES: {"localizations": {language: forms}}})
            self.assertEqual(len(errors), 1, errors)
            self.assertIn(f"{language}: plural forms missing ({absent})", errors[0])

    def test_a_language_owes_its_categories_inside_a_substitution(self):
        entry = trash_entry()
        entry["localizations"]["ar"] = {
            **unit("%#@threads@ %#@files@"),
            "substitutions": {
                "threads": substitution(1, **{case: "%arg" for case in ARABIC_CASES}),
                "files": substitution(2, one="%arg", other="%arg"),
            },
        }
        errors = problems({TRASH: entry})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("ar [files]: plural forms missing (zero, two, few, many)", errors[0])

    def test_english_and_polish_keep_their_categories(self):
        self.assertEqual(lint.plural_categories("en"), ("one", "other"))
        self.assertEqual(lint.plural_categories("pl"), POLISH_CASES)

    def test_a_script_or_region_takes_the_rules_of_the_longest_known_prefix(self):
        rules = lint.plural_rules()
        self.assertIn("pt-pt", rules)
        self.assertNotIn("pt-br", rules)
        self.assertNotIn("zh-hans", rules)
        self.assertEqual(lint.plural_categories("pt-PT"), rules["pt-pt"])
        self.assertEqual(lint.plural_categories("pt-BR"), rules["pt"])
        self.assertEqual(lint.plural_categories("zh-Hans"), ("other",))
        self.assertEqual(lint.plural_categories("sr-Latn-RS"), ("one", "few", "other"))
        self.assertEqual(lint.plural_categories("en_GB"), ("one", "other"))

    def test_a_language_cldr_has_no_rules_for_is_refused_not_held_to_less(self):
        self.assertIsNone(lint.plural_categories("zz"))
        forms = plural(other="%lld")
        errors = problems(
            {
                FILES: {"localizations": {"zz": forms}},
                "%lld days": {"localizations": {"zz": forms}},
            }
        )
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("language 'zz' has plural forms", errors[0])
        self.assertIn("scripts/data/cldr/plurals.json has no plural rules for it", errors[0])
        # Without plural forms there is nothing the rules would decide.
        self.assertEqual(problems({"Save": {"localizations": {"zz": unit("Zz")}}}), [])

    def test_the_plural_rules_are_the_published_cldr_file(self):
        rules = lint.plural_rules()
        self.assertGreater(len(rules), 200)
        self.assertTrue(all("other" in categories for categories in rules.values()))
        digest = hashlib.sha256(lint.PLURAL_RULES.read_bytes()).hexdigest()
        self.assertEqual(digest, PLURAL_RULES_SHA256, "refresh from cldr-json, never edit by hand")


class CoverageTests(unittest.TestCase):
    """A language the bundle carries is owed in full, in both catalogs."""

    def lint(self, strings: dict, allow_partial: bool = False, languages=None) -> list[str]:
        errors: list[str] = []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Localizable.xcstrings"
            catalog = {"sourceLanguage": "en", "strings": strings, "version": "1.0"}
            path.write_text(json.dumps(catalog), encoding="utf-8")
            lint.lint_catalog(path, errors, [], languages, allow_partial)
        return errors

    def test_a_partly_translated_language_is_refused(self):
        strings = {"Save": {"localizations": {"pl": unit("Zapisz")}}, "Cancel": {}}
        errors = self.lint(strings)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("partly translated (1/2)", errors[0])

    def test_allow_partial_turns_the_refusal_into_a_report(self):
        strings = {"Save": {"localizations": {"pl": unit("Zapisz")}}, "Cancel": {}}
        self.assertEqual(self.lint(strings, allow_partial=True), [])

    def test_a_complete_language_passes(self):
        strings = {
            "Save": {"localizations": {"pl": unit("Zapisz")}},
            "Cancel": {"localizations": {"pl": unit("Anuluj")}},
            "Lab": {"shouldTranslate": False},
        }
        self.assertEqual(self.lint(strings), [])

    def test_a_language_declared_elsewhere_is_owed_here_too(self):
        errors = self.lint({"Save": {}}, languages={"pl"})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("carries no translations", errors[0])

    def test_catalog_languages_names_what_a_catalog_carries(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "InfoPlist.xcstrings"
            strings = {"NSMicrophoneUsageDescription": {"localizations": {"en": unit("a"), "pl": unit("b")}}}
            path.write_text(json.dumps({"sourceLanguage": "en", "strings": strings}), encoding="utf-8")
            self.assertEqual(lint.catalog_languages(path), {"pl"})


class SpellingTests(unittest.TestCase):
    def test_the_wrong_product_spelling_is_refused_in_any_language(self):
        errors = problems({"Open CodeScribe": {"localizations": {"pl": unit("Otwórz Codescribe")}}})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("en: spells `CodeScribe`", errors[0])
        errors = problems({"Open Codescribe": {"localizations": {"pl": unit("Otwórz CodeScribe")}}})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("pl: spells `CodeScribe`", errors[0])

    def test_the_spelling_is_checked_inside_plural_forms_and_substitutions(self):
        entry = trash_entry()
        entry["localizations"]["en"]["substitutions"]["files"]["variations"]["plural"]["other"] = unit("%arg CodeScribe files")
        errors = problems({TRASH: entry})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("[files.plural.other]", errors[0])


class ShippedCatalogTests(unittest.TestCase):
    def test_shipped_catalogs_pass(self):
        errors: list[str] = []
        catalogs = sorted(lint.CATALOG_DIR.glob("*.xcstrings"))
        self.assertTrue(catalogs)
        for path in catalogs:
            lint.lint_catalog(path, errors, [])
        lint.lint_usage_descriptions(errors)
        self.assertEqual(errors, [])

    def test_the_reported_edit_to_a_shipped_key_is_refused(self):
        path = lint.CATALOG_DIR / "Localizable.xcstrings"
        strings = json.loads(path.read_text(encoding="utf-8"))["strings"]
        entry = strings[TRASH]
        other = entry["localizations"]["en"]["substitutions"]["files"]["variations"]["plural"]["other"]
        other["stringUnit"]["value"] = "%arg Agent files from %@"
        errors = problems({TRASH: entry})
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("en [files.plural.other]", errors[0])


if __name__ == "__main__":
    unittest.main()
