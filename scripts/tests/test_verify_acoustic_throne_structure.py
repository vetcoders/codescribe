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
    total_lines = len(source.splitlines())
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


class CorridorBodyRecoveryTests(unittest.TestCase):
    file = "bridge/src/recording.rs"
    symbol = "from_bus_receipt"
    signature = "fn from_bus_receipt(receipt: &ProjectedAcousticReceipt) -> Self"

    def typed_payload(self, source: str) -> dict[str, object]:
        return body_payload(self.symbol, self.file, source, start_line=194)

    def select(self, payload: dict[str, object]) -> list[dict[str, object]]:
        return VERIFIER.corridor_body_rows(
            payload, symbol=self.symbol, file=self.file,
            signature_contains=self.signature,
        )

    def test_selects_acoustic_method_among_two_real_types(self) -> None:
        payload = self.typed_payload(
            f"pub(crate) {self.signature} {{ Self {{}} }}"
        )
        other = body_payload(
            self.symbol, self.file,
            "fn from_bus_receipt(receipt: &ProjectedPresentationReceipt) -> Self { Self {} }",
            start_line=71,
        )
        payload["bodies"].extend(other["bodies"])
        self.assertEqual([row["start_line"] for row in self.select(payload)], [194])

    def test_wrong_type_cannot_borrow_selector_from_comment_string_or_nested_body(self) -> None:
        for decoy in (
            f"// {self.signature}\n",
            f'let decoy = "{self.signature}";',
            f"{self.signature} {{ Self {{}} }}",
        ):
            with self.subTest(decoy=decoy):
                payload = self.typed_payload(
                    "fn from_bus_receipt(receipt: &ProjectedPresentationReceipt) -> Self {\n"
                    f"{decoy}\nSelf {{}}\n}}"
                )
                self.assertEqual(self.select(payload), [])

    def test_duplicate_typed_bodies_still_fail_corridor_cardinality(self) -> None:
        payload = self.typed_payload(f"{self.signature} {{ Self {{}} }}")
        payload["bodies"].extend(copy.deepcopy(payload["bodies"]))
        verifier = StubVerifier(Path("/repo"), bodies={(self.symbol, self.file): payload})
        contract = [{
            "name": "typed_receipt",
            "hops": [{"symbol": self.symbol, "file": self.file,
                      "signature_contains": self.signature,
                      "required_code": [self.signature]}],
            "required_invocations": [{"caller": "from_bus_event", "caller_file": self.file,
                                      "callee": self.symbol, "callee_file": self.file}],
        }]
        _, failures = VERIFIER.verify_code_corridors(verifier, contract)
        self.assertTrue(any("expected one body" in failure for failure in failures))

    def test_incomplete_extent_cannot_be_blessed_by_flipping_truncated(self) -> None:
        for mutation in (
            {"truncated": True},
            {"extent": "window", "truncated": False},
            {"extent": None},
            {"total_lines": 41},
            {"end_line": 234},
        ):
            with self.subTest(mutation=mutation):
                payload = self.typed_payload(f"{self.signature} {{ Self {{}} }}")
                payload["bodies"][0].update(mutation)
                with self.assertRaisesRegex(RuntimeError, "incomplete"):
                    self.select(payload)

    def test_complete_generic_helper_is_accepted_but_window_is_refused(self) -> None:
        # Structural specimen only; it does not reproduce the product pipeline.
        source = "fn helper<F>(provider: F) -> bool\nwhere F: Fn() -> bool,\n{\nprovider()\n}"
        payload = body_payload("helper", "core/helper.rs", source)
        args = {"symbol": "helper", "file": "core/helper.rs", "signature_contains": "fn helper<F>"}
        self.assertEqual(len(VERIFIER.corridor_body_rows(payload, **args)), 1)
        payload["bodies"][0].update(extent="window", truncated=True)
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            VERIFIER.corridor_body_rows(payload, **args)

    def test_snapshot_refresh_is_requested_on_both_coherence_reads(self) -> None:
        from unittest.mock import patch

        verifier = VERIFIER.StructuralVerifier(Path("/repo"))
        with patch.object(verifier, "run_loct", return_value=VERIFIER.LoctResult([], {})) as run:
            verifier.context()
            verifier.context()
        self.assertEqual(run.call_count, 2)
        for call in run.call_args_list:
            self.assertIn("--fresh", call.args)


class NeutralTargetTests(unittest.TestCase):
    """Intercept every child: portability probes must never create a Cargo target."""

    def invoke(self, repo, overrides):
        from subprocess import CompletedProcess
        from unittest.mock import patch
        evidence = {
            "identity": VERIFIER.AST_IDENTITY,
            "schema": "codescribe.structural-ast-evidence.v1",
            "accepted": True, "failures": [],
            "contracts": [{"symbol": symbol, "accepted": True, "failures": [], "events": []}
                          for symbol in VERIFIER.AST_BODIES],
        }
        with patch.dict(VERIFIER.os.environ, overrides, clear=True), \
             patch.object(VERIFIER, "ast_tool_digest", return_value="a" * 64), \
             patch.object(VERIFIER.subprocess, "run", return_value=CompletedProcess(
                 [], 0, json.dumps(evidence), "")) as run:
            receipt = VERIFIER.run_ast_json(repo, list(VERIFIER.AST_COMMAND), {})
        run.assert_called_once()
        self.assertEqual(run.call_args.args[0], list(VERIFIER.AST_COMMAND))
        self.assertEqual(run.call_args.kwargs["cwd"], repo)
        self.assertEqual(receipt["invocation"]["cwd"], str(repo.resolve()))
        target = receipt["invocation"]["target_dir"]
        self.assertEqual(run.call_args.kwargs["env"],
                         {"CARGO_TARGET_DIR": target, "CARGO_BUILD_JOBS": "4"})
        return target

    def test_two_repository_defaults_without_creating_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            targets = []
            for name in ("checkout-one", "checkout-two"):
                repo = root / name
                repo.mkdir()
                targets.append(self.invoke(repo, {}))
                self.assertEqual(targets[-1], str(repo / "target"))
                self.assertFalse((repo / "target").exists())
            self.assertNotEqual(*targets)

    def test_existing_absolute_and_relative_shared_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            repo, shared = root / "repo", root / "shared target"
            repo.mkdir()
            shared.mkdir()
            sentinel = shared / "preserved"
            sentinel.write_text("original evidence")
            for value in (str(shared), "../shared target"):
                with self.subTest(value=value):
                    self.assertEqual(self.invoke(repo, {"CARGO_TARGET_DIR": value}), str(shared))
                    self.assertEqual(sentinel.read_text(), "original evidence")
            (repo / "target").mkdir()
            self.assertEqual(self.invoke(repo, {}), str(repo / "target"))
            self.assertEqual(self.invoke(repo, {"CARGO_TARGET_DIR": "target"}), str(repo / "target"))

    def test_invalid_targets_refused_before_execution_without_mutation(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            repo = root / "repo"
            repo.mkdir()
            file = root / "file"
            file.write_text("preserve")
            values = ["", " ", " target", "target ", "bad\npath", "bad\x00path",
                      "bad\x7fpath", "~/target", "$HOME/target", "C:\\target", "file:///target",
                      "/", str(Path.home()), str(repo), str(root), ".", "..",
                      str(file), str(file / "child"), str(repo / "missing"),
                      str(repo / "missing" / "child")]
            before = sorted(str(p.relative_to(root)) for p in root.rglob("*"))
            for value in values:
                with self.subTest(value=value), \
                     patch.object(VERIFIER.os, "environ", {"CARGO_TARGET_DIR": value}), \
                     patch.object(VERIFIER.subprocess, "run") as run, \
                     self.assertRaises(RuntimeError):
                    try:
                        VERIFIER.run_ast_json(repo, list(VERIFIER.AST_COMMAND), {})
                    finally:
                        run.assert_not_called()
            self.assertEqual(before, sorted(str(p.relative_to(root)) for p in root.rglob("*")))
            self.assertEqual(file.read_text(), "preserve")

    def test_symlink_redirects_including_default_are_refused_before_execution(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            repo, shared = root / "repo", root / "shared"
            repo.mkdir()
            shared.mkdir()
            (repo / "target").symlink_to(shared, target_is_directory=True)
            (root / "alias").symlink_to(shared, target_is_directory=True)
            (root / "dangling").symlink_to(root / "absent")
            (root / "loop").symlink_to(root / "loop")
            for overrides in ({}, *({"CARGO_TARGET_DIR": value} for value in (
                "target", str(root / "alias" / "child"), str(root / "dangling"),
                str(root / "loop"), str(root / "alias" / ".." / "shared")))):
                with self.subTest(overrides=overrides), \
                     patch.dict(VERIFIER.os.environ, overrides, clear=True), \
                     patch.object(VERIFIER.subprocess, "run") as run, \
                     self.assertRaises(RuntimeError):
                    try:
                        VERIFIER.run_ast_json(repo, list(VERIFIER.AST_COMMAND), {})
                    finally:
                        run.assert_not_called()
            self.assertTrue((repo / "target").is_symlink())
            self.assertFalse((shared / "child").exists())

    def test_default_non_directory_is_refused_before_execution(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            (repo / "target").write_text("preserve")
            with patch.dict(VERIFIER.os.environ, {}, clear=True), \
                 patch.object(VERIFIER.subprocess, "run") as run, \
                 self.assertRaises(RuntimeError):
                try:
                    VERIFIER.run_ast_json(repo, list(VERIFIER.AST_COMMAND), {})
                finally:
                    run.assert_not_called()
            self.assertEqual((repo / "target").read_text(), "preserve")

    def test_target_schema_requires_canonical_absolute_spelling(self):
        import re
        schema = json.loads((SCRIPT.parents[1] / VERIFIER.DEFAULT_MANIFEST).read_text())[
            "tool_contract"]["receipt_schema"]["$defs"]["neutralInvocation"]["properties"]["target_dir"]
        self.assertEqual(schema["type"], "string")
        pattern = schema["pattern"]
        for value in ("/checkout/target", "/shared build/target", "/t"):
            self.assertIsNotNone(re.search(pattern, value), value)
        for value in ("", "/", "target", "./target", " /target", "/target ",
                      "/target\n", "/a//target", "/a/../target", "/a/./target", "/a/..",
                      "/target/", "/a/\x00target", "/$HOME/target", "/~/target", "/a/C:target"):
            self.assertIsNone(re.search(pattern, value), repr(value))

    # --- build lease (CARGO_BUILD_JOBS / CARGO_INCREMENTAL) -----------------
    # A Fleet Worktree holding an exclusive shared target must be able to state
    # its own budget. These witnesses intercept the child, so they prove the
    # exact argument environment without ever letting Cargo run.

    def lease_child(self, overrides):
        """Run one fully intercepted invocation and return what the child got."""
        from subprocess import CompletedProcess
        from unittest.mock import patch
        evidence = {
            "identity": VERIFIER.AST_IDENTITY,
            "schema": "codescribe.structural-ast-evidence.v1",
            "accepted": True, "failures": [],
            "contracts": [{"symbol": symbol, "accepted": True, "failures": [], "events": []}
                          for symbol in VERIFIER.AST_BODIES],
        }
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory).resolve()
            with patch.dict(VERIFIER.os.environ, overrides, clear=True), \
                 patch.object(VERIFIER, "ast_tool_digest", return_value="a" * 64), \
                 patch.object(VERIFIER.subprocess, "run", return_value=CompletedProcess(
                     [], 0, json.dumps(evidence), "")) as run:
                receipt = VERIFIER.run_ast_json(repo, list(VERIFIER.AST_COMMAND), {})
            run.assert_called_once()
            return run.call_args.kwargs, receipt["invocation"]

    def test_supplied_lease_reaches_the_real_child_environment(self):
        # (jobs, incremental) as supplied -> (effective jobs, forwarded incremental)
        for supplied_jobs, supplied_incremental, jobs, incremental in (
            (None, None, "4", None),     # absent: shipped default, Cargo's own policy
            ("2", "0", "2", "0"),        # the lease this cut actually runs under
            ("1", "1", "1", "1"),
            ("16", None, "16", None),
            (None, "0", "4", "0"),
            (str(VERIFIER.AST_MAX_BUILD_JOBS), "1", str(VERIFIER.AST_MAX_BUILD_JOBS), "1"),
        ):
            overrides = {}
            if supplied_jobs is not None:
                overrides["CARGO_BUILD_JOBS"] = supplied_jobs
            if supplied_incremental is not None:
                overrides["CARGO_INCREMENTAL"] = supplied_incremental
            with self.subTest(overrides=overrides):
                kwargs, invocation = self.lease_child(overrides)
                env = kwargs["env"]
                self.assertEqual(env["CARGO_BUILD_JOBS"], jobs)
                self.assertEqual(env.get("CARGO_INCREMENTAL"), incremental)
                # The receipt states the same policy the child received.
                self.assertEqual(invocation["jobs"], int(jobs))
                self.assertEqual(invocation["incremental"],
                                 None if incremental is None else int(incremental))
                # Nothing beyond the validated target and lease is configurable,
                # and no invocation is ever routed through a shell.
                self.assertEqual(set(env) - {"CARGO_INCREMENTAL"},
                                 {"CARGO_TARGET_DIR", "CARGO_BUILD_JOBS"})
                self.assertFalse(kwargs.get("shell", False))

    def test_malformed_lease_is_refused_before_any_child(self):
        from unittest.mock import patch
        for overrides in (
            {"CARGO_BUILD_JOBS": "0"}, {"CARGO_BUILD_JOBS": "-1"}, {"CARGO_BUILD_JOBS": "+2"},
            {"CARGO_BUILD_JOBS": ""}, {"CARGO_BUILD_JOBS": " 2"}, {"CARGO_BUILD_JOBS": "2 "},
            {"CARGO_BUILD_JOBS": "2.0"}, {"CARGO_BUILD_JOBS": "two"}, {"CARGO_BUILD_JOBS": "0x2"},
            {"CARGO_BUILD_JOBS": "02"}, {"CARGO_BUILD_JOBS": "١٢"},
            {"CARGO_BUILD_JOBS": str(VERIFIER.AST_MAX_BUILD_JOBS + 1)},
            {"CARGO_BUILD_JOBS": "2;rm -rf /"}, {"CARGO_BUILD_JOBS": "$(nproc)"},
            {"CARGO_BUILD_JOBS": "2\n4"}, {"CARGO_BUILD_JOBS": "1_0"},
            {"CARGO_INCREMENTAL": "2"}, {"CARGO_INCREMENTAL": ""},
            {"CARGO_INCREMENTAL": "true"}, {"CARGO_INCREMENTAL": "0 "},
            {"CARGO_INCREMENTAL": "-0"}, {"CARGO_INCREMENTAL": "00"},
            {"CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "yes"},
        ):
            with tempfile.TemporaryDirectory() as directory:
                repo = Path(directory).resolve()
                with self.subTest(overrides=overrides), \
                     patch.dict(VERIFIER.os.environ, overrides, clear=True), \
                     patch.object(VERIFIER.subprocess, "run") as run, \
                     self.assertRaises(RuntimeError):
                    try:
                        VERIFIER.run_ast_json(repo, list(VERIFIER.AST_COMMAND), {})
                    finally:
                        run.assert_not_called()

    def test_invocation_schema_rejects_a_forged_effective_budget(self):
        def admits(schema, value):
            if "oneOf" in schema:
                return any(admits(branch, value) for branch in schema["oneOf"])
            if schema["type"] == "null":
                return value is None
            if schema["type"] != "integer" or type(value) is not int:
                return False
            if "enum" in schema and value not in schema["enum"]:
                return False
            return (schema.get("minimum", value) <= value
                    and value <= schema.get("maximum", value))

        invocation = json.loads((SCRIPT.parents[1] / VERIFIER.DEFAULT_MANIFEST).read_text())[
            "tool_contract"]["receipt_schema"]["$defs"]["neutralInvocation"]
        self.assertEqual(set(invocation["required"]), {
            "command", "cwd", "source_sha256", "input_sha256",
            "build_policy", "target_dir", "jobs", "incremental"})
        jobs = invocation["properties"]["jobs"]
        # The forced budget is gone; a bounded validated integer replaces it.
        self.assertNotIn("const", jobs)
        self.assertEqual((jobs["type"], jobs["minimum"], jobs["maximum"]),
                         ("integer", 1, VERIFIER.AST_MAX_BUILD_JOBS))
        for forged in (0, -1, 4.5, "2", "4", True, None,
                       VERIFIER.AST_MAX_BUILD_JOBS + 1):
            self.assertFalse(admits(jobs, forged), repr(forged))
        for honest in (1, 2, 4, 16, VERIFIER.AST_MAX_BUILD_JOBS):
            self.assertTrue(admits(jobs, honest), repr(honest))
        incremental = invocation["properties"]["incremental"]
        for forged in (2, -1, "0", "1", True, 0.0, 1.0):
            self.assertFalse(admits(incremental, forged), repr(forged))
        for honest in (None, 0, 1):
            self.assertTrue(admits(incremental, honest), repr(honest))
        # The producer cannot emit anything the declared schema refuses.
        for overrides in ({}, {"CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0"},
                          {"CARGO_BUILD_JOBS": "1", "CARGO_INCREMENTAL": "1"}):
            with self.subTest(overrides=overrides):
                _, observed = self.lease_child(overrides)
                self.assertTrue(admits(jobs, observed["jobs"]), observed)
                self.assertTrue(admits(incremental, observed["incremental"]), observed)

    def test_receipt_generation_is_declared_not_silently_reused(self):
        contract = json.loads(
            (SCRIPT.parents[1] / VERIFIER.DEFAULT_MANIFEST).read_text())["tool_contract"]
        # The invocation contract gained a required key and dropped a const, so
        # the generation is bumped instead of v2 being redefined underneath the
        # receipts already written against it.
        self.assertEqual(VERIFIER.RECEIPT_SCHEMA, "codescribe.acoustic-structure-receipt.v3")
        self.assertEqual(contract["receipt_version"], VERIFIER.RECEIPT_SCHEMA)
        self.assertIn("codescribe.acoustic-structure-receipt.v2",
                      contract["superseded_versions"])
        self.assertNotIn(VERIFIER.RECEIPT_SCHEMA, contract["superseded_versions"])
        schema = contract["receipt_schema"]
        self.assertTrue(schema["$id"].endswith("acoustic-structure-receipt.v3.json"), schema["$id"])
        self.assertEqual(schema["properties"]["schema"]["const"], VERIFIER.RECEIPT_SCHEMA)


class NeutralAstTests(unittest.TestCase):
    """Actual neutral executable on fresh Loctree data; never import product code."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.repo = SCRIPT.parents[1]
        cls.live = VERIFIER.StructuralVerifier(cls.repo)
        cls.live.context()
        cls.payload = {"schema": "codescribe.structural-ast-input.v1", "bodies": []}
        for symbol, file in VERIFIER.AST_BODIES.items():
            rows = VERIFIER.corridor_body_rows(cls.live.body(symbol, file),
                symbol=symbol, file=file, signature_contains=None)
            if len(rows) != 1:
                raise AssertionError(f"real positive body unavailable: {symbol}")
            cls.payload["bodies"].append(rows[0])

    def run_payload(self, payload):
        return VERIFIER.run_ast_json(self.repo, list(VERIFIER.AST_COMMAND), payload)

    def mutate(self, symbol, old, new):
        payload = copy.deepcopy(self.payload)
        body = next(row for row in payload["bodies"] if row["symbol"] == symbol)
        self.assertIn(old, body["source"], symbol)
        body["source"] = body["source"].replace(old, new, 1)
        body["total_lines"] = len(body["source"].splitlines())
        body["end_line"] = body["start_line"] + body["total_lines"] - 1
        body["line_cap"] = max(body["line_cap"], body["total_lines"])
        return payload

    def test_real_positive_and_comment_only_change(self):
        evidence = self.run_payload(self.payload)
        self.assertTrue(evidence["accepted"], evidence)
        payload = self.mutate("complete_stop", "self.lifecycle_handle = None;",
            "/* return Ok(fake); unknown!(); */ self.lifecycle_handle = None;")
        self.assertTrue(self.run_payload(payload)["accepted"])

    def test_all_eleven_previous_mutants_rejected(self):
        mutations = [
            ("paste_before_guard", "execute_clipboard_paste", "let focus_confirmed = target_app", "clipboard::paste_and_restore(&paste_text)?; let focus_confirmed = target_app"),
            ("focus_removed", "execute_clipboard_paste", "if focus_confirmed && preflight.can_post_events()", "if preflight.can_post_events()"),
            ("preflight_removed", "execute_clipboard_paste", "if focus_confirmed && preflight.can_post_events()", "if focus_confirmed"),
            ("wrong_paste_helper", "paste_text_from_overlay", "self.execute_clipboard_paste(", "self.wrong_helper("),
            ("coverage_removed", "complete_stop", ".terminal_finality(session, self.capture_epoch)", ".wrong_finality(session, self.capture_epoch)"),
            ("incomplete_success_decoy", "complete_stop", "if empty_capture {", "if empty_capture || bypass {"),
            ("audio_receipt_removed", "complete_stop", "                    finality,\n                    audio_path,", "                    finality,"),
            ("committed_receipt_removed", "complete_stop", "                    committed_text: transcript,", ""),
            ("shutdown_bypass", "complete_stop", "if let Some(sender)", "if bypass { return Ok((String::new(), None)); } if let Some(sender)"),
            ("shutdown_order", "complete_stop", "self.transcription_handle = None;", ""),
            ("stop_bypass", "stop", "let stopped =", "if bypass { return Ok((String::new(), None)); } let stopped ="),
        ]
        for name, symbol, old, new in mutations:
            with self.subTest(mutation=name):
                evidence = self.run_payload(self.mutate(symbol, old, new))
                self.assertFalse(evidence["accepted"], name)
                self.assertTrue(any(not row["accepted"] for row in evidence["contracts"]))

    def test_coverage_refusal_predicate_mutants_rejected(self):
        """The terminal filter must refuse every non-complete verdict.

        `== SealCoverageStatus::Incomplete` was the previous grammar and the
        previous product shape. It lets an `Unavailable` receipt — "nothing was
        measured" — fall through to the terminal success, which is the exact
        silence/absence ambiguity the ledger now keeps apart.
        """
        mutations = [
            # The stale grammar's own predicate, restored in the product body.
            ("incomplete_only_restored",
             "!receipt.status.is_complete()",
             "receipt.status == SealCoverageStatus::Incomplete"),
            # Refuse complete receipts and admit the broken ones.
            ("predicate_reversed",
             "!receipt.status.is_complete()",
             "receipt.status.is_complete()"),
            # No adjudication at all: the latest receipt always refuses.
            ("filter_dropped",
             "coverage.is_some_and(|receipt| !receipt.status.is_complete())",
             "coverage.is_some()"),
            # Absence silently treated as completeness.
            ("unavailable_admitted_as_complete",
             "!receipt.status.is_complete()",
             "!receipt.status.is_complete() && receipt.status.unavailable_reason().is_none()"),
        ]
        for name, old, new in mutations:
            with self.subTest(mutation=name):
                evidence = self.run_payload(self.mutate("terminal_finality", old, new))
                self.assertFalse(evidence["accepted"], name)
                contract = next(row for row in evidence["contracts"]
                                if row["symbol"] == "terminal_finality")
                self.assertFalse(contract["accepted"], name)
                self.assertTrue(any("non-complete receipt" in failure
                                    for failure in contract["failures"]), contract)

    def test_terminal_finality_authority_mutants_rejected(self):
        cases = [
            ("stop_mints_seal", "complete_stop", ".terminal_finality(session, self.capture_epoch)", ".seal_terminal(session, self.capture_epoch)"),
            ("zero_sample_guard_removed", "complete_stop", "self.captured_samples.load(Ordering::Relaxed) == 0", "true"),
            ("missing_authority_succeeds", "complete_stop", "match finality {", "if finality.is_none() { return Ok((transcript, audio_path)); } match finality {"),
            ("foreign_receipt", "terminal_finality", "receipt.coverage.capture_epoch == capture_epoch", "true"),
            ("stale_occurrence_set", "terminal_finality", "receipt.sealed_occurrences.contains(range)", "true"),
            ("silence_without_measurement", "terminal_finality", 'receipt.availability == "observed"', "true"),
            ("speech_treated_as_silence", "terminal_finality", "receipt.speech_samples == 0", "true"),
            ("debt_ignored", "terminal_finality", "if pending {", "if false {"),
            ("coverage_rewritten", "terminal_finality", "coverage: coverage.cloned(),", "coverage: None,"),
            ("controller_foreign_take", "process_terminal_stop_error", "retainable_session_id(take_id.as_deref()) != Some(refusal.finality.session_id())", "false"),
            ("bus_foreign_epoch", "matches_refused_document", "last.capture_epoch == refusal.capture_epoch()", "true"),
            ("bus_foreign_words", "matches_refused_document", "last.rendered_text == text", "true"),
            ("bus_ended_admitted", "matches_refused_document", "!writer.ended", "true"),
            ("bus_sealed_admitted", "matches_refused_document", "!writer.sealed", "true"),
        ]
        for name, symbol, old, new in cases:
            with self.subTest(mutation=name):
                self.assertFalse(self.run_payload(self.mutate(symbol, old, new))["accepted"], name)

    def test_control_scope_and_unknown_syntax_counterexamples(self):
        cases = [
            ("else_effect", "execute_clipboard_paste", "self.arm_or_copy_deferred_payload(", "clipboard::paste_and_restore(&paste_text)?; self.arm_or_copy_deferred_payload("),
            ("duplicate_guarded_effect", "execute_clipboard_paste", "OverlayPasteDelivery::Pasted", "clipboard::paste_and_restore(&paste_text)?; OverlayPasteDelivery::Pasted"),
            ("nested_false", "execute_clipboard_paste", "clipboard::paste_and_restore(&paste_text)", "if false { clipboard::paste_and_restore(&paste_text)?; } Ok::<(), Error>(())"),
            ("closure_effect", "execute_clipboard_paste", "clipboard::paste_and_restore(&paste_text)", "(|| clipboard::paste_and_restore(&paste_text))()"),
            ("closure_refusal", "complete_stop", "Err(anyhow::Error::new(TerminalSealRefused {", "|| Err(anyhow::Error::new(TerminalSealRefused {"),
            ("unreachable_shutdown", "complete_stop", "self.lifecycle_handle = None;", "return Err(anyhow::anyhow!(\"early\")); self.lifecycle_handle = None;"),
            ("question_mark_stop", "stop", "self.recorder.stop().await;", "self.recorder.stop().await?;"),
            ("unknown_macro", "complete_stop", "self.lifecycle_handle = None;", "unreviewed!(); self.lifecycle_handle = None;"),
            ("macro_argument_return", "stop", 'info!("Stopping streaming recorder...");', 'info!("{}", { return Ok((String::new(), None)); });'),
            ("unknown_callee", "complete_stop", "self.lifecycle_handle = None;", "unreviewed(); self.lifecycle_handle = None;"),
            ("unknown_loop", "stop", "let stopped =", "while condition { return Err(error); } let stopped ="),
            ("shadow_guard", "execute_clipboard_paste", "let preflight =", "let focus_confirmed = true; let preflight ="),
            ("false_receipt", "complete_stop", "committed_text: transcript,", "committed_text: String::new(),"),
            ("else_success", "complete_stop", "if empty_capture {", "if !empty_capture {"),
            ("attribute", "stop", "pub async fn stop", "#[cfg(any())] pub async fn stop"),
        ]
        for name, symbol, old, new in cases:
            with self.subTest(mutation=name):
                self.assertFalse(self.run_payload(self.mutate(symbol, old, new))["accepted"], name)

    def test_complete_body_and_json_contract_fail_closed(self):
        for patch in ({"truncated": True}, {"extent": "window"}, {"total_lines": 9999},
                      {"source": "fn {"}, {"language": "swift"}):
            payload = copy.deepcopy(self.payload)
            payload["bodies"][0].update(patch)
            with self.subTest(patch=patch):
                self.assertFalse(self.run_payload(payload)["accepted"])
        for payload in ({}, {"schema": "x", "bodies": [], "command": "sh"}):
            with self.assertRaises(RuntimeError):
                self.run_payload(payload)

    def test_missing_and_ambiguous_body_cannot_borrow_other_proof(self):
        for bodies in (self.payload["bodies"][:-1], self.payload["bodies"] + self.payload["bodies"][:1]):
            # Tool emits explicit cardinality refusal; Python also refuses malformed result counts.
            with self.assertRaises(RuntimeError):
                self.run_payload({"schema": self.payload["schema"], "bodies": bodies})

    def test_helper_presence_alone_never_discharges_guards_or_stop(self):
        for symbol in VERIFIER.AST_BODIES:
            payload = copy.deepcopy(self.payload)
            body = next(row for row in payload["bodies"] if row["symbol"] == symbol)
            body.update(source=f"async fn {symbol}() {{ helper().await }}", total_lines=1,
                        end_line=body["start_line"])
            with self.subTest(symbol=symbol):
                self.assertFalse(self.run_payload(payload)["accepted"])

    def test_exact_command_policy_rejects_injection_before_subprocess(self):
        from unittest.mock import patch
        commands = [["cargo", "test"], ["sh", "-c", "true"], [],
            [*VERIFIER.AST_COMMAND, "--manifest-path", "/tmp/Cargo.toml"],
            [*VERIFIER.AST_COMMAND, "--", "anything"],
            [arg.replace("codescribe-structural-ast", "codescribe-core") for arg in VERIFIER.AST_COMMAND],
            [arg for arg in VERIFIER.AST_COMMAND if arg != "--locked"],
            ["/tmp/cargo", *VERIFIER.AST_COMMAND[1:]]]
        with patch.object(VERIFIER.subprocess, "run") as run:
            for command in commands:
                with self.subTest(command=command), self.assertRaises(RuntimeError):
                    VERIFIER.run_ast_json(self.repo, command, self.payload)
            run.assert_not_called()

    def test_failed_missing_stale_or_forged_parser_never_certifies(self):
        from unittest.mock import patch
        from subprocess import CompletedProcess
        for result in (CompletedProcess([], 1, "", "compile failed"),
                       CompletedProcess([], 0, "not JSON", ""),
                       CompletedProcess([], 0, '{"accepted": true}', "")):
            with patch.object(VERIFIER.subprocess, "run", return_value=result), self.assertRaises(RuntimeError):
                self.run_payload(self.payload)
        with patch.object(VERIFIER.subprocess, "run", side_effect=FileNotFoundError("cargo")), self.assertRaises(RuntimeError):
            self.run_payload(self.payload)
        with patch.object(VERIFIER, "ast_tool_digest", side_effect=["before", "after"]), \
             patch.object(VERIFIER.subprocess, "run", return_value=CompletedProcess([], 0, "{}", "")), \
             self.assertRaisesRegex(RuntimeError, "changed during"):
            self.run_payload(self.payload)

    def test_package_contract_rejects_product_dependency_and_build_script(self):
        import shutil
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            shutil.copytree(self.repo / "tools/structural-ast", repo / "tools/structural-ast")
            for name in ("Cargo.toml", "Cargo.lock"):
                shutil.copyfile(self.repo / name, repo / name)
            VERIFIER.ast_tool_digest(repo)
            manifest = repo / "tools/structural-ast/Cargo.toml"
            original = manifest.read_text()
            manifest.write_text(original + '\ncodescribe = { path = "../.." }\n')
            with self.assertRaisesRegex(RuntimeError, "dependency/build"):
                VERIFIER.ast_tool_digest(repo)
            manifest.write_text(original)
            (repo / "tools/structural-ast/build.rs").write_text("fn main() {}")
            with self.assertRaisesRegex(RuntimeError, "dependency/build"):
                VERIFIER.ast_tool_digest(repo)

    def test_compiler_overrides_are_removed_and_receipt_policy_is_exact(self):
        from unittest.mock import patch
        from subprocess import CompletedProcess
        evidence = self.run_payload(self.payload)
        with patch.dict(VERIFIER.os.environ, {"RUSTC_WRAPPER": "/tmp/evil", "RUSTFLAGS": "injected",
                    "CARGO_TARGET_DIR": evidence["invocation"]["target_dir"],
                    "CARGO_ENCODED_RUSTFLAGS": "injected", "RUSTC_WORKSPACE_WRAPPER": "/tmp/evil",
                    "CARGO_BUILD_TARGET": "injected", "DYLD_INSERT_LIBRARIES": "/tmp/evil"}), \
             patch.object(VERIFIER.subprocess, "run", return_value=CompletedProcess([], 0, json.dumps(evidence), "")) as run:
            self.run_payload(self.payload)
            env = run.call_args.kwargs["env"]
            self.assertNotIn("RUSTC_WRAPPER", env)
            self.assertNotIn("RUSTFLAGS", env)
            for key in ("CARGO_ENCODED_RUSTFLAGS", "RUSTC_WORKSPACE_WRAPPER",
                        "CARGO_BUILD_TARGET", "DYLD_INSERT_LIBRARIES"):
                self.assertNotIn(key, env)
            self.assertEqual(env["CARGO_TARGET_DIR"], evidence["invocation"]["target_dir"])
            # The forbidden settings above are stripped while the declared
            # build lease survives: the receipt and the real child argument
            # environment must state the same effective policy, whatever the
            # caller leased. A hardcoded "4" here would re-assert the old
            # forced budget and hide a lease the fleet actually supplied.
            leased_jobs, leased_incremental = VERIFIER.resolve_ast_build_policy()
            self.assertEqual(env["CARGO_BUILD_JOBS"], str(leased_jobs))
            self.assertEqual(run.call_args.kwargs["env"].get("CARGO_INCREMENTAL"),
                             None if leased_incremental is None else str(leased_incremental))
            self.assertFalse(run.call_args.kwargs.get("shell", False))
        verifier = StubVerifier(Path("/repo"))
        receipt, _ = VERIFIER.verify_stage(verifier, wired_manifest(), "wired", None, None)
        receipt["command_inventory"].append(" ".join(VERIFIER.AST_COMMAND))
        with self.assertRaisesRegex(RuntimeError, "authenticated tool receipt"):
            VERIFIER.validate_receipt_shape(receipt)

    def test_neutral_grammar_residue_exception_is_not_a_product_allowlist(self):
        for file, ident, expected in (
            ("tools/structural-ast/src/productions.rs", "OverlayPasteResult", "verifier_self_literal"),
            ("tools/structural-ast/src/productions.rs", "overlay_paste_evil", "unclassified_requires_review"),
            ("core/evil.rs", "OverlayPasteResult", "unclassified_requires_review"),
        ):
            row = occurrence(ident, file=file)
            self.assertEqual(VERIFIER.classify_substring_residue(row, "overlay_paste")[0], expected)

    def test_real_chain_requires_callsite_receipts_as_well_as_ast(self):
        manifest = json.loads((self.repo / VERIFIER.DEFAULT_MANIFEST).read_text())
        contracts = []
        for corridor in manifest["stages"]["wired"]["required_corridors"]:
            hops = [row for row in corridor["hops"] if row.get("ast_contract")]
            if not hops:
                continue
            contracts.append({"name": corridor["name"], "hops": hops,
                "required_invocations": [row for row in corridor["required_invocations"]
                    if row["caller"] in VERIFIER.AST_BODIES and row["callee"] in
                    {"execute_clipboard_paste", "paste_and_restore", "complete_stop", "terminal_finality"}]})
        live = VERIFIER.StructuralVerifier(self.repo)
        observed, failures = VERIFIER.verify_code_corridors(live, contracts)
        self.assertFalse(failures, failures)
        self.assertEqual(sum(len(row["invocations"]) for row in observed.values()), 4)
        for symbol in ("execute_clipboard_paste", "paste_and_restore", "complete_stop", "terminal_finality"):
            original = live.occurrences(symbol)
            missing = copy.deepcopy(original)
            missing["occurrences"] = [row for row in missing["occurrences"] if row.get("match_role") != "reference"]
            live._occurrences[symbol] = missing
            try:
                _, failures = VERIFIER.verify_code_corridors(live, contracts)
                self.assertTrue(failures, symbol)
            finally:
                live._occurrences[symbol] = original


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

    def test_resolves_child_of_rust_2018_file_module(self) -> None:
        # core/llm/speech.rs declares `mod tests;` and rustc resolves it to
        # core/llm/speech/tests.rs. The verifier used to return no candidates
        # for any source that is not lib.rs/main.rs/mod.rs and reported the
        # declaration as unresolved although cargo compiled it (2026-09-08).
        module_payload = regex_payload(
            occurrence("mod tests;", file="core/llm/speech.rs", line=440),
            query=VERIFIER.RUST_MODULE_DECLARATION_PATTERN,
            indexed_files=2,
        )
        path_payload = regex_payload(
            query=VERIFIER.RUST_PATH_ATTRIBUTE_PATTERN,
            indexed_files=2,
        )
        inventory_payload = [
            {"path": "core/llm/speech.rs", "imports": []},
            {"path": "core/llm/speech/tests.rs", "imports": []},
        ]

        self.assertEqual(
            VERIFIER.module_candidates("core/llm/speech.rs", "tests"),
            ["core/llm/speech/tests.rs", "core/llm/speech/tests/mod.rs"],
        )
        unresolved = VERIFIER.unresolved_module_declarations(
            module_payload, path_payload, inventory_payload
        )
        self.assertEqual(unresolved, [])

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


class CurrentChainMutantTests(unittest.TestCase):
    """Counterexamples pushed through the same proof boundary as the positive run.

    Bodies come from live Loctree and are mutated inside the verifier's own
    cache, so every bypass is judged by the real shipped manifest against the
    real current product source. A synthetic fixture would only prove the
    corridor engine; these prove the corridor *contract*.

    The AST hops are excluded here: they own their own neutral-binary contract
    and are falsified separately in NeutralAstTests.
    """

    CORRIDORS = (
        "settings_to_capture_admission",
        "speech_coverage_to_terminal_truth",
        "capture_to_ledger",
    )

    @classmethod
    def setUpClass(cls) -> None:
        cls.repo = SCRIPT.parents[1]
        manifest = json.loads((cls.repo / VERIFIER.DEFAULT_MANIFEST).read_text())
        cls.contracts = []
        for corridor in manifest["stages"]["wired"]["required_corridors"]:
            if corridor["name"] not in cls.CORRIDORS:
                continue
            row = copy.deepcopy(corridor)
            row["hops"] = [hop for hop in row["hops"] if not hop.get("ast_contract")]
            row.pop("ordering", None)
            cls.contracts.append(row)
        if len(cls.contracts) != len(cls.CORRIDORS):
            raise AssertionError(f"corridors missing from manifest: {cls.CORRIDORS}")
        # One warmed evidence cache; every mutant below reuses it offline.
        cls.seed = VERIFIER.StructuralVerifier(cls.repo)
        cls.seed.context()
        _, cls.positive_failures = VERIFIER.verify_code_corridors(cls.seed, cls.contracts)

    def run_mutations(self, symbol, file, pairs):
        """Apply an ordered edit script to one live body and re-judge it.

        Some bypasses cannot be expressed as a single substitution: moving a
        statement means deleting it here and re-inserting it there. One pair
        keeps the old single-edit call shape; several pairs express a move.
        """
        clone = VERIFIER.StructuralVerifier(self.repo)
        clone._occurrences = dict(self.seed._occurrences)
        clone._literal_occurrences = dict(self.seed._literal_occurrences)
        clone._bodies = copy.deepcopy(self.seed._bodies)
        row = next(item for item in clone._bodies[(symbol, file)]["bodies"]
                   if item["symbol"] == symbol)
        for old, new in pairs:
            self.assertIn(old, row["source"], f"{symbol}: stale mutation anchor")
            row["source"] = row["source"].replace(old, new, 1)
        row["total_lines"] = len(row["source"].splitlines())
        row["end_line"] = row["start_line"] + row["total_lines"] - 1
        _, failures = VERIFIER.verify_code_corridors(clone, self.contracts)
        return failures

    def run_mutated(self, symbol, file, old, new):
        return self.run_mutations(symbol, file, [(old, new)])

    def test_positive_current_chains_pass(self):
        self.assertEqual(self.positive_failures, [])

    def test_captured_input_chain_bypasses_are_rejected(self):
        cases = [
            # Capture is disconnected from the resolver: the snapshot is sealed
            # from facts nobody captured on this launch.
            ("capture_disconnected_from_resolver",
             "load_runtime_snapshot_with_keychain_population", "core/config/loader.rs",
             "let snapshot = Self::resolve_runtime_snapshot_with_capture(|| input);",
             "let snapshot = Self::runtime_snapshot_from_captured(Default::default());",
             "load_runtime_snapshot_with_keychain_population"),
            # A different input is substituted for the captured one.
            ("substituted_input",
             "load_runtime_snapshot_with_keychain_population", "core/config/loader.rs",
             "Self::resolve_runtime_snapshot_with_capture(|| input)",
             "Self::resolve_runtime_snapshot_with_capture(|| CapturedRuntimeInputs::default())",
             "load_runtime_snapshot_with_keychain_population"),
            # Calibration is never acquired, so the sealed snapshot carries none.
            ("calibration_omitted_at_capture",
             "capture_runtime_inputs", "core/config/loader.rs",
             "let energy_calibration = SealedEnergyCalibration::load(&energy_calibration_path);",
             "",
             "capture_runtime_inputs"),
            # Calibration is acquired but never reaches the digest.
            ("calibration_dropped_from_digest",
             "runtime_snapshot_from_captured", "core/config/loader.rs",
             "input.energy_calibration.digest_material()",
             "String::new()",
             "runtime_snapshot_from_captured"),
            # Calibration is acquired but never reaches the sealed parts.
            ("calibration_dropped_from_seal",
             "runtime_snapshot_from_captured", "core/config/loader.rs",
             "energy_calibration: input.energy_calibration,",
             "energy_calibration: SealedEnergyCalibration::default(),",
             "runtime_snapshot_from_captured"),
            # The resolver stops handing the captured value to the sealer.
            ("resolver_stops_calling_the_sealer",
             "resolve_runtime_snapshot_with_capture", "core/config/loader.rs",
             "Self::runtime_snapshot_from_captured(capture())",
             "RuntimeSettingsSnapshot::default()",
             "resolve_runtime_snapshot_with_capture"),
        ]
        for name, symbol, file, old, new, obligation in cases:
            with self.subTest(mutation=name):
                failures = self.run_mutated(symbol, file, old, new)
                self.assertTrue(failures, name)
                self.assertTrue(
                    any("settings_to_capture_admission" in failure and obligation in failure
                        for failure in failures), (name, failures))

    def test_acoustic_chain_bypasses_are_rejected(self):
        cases = [
            # Invalid capture measurement no longer wins over Silero speech, so
            # unreadable PCM can be certified by a later observer.
            ("invalid_capture_precedence_bypassed",
             "coverage_speech_evidence", "core/pipeline/streaming/apple_live_session.rs",
             "return capture_energy;\n    }\n    if let Some(fusion)",
             "()\n    }\n    if let Some(fusion)",
             "coverage_speech_evidence"),
            # The Silero branch stops being held against the captured extent.
            ("extent_check_removed_from_silero_branch",
             "coverage_speech_evidence", "core/pipeline/streaming/apple_live_session.rs",
             "return within_capture(acoustic, captured_samples);",
             "return acoustic;",
             "coverage_speech_evidence"),
            # The capture-energy branch stops being held against the extent.
            ("extent_check_removed_from_capture_branch",
             "coverage_speech_evidence", "core/pipeline/streaming/apple_live_session.rs",
             "within_capture(capture_energy, captured_samples)\n}",
             "capture_energy\n}",
             "coverage_speech_evidence"),
            # A short observation is promoted instead of downgraded.
            ("short_observation_promoted",
             "within_capture", "core/pipeline/streaming/apple_live_session.rs",
             "AcousticAvailability::Discontinuous { observed_samples }",
             "AcousticAvailability::Observed { observed_samples }",
             "within_capture"),
            # The extent comparison itself disappears.
            ("extent_comparison_dropped",
             "within_capture", "core/pipeline/streaming/apple_live_session.rs",
             "if observed_samples >= captured_samples {",
             "if true {",
             "within_capture"),
            # The ledger goes back to assessing bare ranges without availability.
            ("ranges_only_assessment_restored",
             "assess_seal_coverage", "core/pipeline/acoustic_ledger.rs",
             "speech: &AcousticSpeechEvidence,",
             "speech_ranges: &[TailSampleRange],",
             "assess_seal_coverage"),
            # Absence of measurement stops refusing the terminal seal.
            ("unavailable_stops_refusing_the_seal",
             "seal_terminal", "core/pipeline/acoustic_ledger.rs",
             "|| coverage.status.unavailable_reason().is_some()",
             "",
             "seal_terminal"),
        ]
        for name, symbol, file, old, new, obligation in cases:
            with self.subTest(mutation=name):
                failures = self.run_mutated(symbol, file, old, new)
                self.assertTrue(failures, name)
                self.assertTrue(
                    any("speech_coverage_to_terminal_truth" in failure and obligation in failure
                        for failure in failures), (name, failures))

    def test_capture_chain_bypasses_are_rejected(self):
        """Break one capture guard at a time and require a named refusal.

        Every case names the hop that must reject it, so a failure firing for
        an unrelated reason cannot be read as a rejection. Ownership, identity,
        calibration, exact PCM and the qualification-before-admission order are
        each falsified separately; none of them is proven by the positive run
        alone, because a positive that cannot go red proves only that it ran.
        """
        apple = "core/pipeline/streaming/apple_live_session.rs"
        cases = [
            # The armed branch stops routing into the physical lane at all.
            ("armed_routing_removed", "seal_utterance_final",
             [("        seal_sliced_by_silero(state, ev_tx, &segments);\n        return true;\n", "")]),
            # The armed branch forwards cursor-filtered segments instead of the
            # original timed ones, so revised earlier coordinates are lost.
            ("armed_branch_forwards_filtered_input", "seal_utterance_final",
             [("seal_sliced_by_silero(state, ev_tx, &segments);",
               "seal_sliced_by_silero(state, ev_tx, &disjoint);")]),
            # Untimed Apple text is allowed to create an occurrence.
            ("untimed_segments_accepted", "seal_utterance_final",
             [("if segments.is_empty() {", "if segments.len() > usize::MAX {")]),
            # The legacy cursor filter is moved ahead of the armed routing.
            ("cursor_filter_precedes_armed_routing", "seal_utterance_final",
             [("    if state.fusion_seal_armed {",
               "    let mut cursor = state.last_apple_segment_end;\n    if state.fusion_seal_armed {"),
              ("    let mut cursor = state.last_apple_segment_end;\n    let mut overlap_normalized = false;",
               "    let mut overlap_normalized = false;")]),
            # The dispatcher stops delegating to reconciliation.
            ("dispatcher_stops_reconciling", "seal_sliced_by_silero",
             [("reconcile_silero_ledger(state, ev_tx, &ledger, disjoint)", "true")]),
            # The revision fence stops being advanced, so a reopened range can
            # no longer be distinguished from a replayed one.
            ("slice_revision_guard_dropped", "seal_sliced_by_silero",
             [("    state.silero_slice_revision = Some((utterances.len(), utterances.last().cloned()));\n", "")]),
            # Evidence from another capture is accepted as this occurrence's.
            ("foreign_capture_identity_accepted", "reconcile_silero_ledger",
             [("utterance.range.session != state.session_id", "false")]),
            # The identity refusal warns and then continues anyway.
            ("identity_refusal_falls_through", "reconcile_silero_ledger",
             [("        });\n        return false;\n    }\n    let apple_words",
               "        });\n    }\n    let apple_words")]),
            # Ownership is minted from the padded recognition context instead of
            # the physical Silero range.
            ("ownership_minted_from_padded_range", "reconcile_silero_ledger",
             [("let occurrence = OccurrenceIdentity::from(&silero.range);",
               "let occurrence = OccurrenceIdentity::from(&bound_context_range(&silero.range, context, pad_samples, &bounds));")]),
            # The qualifier is still called but its refusal cannot fire.
            ("qualification_refusal_ignored", "reconcile_silero_ledger",
             [("if !qualify_owned_occurrence(state, &occurrence) {",
               "if !qualify_owned_occurrence(state, &occurrence) && false {")]),
            # A refused qualification stops being recorded as a failure.
            ("refusal_not_routed_to_failure", "reconcile_silero_ledger",
             [("state.fail_refinement(ev_tx, utterance_id, &occurrence, reason);", "")]),
            # Admission re-qualifies itself instead of reusing the qualification
            # reconciliation already established, which would let the ordering
            # guarantee be satisfied by accident.
            ("admission_self_qualifies_instead_of_reusing", "reconcile_silero_ledger",
             [("energy: EnergyAdmission::RequireExistingQualification,",
               "energy: EnergyAdmission::QualifyFromOwnedPcm,")]),
            # A comment carrying the exact required text discharges nothing.
            ("comment_cannot_discharge", "reconcile_silero_ledger",
             [("        if !qualify_owned_occurrence(state, &occurrence) {",
               "        // if !qualify_owned_occurrence(state, &occurrence) {\n"
               "        if occurrence.sample_end == 0 {")]),
            # Neither does a string literal carrying the same text.
            ("string_literal_cannot_discharge", "reconcile_silero_ledger",
             [('        if !qualify_owned_occurrence(state, &occurrence) {',
               '        let _marker = "if !qualify_owned_occurrence(state, &occurrence) {";\n'
               '        if occurrence.sample_end == 0 {')]),
            # Qualification stops checking the capture epoch.
            ("wrong_session_or_epoch_qualified", "qualify_owned_occurrence",
             [("if occurrence.session != state.session_id || occurrence.capture_epoch != state.capture_epoch {",
               "if occurrence.session != state.session_id {")]),
            # Calibration becomes optional, so energy is measured against none.
            ("calibration_omitted", "qualify_owned_occurrence",
             [("let Some(calibration) = state.energy_calibration.as_ref() else {",
               "let Some(calibration) = state.energy_calibration.as_ref().or(Some(&EnergyCalibration::DEFAULT)) else {")]),
            # The exact owned window is padded, so measured energy would come
            # from samples this occurrence does not own.
            ("exact_window_replaced_by_padded", "qualify_owned_occurrence",
             [("let Some(window) = state.window_by_samples(occurrence.sample_start, occurrence.sample_end)",
               "let Some(window) = state.window_by_samples(occurrence.sample_start.saturating_sub(16_000), occurrence.sample_end.saturating_add(16_000))")]),
            # An empty PCM window stops refusing qualification.
            ("empty_pcm_accepted", "qualify_owned_occurrence",
             [("if window.samples.is_empty() {", "if window.samples.len() > usize::MAX {")]),
            # The ledger never mints evidence; a prior qualification is reused.
            ("ledger_qualify_dropped", "qualify_owned_occurrence",
             [("if !ledger.qualify(&evidence, calibration).is_qualified() {",
               "if !ledger.is_qualified(occurrence) {")]),
            # Evidence is minted against a calibration version nobody measured.
            ("calibration_version_forged", "qualify_owned_occurrence",
             [("evidence_calibration_version: calibration.version.clone(),",
               "evidence_calibration_version: String::new(),")]),
            # Admission stops requiring an existing qualification.
            ("existing_qualification_guard_bypassed", "admit_ledger_label",
             [("    if !ledger.is_qualified(&occurrence) {\n        return None;\n    }\n", "")]),
            # The self-qualifying admission modes stop invoking the qualifier.
            ("energy_mode_guard_ignored", "admit_ledger_label",
             [(") && !qualify_owned_occurrence(state, &occurrence)", ") && true")]),
            # Admission stops checking which session the occurrence belongs to.
            ("admission_session_epoch_guard_narrowed", "admit_ledger_label",
             [("if occurrence.session != state.session_id || occurrence.capture_epoch != state.capture_epoch {",
               "if occurrence.capture_epoch != state.capture_epoch {")]),
        ]
        for name, symbol, pairs in cases:
            with self.subTest(mutation=name):
                failures = self.run_mutations(symbol, apple, pairs)
                self.assertTrue(failures, name)
                self.assertTrue(
                    any("capture_to_ledger" in failure and symbol in failure
                        for failure in failures), (name, failures))

    def test_retired_capture_names_are_absent_from_the_manifest(self):
        """The stale capture corridor named a predicate and an argument shape
        the product retired in the delegation refactor. Naming them again must
        be caught here rather than by a corridor that dies before it reports.
        """
        manifest = (self.repo / VERIFIER.DEFAULT_MANIFEST).read_text()
        for retired in (
            "state.fusion_seal_armed && seal_sliced_by_silero(state, ev_tx, &disjoint)",
            "window_by_samples(occurrence.sample_start, occurrence.sample_end)?",
            "let energy_integral = window.samples.iter()",
            "let peak = window.samples.iter()",
        ):
            self.assertNotIn(retired, manifest, retired)

    def test_retired_acoustic_names_are_absent_from_the_manifest(self):
        """The stale corridor named symbols the product no longer defines.

        Loctree resolves them to zero bodies, so the verifier died before any
        receipt existed. Naming a retired symbol again must be caught here, not
        by a fail-closed run that reports no failures at all.
        """
        manifest = (self.repo / VERIFIER.DEFAULT_MANIFEST).read_text()
        for retired in ("coverage_speech_ranges_with_source",
                        "select_coverage_speech_source",
                        "coverage_speech_ranges"):
            self.assertNotIn(f'"{retired}"', manifest, retired)


class CaptureOrderingProofTests(unittest.TestCase):
    """The capture corridor judged with its ordering contract still attached.

    `CurrentChainMutantTests` drops `ordering` because a body mutation cannot
    move a callsite: observed lines come from occurrences, not from source
    text. Ordering therefore needs its own falsifiers, which mutate the
    occurrence receipts instead. Without this class the ordering rows would
    only ever be observed green, and a green that cannot go red is decoration.
    """

    APPLE = "core/pipeline/streaming/apple_live_session.rs"
    CORRIDOR = "capture_to_ledger"

    @classmethod
    def setUpClass(cls) -> None:
        cls.repo = SCRIPT.parents[1]
        manifest = json.loads((cls.repo / VERIFIER.DEFAULT_MANIFEST).read_text())
        cls.contracts = [
            copy.deepcopy(corridor)
            for corridor in manifest["stages"]["wired"]["required_corridors"]
            if corridor["name"] == cls.CORRIDOR
        ]
        if len(cls.contracts) != 1:
            raise AssertionError(f"{cls.CORRIDOR} missing from manifest")
        if not cls.contracts[0].get("ordering"):
            raise AssertionError(f"{cls.CORRIDOR} declares no ordering contract")
        cls.seed = VERIFIER.StructuralVerifier(cls.repo)
        cls.seed.context()
        cls.observations, cls.failures = VERIFIER.verify_code_corridors(
            cls.seed, cls.contracts
        )

    def clone(self):
        clone = VERIFIER.StructuralVerifier(self.repo)
        clone._occurrences = copy.deepcopy(self.seed._occurrences)
        clone._literal_occurrences = dict(self.seed._literal_occurrences)
        clone._bodies = copy.deepcopy(self.seed._bodies)
        return clone

    def callsites(self, verifier, callee, caller):
        rows = verifier._occurrences[callee]["occurrences"]
        return [
            row
            for row in rows
            if row.get("file") == self.APPLE
            and row.get("match_role") == "reference"
            and isinstance(row.get("enclosing_symbol"), dict)
            and row["enclosing_symbol"].get("name") == caller
        ]

    def test_positive_capture_chain_with_ordering_passes(self):
        self.assertEqual(self.failures, [])
        ordering = self.observations[self.CORRIDOR]["ordering"]
        self.assertTrue(ordering, "ordering rows were not observed at all")
        for row in ordering:
            with self.subTest(barrier=row["barrier"]["required_code"]):
                self.assertEqual(row["verdict"], "GREEN", row)
                # Relational, never absolute: an unrelated edit above these
                # functions must not turn a real proof red.
                self.assertTrue(row["before_observed_lines"], row)
                self.assertTrue(row["after_observed_lines"], row)
                self.assertTrue(row["barrier_observed_lines"], row)
                self.assertLess(
                    max(row["before_observed_lines"]),
                    min(row["barrier_observed_lines"]),
                    row,
                )
                self.assertLess(
                    max(row["barrier_observed_lines"]),
                    min(row["after_observed_lines"]),
                    row,
                )

    def test_each_capture_edge_is_individually_required(self):
        """Remove one declared edge at a time; each must be named in refusal."""
        cases = [
            ("dispatcher_to_reconciliation", "reconcile_silero_ledger",
             "seal_sliced_by_silero", 0),
            ("worker_to_dispatcher", "seal_sliced_by_silero",
             "apple_stream_worker", 0),
            ("reconciliation_to_qualification", "qualify_owned_occurrence",
             "reconcile_silero_ledger", 0),
            ("reconciliation_to_admission", "admit_ledger_label",
             "reconcile_silero_ledger", 0),
            ("admission_to_qualification", "qualify_owned_occurrence",
             "admit_ledger_label", 0),
            ("qualification_to_ledger", "qualify", "qualify_owned_occurrence", 0),
            # Halving a two-callsite edge: the Lexicon admission and the second
            # worker tick are obligations, not decoration. A `minimum_count`
            # nobody enforces would let either disappear silently.
            ("lexicon_admission_dropped", "admit_ledger_label",
             "reconcile_silero_ledger", 1),
            ("second_worker_tick_dropped", "seal_sliced_by_silero",
             "apple_stream_worker", 1),
        ]
        for name, callee, caller, keep in cases:
            with self.subTest(edge=name):
                clone = self.clone()
                hits = self.callsites(clone, callee, caller)
                self.assertGreater(len(hits), keep, f"{name}: stale edge anchor")
                for row in hits[keep:]:
                    clone._occurrences[callee]["occurrences"].remove(row)
                _, failures = VERIFIER.verify_code_corridors(clone, self.contracts)
                self.assertTrue(failures, name)
                self.assertTrue(
                    any(self.CORRIDOR in failure and callee in failure
                        and caller in failure for failure in failures),
                    (name, failures))

    def test_qualification_must_precede_admission(self):
        """Move the qualification callsite past both admissions.

        Nothing about the bodies changes; only where the call happens. The
        corridor must still refuse, because ownership qualified after the
        label is admitted is not ownership.
        """
        clone = self.clone()
        admissions = self.callsites(clone, "admit_ledger_label", "reconcile_silero_ledger")
        self.assertTrue(admissions, "stale admission anchor")
        moved = max(int(row["line"]) for row in admissions) + 1
        hits = self.callsites(clone, "qualify_owned_occurrence", "reconcile_silero_ledger")
        self.assertTrue(hits, "stale qualification anchor")
        for row in hits:
            row["line"] = moved
        _, failures = VERIFIER.verify_code_corridors(clone, self.contracts)
        self.assertTrue(failures)
        self.assertTrue(
            any(self.CORRIDOR in failure and "ordering" in failure
                and "qualify_owned_occurrence" in failure
                and "admit_ledger_label" in failure for failure in failures),
            failures)

    def test_ordering_barriers_are_required(self):
        """Delete a declared barrier and the ordering claim loses its guard.

        The seal-immutability barrier is deliberately not one of the hop's
        `required_code` snippets, so its removal can only be caught by the
        ordering contract. That is what makes it a barrier test rather than a
        second copy of the body test.
        """
        clone = self.clone()
        row = next(
            item
            for item in clone._bodies[("reconcile_silero_ledger", self.APPLE)]["bodies"]
            if item["symbol"] == "reconcile_silero_ledger"
        )
        anchor = "            if ledger.is_sealed(&occurrence) {"
        self.assertIn(anchor, row["source"], "stale barrier anchor")
        row["source"] = row["source"].replace(
            anchor, "            if ledger.frontier_of(&occurrence).is_some() {", 1
        )
        row["total_lines"] = len(row["source"].splitlines())
        row["end_line"] = row["start_line"] + row["total_lines"] - 1
        _, failures = VERIFIER.verify_code_corridors(clone, self.contracts)
        self.assertTrue(failures)
        self.assertTrue(
            any(self.CORRIDOR in failure and "barrier" in failure
                for failure in failures), failures)

    def test_capture_invocations_use_the_key_the_verifier_reads(self):
        """`minimum_count` is read; `min_calls` is not.

        `verify_code_corridors` reads `minimum_count` and defaults to 1. A
        manifest entry spelled `min_calls: 2` is therefore silently downgraded
        to 1 — it looks like an enforced obligation and is not one. This pins
        the capture corridor to the spelling the instrument actually consumes;
        the other corridors are a separate, reported finding.
        """
        manifest = json.loads((self.repo / VERIFIER.DEFAULT_MANIFEST).read_text())
        corridor = next(
            row
            for row in manifest["stages"]["wired"]["required_corridors"]
            if row["name"] == self.CORRIDOR
        )
        for invocation in corridor["required_invocations"]:
            with self.subTest(invocation=invocation["caller"]):
                self.assertNotIn("min_calls", invocation, invocation)
                self.assertIn("minimum_count", invocation, invocation)

    def test_local_const_callsite_attribution_limit_is_declared(self):
        """A declared instrument limit, pinned so it cannot rot silently.

        `seal_utterance_final` opens with a function-local `const`, and
        Loctree attributes every callsite in that function to the constant
        instead of the function. Its two real `admit_ledger_label` calls and
        its `seal_sliced_by_silero` call therefore observe zero production
        callsites, so those edges are deliberately NOT declared — declaring
        them would be a red obligation caused by the provider, not the product.

        The bodies below prove the calls exist. When the provider is fixed this
        test goes red and forces the corridor to gain the missing edges, which
        is exactly the behaviour a silent limit does not have.
        """
        body = VERIFIER.corridor_body_rows(
            self.seed.body("seal_utterance_final", self.APPLE),
            symbol="seal_utterance_final",
            file=self.APPLE,
            signature_contains=None,
        )
        self.assertEqual(len(body), 1)
        source = VERIFIER.code_without_comments_or_strings(body[0]["source"])
        for present in ("admit_ledger_label(", "seal_sliced_by_silero(state,ev_tx,&segments)"):
            self.assertIn(present, source, present)
        for callee in ("admit_ledger_label", "seal_sliced_by_silero"):
            with self.subTest(callee=callee):
                self.assertEqual(
                    self.callsites(self.seed, callee, "seal_utterance_final"), [],
                    f"{callee}: provider now attributes this caller; declare the edge")
        corridor = next(
            row for row in self.contracts if row["name"] == self.CORRIDOR
        )
        for invocation in corridor["required_invocations"]:
            self.assertNotEqual(
                invocation["caller"], "seal_utterance_final",
                "an edge is declared on a caller the provider cannot attribute")


if __name__ == "__main__":
    unittest.main()
