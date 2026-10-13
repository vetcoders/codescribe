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

# The other three shapes Rust's text crosses in, written as UniFFI writes them:
# `protocol X` + `open class XImpl` is a callback (Rust calls Swift), and
# `protocol XProtocol` + `open class X` is an object (Swift calls Rust).
SIGNATURES = """
public protocol CsModeListener: AnyObject, Sendable {

    /**
     * Rust hands Swift a label and a code.
     */
    func onMode(label: String, code: String, count: Int64)

    /**
     * What Swift answers with is Swift's own string.
     */
    func describe(mode: CsWorkMode) -> String

}

public protocol CodescribeNotesProtocol: AnyObject, Sendable {

    func notesDir()  -> String

    func saveSelection() throws  -> String?

    func rename(title: String) throws

    func tags()  -> [String]

    func count()  -> Int64

}

open class CodescribeNotes: CodescribeNotesProtocol, @unchecked Sendable {
}

public func configRepairSummary() -> String?  {
    return nil
}

public func applyTheme(name: String)  {
}

public func snapshot()throws  -> CsSnapshot  {
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
        self.assertIn("CsNew.reason carries a String from Rust and is not classified", errors[0])
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

    def ratchet(self, classification: str, bindings: str | None = BINDINGS):
        return census.ratchet(classification, self.BASE, bindings)

    def test_an_unchanged_list_passes_and_reports_the_base_count(self):
        self.assertEqual(self.ratchet(CLASSIFIED), ([], [], [], 3))

    def test_retiring_a_seam_passes_and_the_departure_is_named(self):
        shorter = CLASSIFIED.replace("prose CsError.Agent.msg\n", "data  CsError.Agent.msg\n")
        added, visible, retired, before = self.ratchet(shorter)
        self.assertEqual((added, visible), ([], []))
        self.assertEqual(retired, ["CsError.Agent.msg"])
        self.assertEqual(before, 3)

    def test_a_new_prose_line_for_a_field_the_base_carried_is_refused(self):
        # CsModeBinding.count is in BINDINGS as an Int64 — not a seam at all —
        # so use a String the base classified as data: reclassifying launders it.
        grown = CLASSIFIED.replace("data  CsThread.title", "prose CsThread.title")
        added, visible, _, _ = self.ratchet(grown)
        self.assertEqual(added, ["CsThread.title"])
        self.assertEqual(visible, [])

    def test_a_new_prose_line_for_a_field_the_base_never_carried_is_refused(self):
        grown = CLASSIFIED + "prose CsThread.summary\n"
        added, visible, retired, before = self.ratchet(grown)
        self.assertEqual(added, ["CsThread.summary"])
        self.assertEqual((visible, retired), ([], []))
        self.assertGreater(len(census.prose_lines(grown)), before)

    def test_a_seam_the_base_carried_and_never_classified_is_newly_visible(self):
        # What a cut that widens the census does: it writes down debt that
        # shipped long ago. The base's own bindings are what prove it is not new.
        silent = CLASSIFIED.replace("data  CsThread.title\n", "")
        now = CLASSIFIED.replace("data  CsThread.title", "prose CsThread.title")
        added, visible, _, _ = census.ratchet(now, silent, BINDINGS)
        self.assertEqual(added, [])
        self.assertEqual(visible, ["CsThread.title"])

    def test_the_same_line_is_refused_where_the_base_bindings_lacked_the_field(self):
        silent = CLASSIFIED.replace("data  CsThread.title\n", "")
        now = CLASSIFIED.replace("data  CsThread.title", "prose CsThread.title")
        without = BINDINGS.replace("    public var title: String\n", "")
        self.assertNotIn("CsThread.title", census.bridge_fields(without))
        added, visible, _, _ = census.ratchet(now, silent, without)
        self.assertEqual(added, ["CsThread.title"])
        self.assertEqual(visible, [])

    def test_without_the_base_bindings_nothing_is_forgiven(self):
        silent = CLASSIFIED.replace("data  CsThread.title\n", "")
        now = CLASSIFIED.replace("data  CsThread.title", "prose CsThread.title")
        added, visible, _, _ = census.ratchet(now, silent, None)
        self.assertEqual(added, ["CsThread.title"])
        self.assertEqual(visible, [])


class BaseRefTests(unittest.TestCase):
    def test_the_classification_and_the_bindings_are_read_out_of_a_real_git_ref(self):
        self.assertIn("prose ", census.at_ref("HEAD", census.CLASSIFICATION))
        self.assertIn("public protocol Cs", census.at_ref("HEAD", census.BINDINGS))

    def test_a_ref_the_clone_does_not_have_is_absent_not_fatal(self):
        ref = "no/such/ref-for-the-census-test"
        self.assertIsNone(census.at_ref(ref, census.CLASSIFICATION))
        self.assertIsNone(census.at_ref(ref, census.BINDINGS))


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
        """main() with the base classification replaced; the base bindings stay
        the shipped ones, which is what a same-bindings base looks like."""
        shipped = census.BINDINGS.read_text(encoding="utf-8")
        answers = {census.CLASSIFICATION: base, census.BINDINGS: shipped}
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(census, "at_ref", lambda ref, path: answers[path]):
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = census.main(["l10n-bridge-census.py", "--base", "pretend/base"])
        return code, out.getvalue(), err.getvalue()

    def test_the_shipped_tree_passes_against_its_own_classification(self):
        code, out, err = self.run_against(census.CLASSIFICATION.read_text(encoding="utf-8"))
        self.assertEqual(code, 0, err)
        self.assertIn("prose seams here", out)

    def test_the_shipped_tree_fails_a_base_that_called_one_of_its_seams_data(self):
        shipped = census.CLASSIFICATION.read_text(encoding="utf-8")
        first = next(line for line in shipped.splitlines() if line.startswith("prose "))
        base = shipped.replace(first, first.replace("prose", "data ", 1), 1)
        code, _, err = self.run_against(base)
        self.assertEqual(code, 1)
        self.assertIn("the prose list grew against", err)
        self.assertIn(first.split()[1], err)

    def test_the_shipped_tree_passes_a_base_that_had_not_classified_a_seam_yet(self):
        # The census widening that found it: the base's bindings carried it.
        shipped = census.CLASSIFICATION.read_text(encoding="utf-8")
        first = next(line for line in shipped.splitlines() if line.startswith("prose "))
        code, out, err = self.run_against(shipped.replace(first + "\n", "", 1))
        self.assertEqual(code, 0, err)
        self.assertIn(f"newly visible: {first.split()[1]}", out)

    def test_an_unreachable_base_says_so_instead_of_passing_quietly(self):
        code, _, err = self.run_against(None)
        self.assertEqual(code, 0)
        self.assertIn("the prose ratchet is NOT enforced", err)


class SignatureTests(unittest.TestCase):
    """Records were not the only way across: a callback argument and a return
    carry Rust's text too, and the census was blind to both."""

    def test_callback_arguments_are_counted_and_its_return_is_not(self):
        fields = census.bridge_fields(SIGNATURES)
        self.assertIn("CsModeListener.onMode.label", fields)
        self.assertIn("CsModeListener.onMode.code", fields)
        self.assertNotIn("CsModeListener.describe()", fields)

    def test_an_object_return_is_counted_and_its_arguments_are_not(self):
        fields = census.bridge_fields(SIGNATURES)
        self.assertIn("CodescribeNotes.notesDir()", fields)
        self.assertIn("CodescribeNotes.saveSelection()", fields)
        self.assertIn("CodescribeNotes.tags()", fields)
        self.assertNotIn("CodescribeNotes.rename.title", fields)
        self.assertNotIn("CodescribeNotes.rename()", fields)

    def test_a_free_function_returning_a_string_is_counted(self):
        fields = census.bridge_fields(SIGNATURES)
        self.assertIn("configRepairSummary()", fields)
        self.assertNotIn("applyTheme.name", fields)

    def test_nothing_but_a_string_return_is_counted(self):
        fields = census.bridge_fields(SIGNATURES)
        self.assertNotIn("CodescribeNotes.count()", fields)
        self.assertNotIn("snapshot()", fields)

    def test_an_unclassified_callback_argument_fails_like_any_other_seam(self):
        errors, _, _ = census.census(BINDINGS + SIGNATURES, CLASSIFIED)
        unclassified = [error for error in errors if "carries a String from Rust" in error]
        self.assertTrue(any("CsModeListener.onMode.label" in error for error in unclassified))
        self.assertTrue(any("CodescribeNotes.notesDir()" in error for error in unclassified))
        self.assertTrue(any("configRepairSummary()" in error for error in unclassified))

    def test_the_shipped_bindings_carry_all_four_shapes(self):
        fields = census.bridge_fields(census.BINDINGS.read_text(encoding="utf-8"))
        self.assertTrue(any(field.count(".") == 1 and not field.endswith("()") for field in fields))
        self.assertTrue(any(field.count(".") == 2 for field in fields))
        self.assertTrue(any(field.count(".") == 1 and field.endswith("()") for field in fields))
        self.assertTrue(any(field.count(".") == 0 for field in fields))


if __name__ == "__main__":
    unittest.main()
