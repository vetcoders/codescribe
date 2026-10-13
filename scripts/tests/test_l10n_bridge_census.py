"""Tests for scripts/l10n-bridge-census.py: the ratchet on English prose
crossing the Rust -> Swift bridge."""

import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "l10n-bridge-census.py"
SPEC = importlib.util.spec_from_file_location("l10n_bridge_census", SCRIPT)
census = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(census)

BINDINGS = """
public struct CsModeBinding: Equatable, Hashable {
    public var mode: CsWorkMode
    public var modeLabel: String
    public var bindingLabel: String?
    public var tags: [String]
    public var count: Int64
    public var env: [String: String]
}

public enum CsError {
    case Agent(msg: String)
    case Quota(limit: Int64)
}

public struct CsThread: Equatable, Hashable {
    public var id: String
    public var title: String
}
"""

CLASSIFIED = """# comment
prose CsModeBinding.modeLabel
prose CsModeBinding.bindingLabel
data  CsModeBinding.tags
prose CsError.Agent.msg
data  CsThread.id
data  CsThread.title
"""


class CensusTests(unittest.TestCase):
    def test_every_string_field_and_enum_payload_is_found_in_source_order(self):
        self.assertEqual(
            census.bridge_fields(BINDINGS),
            [
                "CsModeBinding.modeLabel",
                "CsModeBinding.bindingLabel",
                "CsModeBinding.tags",
                "CsError.Agent.msg",
                "CsThread.id",
                "CsThread.title",
            ],
        )

    def test_a_fully_classified_bridge_passes_and_counts_the_prose(self):
        errors, prose, counts = census.census(BINDINGS, CLASSIFIED)
        self.assertEqual(errors, [])
        self.assertEqual(prose, ["CsModeBinding.modeLabel", "CsModeBinding.bindingLabel", "CsError.Agent.msg"])
        self.assertEqual(counts, {"data": 3, "prose": 3})

    def test_a_new_string_field_fails_until_it_is_classified(self):
        errors, _, _ = census.census(BINDINGS + "\npublic struct CsNew: Equatable {\n    public var reason: String\n}\n", CLASSIFIED)
        self.assertEqual(len(errors), 1)
        self.assertIn("CsNew.reason crosses the bridge as a String and is not classified", errors[0])
        self.assertIn("LOCALIZATION_LEDGER.md §4", errors[0])

    def test_a_classified_field_the_bindings_dropped_fails_so_the_list_stays_true(self):
        errors, _, _ = census.census(BINDINGS, CLASSIFIED + "prose CsGone.message\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("CsGone.message is classified", errors[0])
        self.assertIn("remove the line", errors[0])

    def test_a_malformed_or_duplicate_line_is_refused(self):
        errors, _, _ = census.census(BINDINGS, CLASSIFIED + "maybe CsThread.id\ndata CsThread.id\n")
        self.assertTrue(any("expected `data|prose <Record.field>`" in e for e in errors))
        self.assertTrue(any("CsThread.id is listed twice" in e for e in errors))

    def test_the_shipped_bindings_are_fully_classified(self):
        errors, prose, counts = census.census(
            census.BINDINGS.read_text(encoding="utf-8"),
            census.CLASSIFICATION.read_text(encoding="utf-8"),
        )
        self.assertEqual(errors, [])
        self.assertGreater(counts["data"], 0)
        self.assertEqual(len(prose), counts["prose"])


class RatchetTests(unittest.TestCase):
    """`--base <ref>`: the prose list is a burn-down, so it may not grow."""

    BASE = CLASSIFIED

    def test_an_unchanged_list_passes_and_reports_the_base_count(self):
        added, retired, before = census.ratchet(CLASSIFIED, self.BASE)
        self.assertEqual((added, retired, before), ([], [], 3))

    def test_retiring_a_seam_passes_and_the_departure_is_named(self):
        shorter = CLASSIFIED.replace("prose CsError.Agent.msg\n", "data  CsError.Agent.msg\n")
        added, retired, before = census.ratchet(shorter, self.BASE)
        self.assertEqual(added, [])
        self.assertEqual(retired, ["CsError.Agent.msg"])
        self.assertEqual(before, 3)

    def test_rewording_a_seam_under_a_new_name_is_not_growth(self):
        renamed = CLASSIFIED.replace("prose CsError.Agent.msg", "prose CsError.Agent.detail")
        added, retired, before = census.ratchet(renamed, self.BASE)
        self.assertEqual(added, ["CsError.Agent.detail"])
        self.assertEqual(retired, ["CsError.Agent.msg"])
        self.assertEqual(len(census.prose_lines(renamed)), before)

    def test_a_new_prose_line_is_growth_and_is_named(self):
        grown = CLASSIFIED + "prose CsThread.summary\n"
        added, retired, before = census.ratchet(grown, self.BASE)
        self.assertEqual(added, ["CsThread.summary"])
        self.assertEqual(retired, [])
        self.assertGreater(len(census.prose_lines(grown)), before)

    def test_a_field_reclassified_from_data_to_prose_is_growth(self):
        grown = CLASSIFIED.replace("data  CsThread.title", "prose CsThread.title")
        added, _, _ = census.ratchet(grown, self.BASE)
        self.assertEqual(added, ["CsThread.title"])


class BaseRefTests(unittest.TestCase):
    def test_the_classification_is_read_out_of_a_real_git_ref(self):
        text = census.base_classification("HEAD")
        self.assertIsNotNone(text)
        self.assertIn("prose ", text)

    def test_a_ref_the_clone_does_not_have_is_absent_not_fatal(self):
        self.assertIsNone(census.base_classification("no/such/ref-for-the-census-test"))


class CommandLineTests(unittest.TestCase):
    def test_the_base_ref_is_read_in_either_spelling(self):
        self.assertEqual(census.parse(["--base", "origin/main"]), (False, "origin/main"))
        self.assertEqual(census.parse(["--base=origin/main", "--report"]), (True, "origin/main"))
        self.assertEqual(census.parse([]), (False, None))

    def test_a_base_without_a_ref_or_an_unknown_flag_is_refused(self):
        self.assertIsNone(census.parse(["--base"]))
        self.assertIsNone(census.parse(["--base="]))
        self.assertIsNone(census.parse(["--whatever"]))

    def run_against(self, base: str | None) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(census, "base_classification", return_value=base):
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = census.main(["l10n-bridge-census.py", "--base", "pretend/base"])
        return code, out.getvalue(), err.getvalue()

    def test_the_shipped_tree_passes_against_its_own_classification(self):
        code, out, err = self.run_against(census.CLASSIFICATION.read_text(encoding="utf-8"))
        self.assertEqual(code, 0, err)
        self.assertIn("prose seams here", out)

    def test_the_shipped_tree_fails_a_base_that_carried_one_seam_fewer(self):
        shipped = census.CLASSIFICATION.read_text(encoding="utf-8")
        first = next(line for line in shipped.splitlines() if line.startswith("prose "))
        code, _, err = self.run_against(shipped.replace(first + "\n", "", 1))
        self.assertEqual(code, 1)
        self.assertIn("the prose list grew against", err)
        self.assertIn(first.split()[1], err)

    def test_an_unreachable_base_says_so_instead_of_passing_quietly(self):
        code, _, err = self.run_against(None)
        self.assertEqual(code, 0)
        self.assertIn("the prose ratchet is NOT enforced", err)


if __name__ == "__main__":
    unittest.main()
