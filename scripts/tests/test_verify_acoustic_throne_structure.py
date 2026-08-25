from __future__ import annotations

import copy
import importlib.util
import json
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
    return {
        "matches": {
            "occurrences": list(rows),
            "offset": 0,
            "total": len(rows),
            "truncated": False,
            "universe": {"scan_complete": True},
        },
        "regex_trust": {
            "pattern_compiled": True,
            "file_scope_resolved": True,
            "absence_trustworthy_for_scanned": True,
        },
    }


def exact_payload(*rows: dict[str, object]) -> dict[str, object]:
    return {"occurrences": list(rows)}


class StubVerifier:
    def __init__(
        self,
        repo: Path,
        *regex_rows: dict[str, object],
        regex_response: dict[str, object] | None = None,
    ) -> None:
        self.repo = repo
        self.regex_rows = regex_rows
        self.regex_response = regex_response
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
        if self.regex_response is not None:
            return self.regex_response
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

    def test_r14_current_complete_payload_is_green(self) -> None:
        row = occurrence("quality_gate_dropped")
        residue = VERIFIER.residue_occurrences(
            regex_payload(row), exact_payload(), "quality_gate"
        )
        self.assertEqual(len(residue), 1)
        self.assertEqual(residue[0]["matched_identifier"], "quality_gate_dropped")

    def test_r15_truncated_or_unproven_truncation_is_red(self) -> None:
        for value in (True, None):
            with self.subTest(value=value):
                payload = regex_payload()
                if value is None:
                    del payload["matches"]["truncated"]  # type: ignore[index]
                else:
                    payload["matches"]["truncated"] = value  # type: ignore[index]
                with self.assertRaises(RuntimeError):
                    VERIFIER.residue_occurrences(
                        payload, exact_payload(), "quality_gate"
                    )

        with tempfile.TemporaryDirectory() as raw_repo:
            truncated = regex_payload()
            truncated["matches"]["truncated"] = True  # type: ignore[index]
            with self.assertRaises(RuntimeError):
                VERIFIER.verify_stage(
                    StubVerifier(Path(raw_repo), regex_response=truncated),
                    wired_manifest(),
                    "wired",
                    None,
                    None,
                )

    def test_r16_total_must_match_emitted_occurrences_and_be_an_integer(self) -> None:
        row = occurrence("quality_gate_dropped")
        for total in (0, True, -1, None):
            with self.subTest(total=total):
                payload = regex_payload(row)
                if total is None:
                    del payload["matches"]["total"]  # type: ignore[index]
                else:
                    payload["matches"]["total"] = total  # type: ignore[index]
                with self.assertRaises(RuntimeError):
                    VERIFIER.residue_occurrences(
                        payload, exact_payload(), "quality_gate"
                    )

    def test_r17_offset_must_be_exact_integer_zero(self) -> None:
        for offset in (1, True, None):
            with self.subTest(offset=offset):
                payload = regex_payload()
                if offset is None:
                    del payload["matches"]["offset"]  # type: ignore[index]
                else:
                    payload["matches"]["offset"] = offset  # type: ignore[index]
                with self.assertRaises(RuntimeError):
                    VERIFIER.residue_occurrences(
                        payload, exact_payload(), "quality_gate"
                    )

    def test_r18_scan_complete_must_be_present_and_true(self) -> None:
        for value in (False, None):
            with self.subTest(value=value):
                payload = regex_payload()
                universe = payload["matches"]["universe"]  # type: ignore[index]
                if value is None:
                    del universe["scan_complete"]  # type: ignore[index]
                else:
                    universe["scan_complete"] = value  # type: ignore[index]
                with self.assertRaises(RuntimeError):
                    VERIFIER.residue_occurrences(
                        payload, exact_payload(), "quality_gate"
                    )

    def test_r19_regex_trust_must_be_present_and_all_true(self) -> None:
        missing_trust = regex_payload()
        del missing_trust["regex_trust"]
        with self.assertRaises(RuntimeError):
            VERIFIER.residue_occurrences(missing_trust, exact_payload(), "quality_gate")

        for key in VERIFIER.REQUIRED_REGEX_TRUST_KEYS:
            for value in (False, None):
                with self.subTest(key=key, value=value):
                    payload = regex_payload()
                    trust = payload["regex_trust"]
                    if value is None:
                        del trust[key]  # type: ignore[index]
                    else:
                        trust[key] = value  # type: ignore[index]
                    with self.assertRaises(RuntimeError):
                        VERIFIER.residue_occurrences(
                            payload, exact_payload(), "quality_gate"
                        )

    def test_r20_matches_and_occurrences_must_be_well_shaped(self) -> None:
        malformed = regex_payload()
        del malformed["matches"]
        missing_occurrences = regex_payload()
        del missing_occurrences["matches"]["occurrences"]  # type: ignore[index]
        non_list_occurrences = regex_payload()
        non_list_occurrences["matches"]["occurrences"] = {}  # type: ignore[index]
        non_object_occurrence = regex_payload()
        non_object_occurrence["matches"]["occurrences"] = ["row"]  # type: ignore[index]
        for payload in (
            malformed,
            {"matches": []},
            missing_occurrences,
            non_list_occurrences,
            non_object_occurrence,
        ):
            with self.subTest(payload=payload):
                with self.assertRaises(RuntimeError):
                    VERIFIER.residue_occurrences(
                        payload, exact_payload(), "quality_gate"
                    )

    def test_r21_complete_zero_occurrence_payload_is_green(self) -> None:
        self.assertEqual(
            VERIFIER.require_complete_regex_evidence(regex_payload(), "quality_gate"),
            [],
        )
        self.assertEqual(
            VERIFIER.residue_occurrences(
                regex_payload(), exact_payload(), "quality_gate"
            ),
            [],
        )

    def test_r22_receipt_summary_and_schema_are_consistent(self) -> None:
        with tempfile.TemporaryDirectory() as raw_repo:
            receipt, conformant = VERIFIER.verify_stage(
                StubVerifier(Path(raw_repo)), wired_manifest(), "wired", None, None
            )
        summary = receipt["residue_by_substring"]["summary"]
        self.assertTrue(conformant)
        self.assertIs(summary["evidence_complete"], True)
        self.assertEqual(summary["query_count"], 1)
        self.assertEqual(summary["complete_query_count"], 1)
        self.assertEqual(summary["truncated_query_count"], 0)
        self.assertEqual(
            set(receipt["residue_by_substring"]) - {"summary"},
            set(receipt["forbidden_symbols"]),
        )
        VERIFIER.validate_receipt_shape(receipt)

        schema_path = (
            SCRIPT.parents[1] / "tests/fixtures/acoustic_structure_receipt.schema.json"
        )
        summary_schema = json.loads(schema_path.read_text())["$defs"]["residueSummary"]
        required = {
            "evidence_complete",
            "query_count",
            "complete_query_count",
            "truncated_query_count",
        }
        self.assertTrue(required.issubset(summary_schema["required"]))
        self.assertEqual(
            summary_schema["properties"]["evidence_complete"], {"const": True}
        )
        self.assertEqual(
            summary_schema["properties"]["truncated_query_count"], {"const": 0}
        )

        inconsistent = copy.deepcopy(receipt)
        inconsistent["residue_by_substring"]["summary"]["query_count"] = 2
        with self.assertRaises(RuntimeError):
            VERIFIER.validate_receipt_shape(inconsistent)


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
