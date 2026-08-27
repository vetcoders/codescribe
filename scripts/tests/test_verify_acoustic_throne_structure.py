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
    enclosing_symbol: str | None = None,
    enclosing_file: str | None = None,
) -> dict[str, object]:
    row: dict[str, object] = {
        "file": file,
        "line": line,
        "column": column,
        "matched_text": matched_identifier,
        "context": matched_identifier,
        "match_role": match_role,
        "scope_classification": scope_classification,
    }
    if enclosing_symbol is not None:
        row["enclosing_symbol"] = {
            "name": enclosing_symbol,
            "file": enclosing_file or file,
            "kind": "function",
        }
    return row


def regex_payload(
    *rows: dict[str, object],
    query: str | None = None,
    indexed_files: int = 1,
) -> dict[str, object]:
    if query is None:
        matched = " ".join(str(row.get("matched_text", "")) for row in rows)
        needle = (
            "stream_postprocess"
            if "stream_postprocess" in matched or "StreamPostProcessor" in matched
            else "quality_gate"
        )
        query = VERIFIER.substring_identifier_pattern(needle)
    return {
        "mode": "regex",
        "query": query,
        "matches": {
            "query": query,
            "match_mode": "regex",
            "source": "regex",
            "occurrences": list(rows),
            "offset": 0,
            "total": len(rows),
            "emitted": len(rows),
            "truncated": False,
            "universe": {
                "scan_complete": True,
                "indexed_files": indexed_files,
                "scanned_files": indexed_files,
            },
            "scope": {
                "files_in_universe": indexed_files,
                "files_scanned": indexed_files,
            },
        },
        "regex_trust": {
            "pattern_compiled": True,
            "file_scope_resolved": True,
            "absence_trustworthy_for_scanned": True,
        },
    }


def exact_payload(*rows: dict[str, object]) -> dict[str, object]:
    return {"occurrences": list(rows)}


def literal_payload(
    literal: str, *rows: dict[str, object], indexed_files: int = 1
) -> dict[str, object]:
    return {
        "mode": "literal",
        "query": literal,
        "matches": {
            "query": literal,
            "source": "literal",
            "occurrences": list(rows),
            "offset": 0,
            "total": len(rows),
            "emitted": len(rows),
            "truncated": False,
            "universe": {
                "scan_complete": True,
                "indexed_files": indexed_files,
                "scanned_files": indexed_files,
            },
        },
        "literal_trust": {
            "absence_trustworthy_for_scanned": True,
            "file_scope_resolved": True,
            "matched_as_exact_string": True,
            "multi_literal": False,
        },
    }


def body_payload(
    symbol: str,
    file: str,
    source: str,
    *,
    start_line: int = 1,
    language: str = "rs",
) -> dict[str, object]:
    total_lines = source.count("\n") + 1
    return {
        "symbol": symbol,
        "bodies": [
            {
                "symbol": symbol,
                "file": file,
                "start_line": start_line,
                "end_line": start_line + total_lines - 1,
                "language": language,
                "source": source,
                "truncated": False,
                "total_lines": total_lines,
                "line_cap": 1000,
                "extent": "brace",
            }
        ],
    }


class StubVerifier:
    def __init__(
        self,
        repo: Path,
        *regex_rows: dict[str, object],
        regex_response: dict[str, object] | None = None,
        literal_responses: dict[str, dict[str, object]] | None = None,
        bodies: dict[tuple[str, str], dict[str, object]] | None = None,
        occurrence_responses: dict[str, dict[str, object]] | None = None,
    ) -> None:
        self.repo = repo
        self.regex_rows = regex_rows
        self.regex_response = regex_response
        self.literal_responses = literal_responses or {}
        self.bodies = bodies or {}
        self.occurrence_responses = occurrence_responses or {}
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
        if _symbol in self.occurrence_responses:
            return self.occurrence_responses[_symbol]
        body_rows = [
            row
            for (symbol, _file), payload in self.bodies.items()
            if symbol == _symbol
            for row in payload.get("bodies", [])  # type: ignore[union-attr]
        ]
        if body_rows:
            return exact_payload(
                *(
                    occurrence(
                        _symbol,
                        file=str(row["file"]),
                        line=int(row["start_line"]),
                        match_role="definition",
                    )
                    for row in body_rows
                )
            )
        return exact_payload()

    def substring_occurrences(self, _needle: str) -> dict[str, object]:
        if self.regex_response is not None:
            return self.regex_response
        return regex_payload(*self.regex_rows)

    def run_loct(self, *args: str) -> object:
        self.command_inventory.append(["loct", *args, "--json"])
        if args[:2] == ("find", "--regex"):
            query = args[2]
            return VERIFIER.LoctResult(["loct", *args, "--json"], regex_payload(query=query))
        if args[:2] == ("find", "--literal"):
            literal = args[2]
            payload = self.literal_responses.get(literal, literal_payload(literal))
            return VERIFIER.LoctResult(["loct", *args, "--json"], payload)
        raise AssertionError(f"unexpected stub Loctree command: {args}")

    def run_loct_query(self, expression: str) -> object:
        self.command_inventory.append(["loct", expression])
        return VERIFIER.LoctResult(
            ["loct", expression], [{"path": "core/live.rs", "imports": []}]
        )

    def body(self, symbol: str, file: str) -> dict[str, object]:
        return self.bodies.get((symbol, file), {"symbol": symbol, "bodies": []})

    def literal_occurrences(self, literal: str) -> dict[str, object]:
        return self.literal_responses.get(literal, literal_payload(literal))


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

    def test_r11_executable_residue_changes_conformance(self) -> None:
        row = occurrence("should_drop_for_quality_gate", match_role="definition")
        with tempfile.TemporaryDirectory() as raw_repo:
            receipt, conformant = VERIFIER.verify_stage(
                StubVerifier(Path(raw_repo), row), wired_manifest(), "wired", None, None
            )
        self.assertTrue(receipt["residue_by_substring"]["quality_gate"])
        self.assertEqual(
            receipt["failures"], ["forbidden residue retains 1 executable rows"]
        )
        self.assertFalse(conformant)
        self.assertFalse(receipt["conformant"])

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


class CorridorProofTests(unittest.TestCase):
    def contract(self) -> list[dict[str, object]]:
        return [
            {
                "name": "projection",
                "hops": [
                    {
                        "symbol": "publish_revision",
                        "file": "app/presentation/transcript_bus.rs",
                        "required_code": [
                            "ledger.serial_of(&entry.occurrence)",
                            "Self::project_serial(serial)",
                        ],
                    }
                ],
                "required_invocations": [
                    {
                        "callee": "project_serial",
                        "callee_file": "app/presentation/transcript_bus.rs",
                        "caller": "publish_revision",
                        "caller_file": "app/presentation/transcript_bus.rs",
                    }
                ],
            }
        ]

    def verifier_for(self, source: str) -> StubVerifier:
        file = "app/presentation/transcript_bus.rs"
        return StubVerifier(
            Path("/repo"),
            bodies={
                ("publish_revision", file): body_payload(
                    "publish_revision", file, source, start_line=20
                )
            },
            occurrence_responses={
                "project_serial": exact_payload(
                    occurrence(
                        "project_serial",
                        file=file,
                        line=23,
                        enclosing_symbol="publish_revision",
                    ),
                    occurrence(
                        "project_serial",
                        file=file,
                        line=10,
                        match_role="local_binding",
                        enclosing_symbol="project_serial",
                    ),
                )
            },
        )

    def ordering_contract(self) -> list[dict[str, object]]:
        contract = copy.deepcopy(self.contract())
        corridor = contract[0]
        corridor["required_invocations"].append(  # type: ignore[union-attr]
            {
                "callee": "publish_event",
                "callee_file": "app/presentation/transcript_bus.rs",
                "caller": "publish_revision",
                "caller_file": "app/presentation/transcript_bus.rs",
            }
        )
        corridor["ordering"] = [
            {
                "caller": "publish_revision",
                "caller_file": "app/presentation/transcript_bus.rs",
                "before": {"corridor": "projection", "callee": "project_serial"},
                "after": {"corridor": "projection", "callee": "publish_event"},
                "barrier": {
                    "required_code": "let _guard = self.recorder.lock().await",
                    "selection": "last_before_after",
                },
            }
        ]
        return contract

    def verifier_for_ordering(
        self,
        *,
        before_line: int = 22,
        after_line: int = 24,
        include_before: bool = True,
        before_lines: tuple[int, ...] | None = None,
    ) -> StubVerifier:
        file = "app/presentation/transcript_bus.rs"
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "  let _guard = self.recorder.lock().await;\n"
            "  Self::publish_event(event);\n"
            "}\n"
        )
        before_rows = []
        if include_before:
            for line in before_lines or (before_line,):
                before_rows.append(
                    occurrence(
                        "project_serial",
                        file=file,
                        line=line,
                        enclosing_symbol="publish_revision",
                    )
                )
        verifier.occurrence_responses["project_serial"] = exact_payload(
            *before_rows,
            occurrence(
                "project_serial",
                file=file,
                line=10,
                match_role="local_binding",
                enclosing_symbol="project_serial",
            ),
        )
        verifier.occurrence_responses["publish_event"] = exact_payload(
            occurrence(
                "publish_event",
                file=file,
                line=after_line,
                enclosing_symbol="publish_revision",
            ),
            occurrence(
                "publish_event",
                file=file,
                line=11,
                match_role="local_binding",
                enclosing_symbol="publish_event",
            ),
        )
        return verifier

    def verify_admission_ordering_mutant(
        self,
        *,
        caller: str,
        before_callee: str,
        barrier_required_code: str,
        source: str,
        before_lines: tuple[int, ...],
        after_line: int,
    ) -> tuple[dict[str, object], list[str]]:
        caller_file = "app/controller/mod.rs"
        before_callee_file = (
            caller_file
            if before_callee == "admission_readiness"
            else "app/controller/admission.rs"
        )
        after_callee = "bind_session_authority"
        after_callee_file = "core/audio/streaming_recorder.rs"
        contract = [
            {
                "name": "settings_to_capture_admission",
                "hops": [
                    {
                        "symbol": caller,
                        "file": caller_file,
                        "required_code": [barrier_required_code],
                    }
                ],
                "required_invocations": [
                    {
                        "callee": before_callee,
                        "callee_file": before_callee_file,
                        "caller": caller,
                        "caller_file": caller_file,
                    },
                    {
                        "callee": after_callee,
                        "callee_file": after_callee_file,
                        "caller": caller,
                        "caller_file": caller_file,
                    },
                ],
                "ordering": [
                    {
                        "caller": caller,
                        "caller_file": caller_file,
                        "before": {
                            "corridor": "settings_to_capture_admission",
                            "callee": before_callee,
                        },
                        "after": {
                            "corridor": "settings_to_capture_admission",
                            "callee": after_callee,
                        },
                        "barrier": {
                            "required_code": barrier_required_code,
                            "selection": "last_before_after",
                        },
                    }
                ],
            }
        ]
        before_rows = [
            occurrence(
                before_callee,
                file=caller_file,
                line=line,
                enclosing_symbol=caller,
            )
            for line in before_lines
        ]
        verifier = StubVerifier(
            Path("/repo"),
            bodies={
                (caller, caller_file): body_payload(
                    caller, caller_file, source, start_line=20
                )
            },
            occurrence_responses={
                before_callee: exact_payload(
                    *before_rows,
                    occurrence(
                        before_callee,
                        file=before_callee_file,
                        line=10,
                        match_role="definition",
                    ),
                ),
                after_callee: exact_payload(
                    occurrence(
                        after_callee,
                        file=caller_file,
                        line=after_line,
                        enclosing_symbol=caller,
                    ),
                    occurrence(
                        after_callee,
                        file=after_callee_file,
                        line=10,
                        match_role="definition",
                    ),
                ),
            },
        )
        return VERIFIER.verify_code_corridors(verifier, contract)

    def assert_admission_mutant_is_caller_red(
        self,
        observed: dict[str, object],
        failures: list[str],
        caller: str,
    ) -> None:
        corridor = observed["settings_to_capture_admission"]
        assert isinstance(corridor, dict)
        ordering = corridor["ordering"]
        assert isinstance(ordering, list)
        self.assertEqual(ordering[0]["verdict"], "RED")
        self.assertTrue(
            any(
                f"corridor settings_to_capture_admission ordering {caller}" in failure
                and "required-code barrier" in failure
                for failure in failures
            ),
            failures,
        )

    def test_c1_live_hop_is_proven(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertTrue(
            observed["projection"]["hops"][0]["production_definition"]
        )
        self.assertEqual(observed["projection"]["hops"][0]["missing_code"], [])

    def test_c2_dropping_any_hop_code_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(
            observed["projection"]["hops"][0]["missing_code"],
            ["Self::project_serial(serial)"],
        )
        self.assertEqual(len(failures), 1)

    def test_c3_comments_and_strings_cannot_fake_a_hop(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  // ledger.serial_of(&entry.occurrence)\n"
            '  let lie = "Self::project_serial(serial)";\n'
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(
            observed["projection"]["hops"][0]["missing_code"],
            [
                "ledger.serial_of(&entry.occurrence)",
                "Self::project_serial(serial)",
            ],
        )
        self.assertEqual(len(failures), 1)

    def test_c4_test_scope_definition_cannot_fake_production(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )
        verifier.occurrences = lambda _symbol: exact_payload(  # type: ignore[method-assign]
            occurrence(
                "publish_revision",
                file="app/presentation/transcript_bus.rs",
                line=20,
                match_role="definition",
                scope_classification="test",
            )
        )

        _, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertTrue(
            any("not a production definition" in failure for failure in failures)
        )

    def test_c5_compile_disabled_production_witness_is_red(self) -> None:
        literal = "#if false"
        verifier = StubVerifier(
            Path("/repo"),
            literal_responses={
                literal: literal_payload(
                    literal,
                    occurrence(
                        literal,
                        file="macos/Codescribe/Overlay.swift",
                        match_role="unknown",
                    ),
                )
            },
        )

        hits, failures = VERIFIER.verify_forbidden_executable_literals(
            verifier, [literal]
        )

        self.assertEqual(hits[literal], ["macos/Codescribe/Overlay.swift"])
        self.assertEqual(len(failures), 1)

    def test_c6_literal_named_in_test_data_is_not_production_residue(self) -> None:
        literal = "#if false"
        verifier = StubVerifier(
            Path("/repo"),
            literal_responses={
                literal: literal_payload(
                    literal,
                    occurrence(
                        literal,
                        file="scripts/tests/test_structure.py",
                        match_role="string_literal",
                        scope_classification="test",
                    ),
                )
            },
        )

        hits, failures = VERIFIER.verify_forbidden_executable_literals(
            verifier, [literal]
        )

        self.assertEqual(hits[literal], [])
        self.assertEqual(failures, [])

    def test_c7_callsite_in_wrong_caller_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )
        verifier.occurrence_responses["project_serial"] = exact_payload(
            occurrence(
                "project_serial",
                file="app/presentation/transcript_bus.rs",
                line=23,
                enclosing_symbol="disconnected_helper",
            ),
            occurrence(
                "project_serial",
                file="app/presentation/transcript_bus.rs",
                line=10,
                match_role="local_binding",
                enclosing_symbol="project_serial",
            ),
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(observed["projection"]["invocations"][0]["observed_count"], 0)
        self.assertTrue(any("production callsite" in failure for failure in failures))

    def test_c8_unreachable_if_false_body_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  return;\n"
            "  if false {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(
            observed["projection"]["hops"][0]["dead_code_markers"], ["if false"]
        )
        self.assertTrue(any("dead-code markers" in failure for failure in failures))

    def test_c9_required_code_out_of_order_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  Self::project_serial(serial);\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertFalse(
            observed["projection"]["hops"][0]["required_code_in_order"]
        )
        self.assertTrue(any("required order" in failure for failure in failures))

    def test_c10_test_scope_callsite_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )
        verifier.occurrence_responses["project_serial"] = exact_payload(
            occurrence(
                "project_serial",
                file="app/presentation/transcript_bus.rs",
                line=23,
                scope_classification="test",
                enclosing_symbol="publish_revision",
            ),
            occurrence(
                "project_serial",
                file="app/presentation/transcript_bus.rs",
                line=10,
                match_role="local_binding",
                enclosing_symbol="project_serial",
            ),
        )

        _, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertTrue(any("production callsite" in failure for failure in failures))

    def test_c11_required_code_after_top_level_return_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  return;\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(
            all(
                row["reason"] == "after unconditional top-level return"
                for row in unreachable
            )
        )
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c12_required_code_inside_while_false_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  while false {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(
            all("statically false while" in row["reason"] for row in unreachable)
        )
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c13_required_code_inside_false_integer_comparison_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if 1 == 2 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(
            all("statically false if" in row["reason"] for row in unreachable)
        )
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c14_nested_conditional_return_preserves_reachable_hop(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision(stop: bool) {\n"
            "  if stop { return; }\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_c15_return_identifier_is_not_a_terminator(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let return_value = true;\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_c16_true_integer_condition_preserves_reachable_hop(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if 1 == 1 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_c17_return_after_required_hop_does_not_retroactively_kill_it(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "  return;\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_receipt_schema_requires_bounded_reachability_evidence(self) -> None:
        schema_path = (
            SCRIPT.parents[1] / "tests/fixtures/acoustic_structure_receipt.schema.json"
        )
        schema = json.loads(schema_path.read_text())
        hop_schema = schema["properties"]["corridor_paths"][
            "additionalProperties"
        ]["properties"]["hops"]["items"]

        self.assertIn("unreachable_required_code", hop_schema["required"])
        unreachable_schema = hop_schema["properties"]["unreachable_required_code"]
        self.assertEqual(unreachable_schema["type"], "array")
        self.assertEqual(
            unreachable_schema["items"]["required"], ["required_code", "reason"]
        )

    def test_c18_typed_integer_false_condition_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if 1u8 == 2u8 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c19_false_arithmetic_condition_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if 1 + 0 == 2 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c20_return_inside_braceless_closure_does_not_kill_function(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  let callback = || return;\n"
            "  let serial = ledger.serial_of(&entry.occurrence);\n"
            "  Self::project_serial(serial);\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_c21_true_negative_remainder_condition_preserves_reachable_hop(
        self,
    ) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if -5i32 % 3i32 == -2i32 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_c22_false_negative_remainder_condition_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if -5i32 % 3i32 == 1i32 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c23_true_negative_division_condition_preserves_reachable_hop(
        self,
    ) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if -5i32 / 3i32 == -1i32 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["hops"][0]["unreachable_required_code"], []
        )

    def test_c24_false_negative_division_condition_is_red(self) -> None:
        verifier = self.verifier_for(
            "fn publish_revision() {\n"
            "  if -5i32 / 3i32 == -2i32 {\n"
            "    let serial = ledger.serial_of(&entry.occurrence);\n"
            "    Self::project_serial(serial);\n"
            "  }\n"
            "}\n"
        )

        observed, failures = VERIFIER.verify_code_corridors(verifier, self.contract())

        unreachable = observed["projection"]["hops"][0][
            "unreachable_required_code"
        ]
        self.assertEqual(len(unreachable), 2)
        self.assertTrue(any("unreachable required code" in failure for failure in failures))

    def test_c25_integer_division_and_remainder_follow_rust_sign_rules(self) -> None:
        cases = {
            "5 % 3": 2,
            "5 % -3": 2,
            "-5 % 3": -2,
            "-5 % -3": -2,
            "5 / 3": 1,
            "5 / -3": -1,
            "-5 / 3": -1,
            "-5 / -3": 1,
        }
        for expression, expected in cases.items():
            with self.subTest(expression=expression):
                self.assertEqual(
                    VERIFIER.constant_expression_value(expression), expected
                )

        self.assertIsNone(VERIFIER.constant_expression_value("1 / 0"))
        self.assertIsNone(VERIFIER.constant_expression_value("1 % 0"))

    def test_c26_same_caller_ordering_is_green(self) -> None:
        observed, failures = VERIFIER.verify_code_corridors(
            self.verifier_for_ordering(), self.ordering_contract()
        )

        self.assertEqual(failures, [])
        self.assertEqual(
            observed["projection"]["ordering"],
            [
                {
                    "caller": "publish_revision",
                    "caller_file": "app/presentation/transcript_bus.rs",
                    "before": {
                        "corridor": "projection",
                        "callee": "project_serial",
                    },
                    "after": {
                        "corridor": "projection",
                        "callee": "publish_event",
                    },
                    "barrier": {
                        "required_code": "let _guard = self.recorder.lock().await",
                        "selection": "last_before_after",
                    },
                    "barrier_observed_lines": [23],
                    "before_observed_lines": [22],
                    "after_observed_lines": [24],
                    "verdict": "GREEN",
                }
            ],
        )

    def test_c27_before_relocated_after_is_red_and_names_corridor_and_caller(
        self,
    ) -> None:
        observed, failures = VERIFIER.verify_code_corridors(
            self.verifier_for_ordering(before_line=25, after_line=24),
            self.ordering_contract(),
        )

        self.assertEqual(observed["projection"]["ordering"][0]["verdict"], "RED")
        self.assertEqual(len(failures), 1)
        self.assertIn("corridor projection ordering publish_revision", failures[0])
        self.assertIn("observed before lines [25], after lines [24]", failures[0])

    def test_c28_before_relocated_past_barrier_is_red(self) -> None:
        observed, failures = VERIFIER.verify_code_corridors(
            self.verifier_for_ordering(before_line=24, after_line=25),
            self.ordering_contract(),
        )

        ordering = observed["projection"]["ordering"][0]
        self.assertEqual(ordering["verdict"], "RED")
        self.assertEqual(ordering["barrier_observed_lines"], [23])
        self.assertEqual(len(failures), 1)
        self.assertIn("before required-code barrier", failures[0])

    def test_c29_missing_before_invocation_is_ordering_red(self) -> None:
        observed, failures = VERIFIER.verify_code_corridors(
            self.verifier_for_ordering(include_before=False),
            self.ordering_contract(),
        )

        ordering = observed["projection"]["ordering"][0]
        self.assertEqual(ordering["before_observed_lines"], [])
        self.assertEqual(ordering["verdict"], "RED")
        self.assertTrue(
            any(
                "corridor projection ordering publish_revision has no observed "
                "production callsite(s)" in failure
                for failure in failures
            )
        )

    def test_c30_malformed_ordering_entry_is_schema_red(self) -> None:
        contract = self.ordering_contract()
        contract[0]["ordering"] = [  # type: ignore[index]
            {
                "caller": "publish_revision",
                "caller_file": "app/presentation/transcript_bus.rs",
                "before": {"callee": "project_serial"},
                "after": {"corridor": "projection", "callee": "publish_event"},
            }
        ]

        with self.assertRaisesRegex(RuntimeError, "malformed ordering entry"):
            VERIFIER.verify_code_corridors(
                self.verifier_for_ordering(), contract
            )

    def test_c31_fn1_discarded_toggle_decoy_cannot_hide_late_gate(self) -> None:
        caller = "start_toggle_recording"
        observed, failures = self.verify_admission_ordering_mutant(
            caller=caller,
            before_callee="admission_readiness",
            barrier_required_code="let mut recorder_guard = self.recorder.lock().await",
            source=(
                "async fn start_toggle_recording() {\n"
                "  let _ = self.admission_readiness().await;\n"
                "  let mut recorder_guard = self.recorder.lock().await;\n"
                "  match self.admission_readiness().await {\n"
                "    Err(_) => return,\n"
                "    Ok(_) => {}\n"
                "  }\n"
                "  recorder.bind_session_authority();\n"
                "}\n"
            ),
            before_lines=(21, 23),
            after_line=27,
        )

        self.assert_admission_mutant_is_caller_red(observed, failures, caller)
        self.assertIn("observed before lines [21, 23], barrier lines [22]", failures[-1])

    def test_c32_fn2_serial_lock_decoy_cannot_move_recorder_barrier(self) -> None:
        caller = "start_toggle_recording"
        observed, failures = self.verify_admission_ordering_mutant(
            caller=caller,
            before_callee="admission_readiness",
            barrier_required_code="let mut recorder_guard = self.recorder.lock().await",
            source=(
                "async fn start_toggle_recording() {\n"
                "  let mut recorder_guard = self.recorder.lock().await;\n"
                "  match self.admission_readiness().await {\n"
                "    Err(_) => return,\n"
                "    Ok(_) => {}\n"
                "  }\n"
                "  let _decoy = self.serial_lock.lock().await;\n"
                "  recorder.bind_session_authority();\n"
                "}\n"
            ),
            before_lines=(22,),
            after_line=27,
        )

        self.assert_admission_mutant_is_caller_red(observed, failures, caller)
        self.assertIn("observed before lines [22], barrier lines [21]", failures[-1])

    def test_c33_fn5_dead_branch_decoy_cannot_hide_late_gate(self) -> None:
        caller = "start_toggle_recording"
        observed, failures = self.verify_admission_ordering_mutant(
            caller=caller,
            before_callee="admission_readiness",
            barrier_required_code="let mut recorder_guard = self.recorder.lock().await",
            source=(
                "async fn start_toggle_recording() {\n"
                "  if 1 == 2 {\n"
                "    let _ = self.admission_readiness().await;\n"
                "  }\n"
                "  let mut recorder_guard = self.recorder.lock().await;\n"
                "  match self.admission_readiness().await {\n"
                "    Err(_) => return,\n"
                "    Ok(_) => {}\n"
                "  }\n"
                "  recorder.bind_session_authority();\n"
                "}\n"
            ),
            before_lines=(22, 25),
            after_line=29,
        )

        self.assert_admission_mutant_is_caller_red(observed, failures, caller)
        self.assertIn("observed before lines [22, 25], barrier lines [24]", failures[-1])

    def test_c34_fn6_discarded_hold_decoy_cannot_hide_late_gate(self) -> None:
        caller = "schedule_hold_start"
        observed, failures = self.verify_admission_ordering_mutant(
            caller=caller,
            before_callee="evaluate_live_admission_arc",
            barrier_required_code="let mut rec_guard = recorder.lock().await",
            source=(
                "async fn schedule_hold_start() {\n"
                "  let _ = admission::evaluate_live_admission_arc(&settings);\n"
                "  let mut rec_guard = recorder.lock().await;\n"
                "  let verdict = admission::evaluate_live_admission_arc(&settings);\n"
                "  if verdict.is_err() {\n"
                "    return;\n"
                "  }\n"
                "  rec.bind_session_authority();\n"
                "}\n"
            ),
            before_lines=(21, 23),
            after_line=27,
        )

        self.assert_admission_mutant_is_caller_red(observed, failures, caller)
        self.assertIn("observed before lines [21, 23], barrier lines [22]", failures[-1])

    def test_c35_m8b_m9_p7_and_fn3_controls_stay_red(self) -> None:
        relocation_cases = (
            (
                "M8b",
                "start_toggle_recording",
                "admission_readiness",
                "let mut recorder_guard = self.recorder.lock().await",
                "async fn start_toggle_recording() {\n"
                "  let mut recorder_guard = self.recorder.lock().await;\n"
                "  match self.admission_readiness().await {\n"
                "    Err(_) => return,\n"
                "    Ok(_) => {}\n"
                "  }\n"
                "  recorder.bind_session_authority();\n"
                "}\n",
                (22,),
                26,
            ),
            (
                "M9",
                "schedule_hold_start",
                "evaluate_live_admission_arc",
                "let mut rec_guard = recorder.lock().await",
                "async fn schedule_hold_start() {\n"
                "  let mut rec_guard = recorder.lock().await;\n"
                "  let verdict = admission::evaluate_live_admission_arc(&settings);\n"
                "  if verdict.is_err() {\n"
                "    return;\n"
                "  }\n"
                "  rec.bind_session_authority();\n"
                "}\n",
                (22,),
                26,
            ),
        )
        for (
            name,
            caller,
            before_callee,
            barrier_required_code,
            source,
            before_lines,
            after_line,
        ) in relocation_cases:
            with self.subTest(name=name):
                observed, failures = self.verify_admission_ordering_mutant(
                    caller=caller,
                    before_callee=before_callee,
                    barrier_required_code=barrier_required_code,
                    source=source,
                    before_lines=before_lines,
                    after_line=after_line,
                )
                self.assert_admission_mutant_is_caller_red(
                    observed, failures, caller
                )

        absent_cases = (
            (
                "P7",
                "async fn start_toggle_recording() {\n"
                "  let mut recorder_guard = self.recorder.lock().await;\n"
                "  recorder.bind_session_authority();\n"
                "}\n",
            ),
            (
                "FN3",
                "async fn start_toggle_recording() {\n"
                "  // self.admission_readiness().await\n"
                "  let mut recorder_guard = self.recorder.lock().await;\n"
                "  recorder.bind_session_authority();\n"
                "}\n",
            ),
        )
        for name, source in absent_cases:
            with self.subTest(name=name):
                observed, failures = self.verify_admission_ordering_mutant(
                    caller="start_toggle_recording",
                    before_callee="admission_readiness",
                    barrier_required_code=(
                        "let mut recorder_guard = self.recorder.lock().await"
                    ),
                    source=source,
                    before_lines=(),
                    after_line=22 if name == "P7" else 23,
                )
                ordering = observed["settings_to_capture_admission"]["ordering"][0]
                self.assertEqual(ordering["verdict"], "RED")
                self.assertTrue(
                    any(
                        "ordering start_toggle_recording has no observed production "
                        "callsite(s)" in failure
                        for failure in failures
                    ),
                    failures,
                )

    def test_c36_mcard_second_recording_controller_stays_cardinality_red(self) -> None:
        manifest = wired_manifest()
        manifest["stages"]["wired"]["required_present"] = [
            {
                "symbol": "RecordingController",
                "owner": "app/controller/mod.rs",
                "cardinality": 1,
            }
        ]
        verifier = StubVerifier(
            Path("/repo"),
            occurrence_responses={
                "RecordingController": exact_payload(
                    occurrence(
                        "RecordingController",
                        file="app/controller/mod.rs",
                        line=100,
                        match_role="definition",
                    ),
                    occurrence(
                        "RecordingController",
                        file="app/controller/decoy.rs",
                        line=1,
                        match_role="definition",
                    ),
                )
            },
        )

        receipt, conformant = VERIFIER.verify_stage(
            verifier, manifest, "wired", None, None
        )

        self.assertFalse(conformant)
        self.assertIn(
            "part RecordingController expected 1 definition(s) in "
            "app/controller/mod.rs; observed 2 total in "
            "['app/controller/decoy.rs', 'app/controller/mod.rs']",
            receipt["failures"],
        )

    def test_c37_name_only_lock_barrier_is_schema_red(self) -> None:
        contract = self.ordering_contract()
        contract[0]["ordering"][0]["barrier"] = {  # type: ignore[index]
            "callee": "lock",
            "selection": "last_before_after",
        }

        with self.assertRaisesRegex(RuntimeError, "malformed ordering barrier"):
            VERIFIER.verify_code_corridors(
                self.verifier_for_ordering(), contract
            )


class RustModuleResolutionTests(unittest.TestCase):
    def test_reports_missing_module_and_accepts_standard_module_file(self) -> None:
        module_payload = regex_payload(
            occurrence("mod present;", file="core/mod.rs", line=1),
            occurrence("mod missing;", file="core/mod.rs", line=2),
            query=VERIFIER.RUST_MODULE_DECLARATION_PATTERN,
            indexed_files=2,
        )
        path_payload = regex_payload(
            query=VERIFIER.RUST_PATH_ATTRIBUTE_PATTERN,
            indexed_files=2,
        )
        inventory_payload = [
            {
                "path": "core/mod.rs",
                "imports": [
                    {
                        "line": 1,
                        "resolved_path": "core/present.rs",
                        "symbols": [{"name": "present"}],
                    }
                ],
            },
            {"path": "core/present.rs", "imports": []},
        ]

        unresolved = VERIFIER.unresolved_module_declarations(
            module_payload, path_payload, inventory_payload
        )

        self.assertEqual(len(unresolved), 1)
        self.assertEqual(unresolved[0]["module"], "missing")
        self.assertEqual(unresolved[0]["file"], "core/mod.rs")

    def test_honours_explicit_path_attribute(self) -> None:
        module_payload = regex_payload(
            occurrence("mod oracle;", file="app/lib.rs", line=2),
            query=VERIFIER.RUST_MODULE_DECLARATION_PATTERN,
            indexed_files=2,
        )
        path_payload = regex_payload(
            occurrence(
                '#[path = "../shared/oracle.rs"]', file="app/lib.rs", line=1
            ),
            query=VERIFIER.RUST_PATH_ATTRIBUTE_PATTERN,
            indexed_files=2,
        )
        inventory_payload = [
            {"path": "app/lib.rs", "imports": []},
            {"path": "shared/oracle.rs", "imports": []},
        ]

        unresolved = VERIFIER.unresolved_module_declarations(
            module_payload, path_payload, inventory_payload
        )

        self.assertEqual(unresolved, [])


if __name__ == "__main__":
    unittest.main()
