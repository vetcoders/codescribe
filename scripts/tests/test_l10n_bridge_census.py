"""Tests for scripts/l10n-bridge-census.py: the ratchet on English prose
crossing the Rust -> Swift bridge."""

import importlib.util
from pathlib import Path
import unittest

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


if __name__ == "__main__":
    unittest.main()
