from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "verify-acoustic-throne-structure.py"
SPEC = importlib.util.spec_from_file_location("acoustic_structure_verifier", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class OccurrenceScopeTests(unittest.TestCase):
    def test_forbidden_scan_includes_test_code_but_not_non_code(self) -> None:
        payload = {
            "occurrences": [
                {
                    "file": "core/live.rs",
                    "scope_classification": "production",
                    "match_role": "reference",
                },
                {
                    "file": "tests/oracle.rs",
                    "scope_classification": "test",
                    "match_role": "import",
                },
                {
                    "file": "tests/fixture.json",
                    "scope_classification": "test",
                    "match_role": "string_literal",
                },
                {
                    "file": "core/live.rs",
                    "scope_classification": "production",
                    "match_role": "comment",
                },
            ]
        }

        files = {row["file"] for row in VERIFIER.forbidden_occurrences(payload)}

        self.assertEqual(files, {"core/live.rs", "tests/oracle.rs"})


def occurrence(
    matched_identifier: str,
    *,
    file: str = "core/live.rs",
    line: int = 1,
    column: int = 1,
    match_role: str = "reference",
    scope_classification: str = "production",
) -> dict[str, object]:
    return {
        "file": file,
        "line": line,
        "column": column,
        "matched_text": matched_identifier,
        "context": matched_identifier,
        "match_role": match_role,
        "scope_classification": scope_classification,
    }


def regex_payload(*rows: dict[str, object]) -> dict[str, object]:
    return {"matches": {"occurrences": list(rows)}}


def exact_payload(*rows: dict[str, object]) -> dict[str, object]:
    return {"occurrences": list(rows)}


class StubVerifier:
    def __init__(self, repo: Path, *regex_rows: dict[str, object]) -> None:
        self.repo = repo
        self.regex_rows = regex_rows
        self.command_inventory = [
            ["loct", "context", "--json"],
            ["loct", "occurrences", "quality_gate", "--json"],
            ["loct", "find", "--regex", "quality_gate", "--json"],
        ]

    def context(self) -> dict[str, object]:
        return {
            "receipt": {
                "authority": "fresh",
                "root": str(self.repo),
                "head_full": "test-head",
                "dirty_fingerprint": "clean",
                "binary_id": "test-loctree",
                "snapshot_fingerprint": "test-snapshot",
            },
            "project": {"branch": "test-branch"},
        }

    def occurrences(self, _symbol: str) -> dict[str, object]:
        return exact_payload()

    def substring_occurrences(self, _needle: str) -> dict[str, object]:
        return regex_payload(*self.regex_rows)


def wired_manifest() -> dict[str, object]:
    return {
        "retired_reference_symbols": ["quality_gate"],
        "stages": {
            "wired": {
                "required_absent": [],
                "required_present": [],
                "required_unwired": [],
                "required_edges": [],
                "require_zero_dangling": True,
            }
        },
    }


class SubstringResidueTests(unittest.TestCase):
    def test_r1_exact_production_identifier_remains_fail_gated(self) -> None:
        row = occurrence("quality_gate")
        self.assertEqual(VERIFIER.forbidden_occurrences(exact_payload(row)), [row])
        self.assertEqual(
            VERIFIER.residue_occurrences(
                regex_payload(row), exact_payload(row), "quality_gate"
            ),
            [],
        )

    def test_r2_exact_comment_and_string_stay_out_of_fail_gate(self) -> None:
        rows = (
            occurrence("quality_gate", match_role="comment"),
            occurrence("quality_gate", line=2, match_role="string_literal"),
        )
        self.assertEqual(VERIFIER.forbidden_occurrences(exact_payload(*rows)), [])
        residue = VERIFIER.residue_occurrences(
            regex_payload(*rows), exact_payload(*rows), "quality_gate"
        )
        self.assertEqual({row["class"] for row in residue}, {"comment_or_string"})

    def test_r3_exact_test_import_remains_fail_gated(self) -> None:
        row = occurrence(
            "quality_gate",
            file="tests/oracle.rs",
            match_role="import",
            scope_classification="test",
        )
        self.assertEqual(VERIFIER.forbidden_occurrences(exact_payload(row)), [row])
        self.assertEqual(
            VERIFIER.residue_occurrences(
                regex_payload(row), exact_payload(row), "quality_gate"
            ),
            [],
        )

    def test_r4_drop_field_is_report_only_consumer(self) -> None:
        row = occurrence("quality_gate_dropped")
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(), "quality_gate"
        )
        self.assertEqual(residue[0]["class"], "executable_consumer")
        self.assertFalse(residue[0]["fail_gate"])

    def test_r5_engine_drop_predicate_is_report_only_authority(self) -> None:
        row = occurrence("should_drop_for_quality_gate", match_role="definition")
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(), "quality_gate"
        )
        self.assertEqual(residue[0]["class"], "executable_authority")
        self.assertFalse(residue[0]["fail_gate"])

    def test_r6_cfg_any_progressive_seal_is_test_only_mutant(self) -> None:
        row = occurrence("emit_ready_progressive_seals")
        residue_class, _ = VERIFIER.classify_substring_residue(
            row, "progressive_seal", disabled_cfg_any=True
        )
        self.assertEqual(residue_class, "test_only_mutant")

    def test_r7_prefixed_stream_postprocess_test_is_test_only_mutant(self) -> None:
        row = occurrence("test_stream_postprocess_corpus_pairs", match_role="definition")
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(), "stream_postprocess"
        )
        self.assertEqual(residue[0]["class"], "test_only_mutant")

    def test_r8_pascal_comment_is_non_code(self) -> None:
        row = occurrence("StreamPostProcessor", match_role="comment")
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(), "stream_postprocess"
        )
        self.assertEqual(residue[0]["class"], "comment_or_string")
        self.assertEqual(residue[0]["relation"], "pascal_twin")

    def test_r9_fixture_literal_is_visible_but_report_only(self) -> None:
        row = occurrence(
            "quality_gate",
            file="tests/fixtures/stage.json",
            match_role="string_literal",
            scope_classification="test",
        )
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(row), "quality_gate"
        )
        self.assertEqual(residue[0]["class"], "fixture")
        self.assertFalse(residue[0]["fail_gate"])

    def test_r10_verifier_literal_is_visible_but_report_only(self) -> None:
        row = occurrence(
            "quality_gate",
            file="scripts/verify-acoustic-throne-structure.py",
            match_role="string_literal",
        )
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(row), "quality_gate"
        )
        self.assertEqual(residue[0]["class"], "verifier_self_literal")

    def test_r11_residue_does_not_change_conformance(self) -> None:
        row = occurrence("should_drop_for_quality_gate", match_role="definition")
        with tempfile.TemporaryDirectory() as raw_repo:
            receipt, conformant = VERIFIER.verify_stage(
                StubVerifier(Path(raw_repo), row), wired_manifest(), "wired", None, None
            )
        self.assertTrue(receipt["residue_by_substring"]["quality_gate"])
        self.assertEqual(receipt["failures"], [])
        self.assertTrue(conformant)
        self.assertTrue(receipt["conformant"])

    def test_r12_empty_residue_is_shape_valid(self) -> None:
        with tempfile.TemporaryDirectory() as raw_repo:
            receipt, _ = VERIFIER.verify_stage(
                StubVerifier(Path(raw_repo)), wired_manifest(), "wired", None, None
            )
        self.assertEqual(receipt["residue_by_substring"]["quality_gate"], [])
        self.assertEqual(receipt["residue_by_substring"]["summary"]["total_count"], 0)
        VERIFIER.validate_receipt_shape(receipt)

    def test_r13_unknown_infix_requires_review(self) -> None:
        row = occurrence("mystery_quality_gate_adapter")
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(), "quality_gate"
        )
        self.assertEqual(residue[0]["class"], "unclassified_requires_review")
        self.assertTrue(residue[0]["review_required"])
        self.assertFalse(residue[0]["fail_gate"])


class RustModuleResolutionTests(unittest.TestCase):
    def test_reports_missing_module_and_accepts_standard_module_file(self) -> None:
        with tempfile.TemporaryDirectory() as raw_repo:
            repo = Path(raw_repo)
            core = repo / "core"
            core.mkdir()
            (core / "mod.rs").write_text("mod present;\nmod missing;\n")
            (core / "present.rs").write_text("pub fn marker() {}\n")

            unresolved = VERIFIER.unresolved_module_declarations(repo)

        self.assertEqual(len(unresolved), 1)
        self.assertEqual(unresolved[0]["module"], "missing")
        self.assertEqual(unresolved[0]["file"], "core/mod.rs")

    def test_honours_explicit_path_attribute(self) -> None:
        with tempfile.TemporaryDirectory() as raw_repo:
            repo = Path(raw_repo)
            app = repo / "app"
            shared = repo / "shared"
            app.mkdir()
            shared.mkdir()
            (app / "lib.rs").write_text(
                '#[path = "../shared/oracle.rs"]\n#[cfg(test)]\nmod oracle;\n'
            )
            (shared / "oracle.rs").write_text("pub fn marker() {}\n")

            unresolved = VERIFIER.unresolved_module_declarations(repo)

        self.assertEqual(unresolved, [])


if __name__ == "__main__":
    unittest.main()
