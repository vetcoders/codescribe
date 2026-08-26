#!/usr/bin/env python3
"""Emit structure-only receipts for the one-throne transplant.

This verifier deliberately knows no Cargo, compiler, Swift runner, application
launcher, or runtime probe. Its only subprocess executable is `loct`; every
executed command is recorded in the receipt and checked before dispatch. A
small local Rust-module resolution pass complements Loctree so deleted files
cannot remain referenced by `mod name;` declarations.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


RECEIPT_SCHEMA = "codescribe.acoustic-structure-receipt.v1"
DEFAULT_MANIFEST = "tests/fixtures/acoustic_throne_stages.json"
ALLOWED_EXECUTABLE = "loct"
STAGE_VERDICTS = {
    "demolished": "OLD_AUTHORITY_REMOVED",
    "assembled": "STRUCTURALLY_COMPLETE_NOT_WIRED",
    "wired": "STRUCTURALLY_WIRED",
}
SOURCE_SCOPES = {"production", "generated"}
FORBIDDEN_SCOPES = SOURCE_SCOPES | {"test"}
NON_CODE_ROLES = {"comment", "string_literal"}
RUST_SOURCE_ROOTS = ("app", "bin", "bridge", "core", "examples", "tests")
RUST_MOD_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)
RUST_PATH_RE = re.compile(r'^\s*#\[path\s*=\s*"([^"]+)"\]\s*$')
VERIFIER_INFRASTRUCTURE = {
    "scripts/verify-acoustic-throne-structure.py",
    "scripts/verify-authority-shape.sh",
    "scripts/verify-five-iwo.sh",
}
VERIFIER_LITERAL_PATHS = VERIFIER_INFRASTRUCTURE | {
    "scripts/tests/test_verify_acoustic_throne_structure.py",
}
RESIDUE_CLASSES = (
    "executable_authority",
    "executable_consumer",
    "canonical_edge",
    "harmless_historical_name",
    "diagnostic_or_log_label",
    "comment_or_string",
    "test_only_mutant",
    "fixture",
    "verifier_self_literal",
    "unclassified_requires_review",
)
RESIDUE_RELATIONS = {
    "snake_infix",
    "snake_prefix",
    "snake_suffix",
    "pascal_twin",
    "filename",
    "verbatim_substring",
}
REQUIRED_REGEX_TRUST_KEYS = (
    "pattern_compiled",
    "file_scope_resolved",
    "absence_trustworthy_for_scanned",
)
RESIDUE_ROW_KEYS = {
    "file",
    "line",
    "matched_identifier",
    "needle",
    "relation",
    "match_role",
    "scope_classification",
    "class",
    "fail_gate",
    "review_required",
    "reason",
}
KNOWN_RESIDUE_TWINS = {
    "stream_postprocess": ("StreamPostProcessor",),
}
REQUIRED_RECEIPT_KEYS = {
    "schema",
    "stage",
    "scope",
    "verdict",
    "expected_verdict",
    "conformant",
    "repo",
    "branch",
    "head",
    "dirty_fingerprint",
    "loctree_version",
    "loctree_snapshot_fingerprint",
    "expected_parts",
    "observed_parts",
    "expected_owners",
    "observed_owners",
    "forbidden_symbols",
    "forbidden_hits",
    "residue_by_substring",
    "consumer_paths",
    "bypass_paths",
    "unwired_paths",
    "dangling_references",
    "unresolved_module_declarations",
    "canary_rows_observed",
    "unclassified_canary_rows",
    "failures",
    "assessment",
    "command_inventory",
    "command_policy",
}
ASSEMBLY_SCOPE_SYMBOLS = {
    "settings": {
        "RuntimeSettingsSnapshot",
        "SettingsSnapshotProvenance",
        "SettingsSnapshotDigest",
    },
    "acoustic": {
        "AcousticLedger",
        "OccurrenceIdentity",
        "ObservationIdentity",
        "AcousticSerial",
        "WordEvidenceReceipt",
        "LayerDecisionReceipt",
        "ManualEditReceipt",
        "ObservationFrontier",
        "LedgerSealReceipt",
    },
    "document": {
        "TranscriptReducer",
        "TranscriptDocumentEntry",
        "TranscriptRevision",
        "ReducerAction",
        "ProjectedAcousticReceipt",
        "TranscriptBusEvidenceEvent",
    },
    "text-delivery": {"OccurrenceLabelProposal", "DeliveryRoute"},
}
ASSEMBLY_SCOPES = set(ASSEMBLY_SCOPE_SYMBOLS)


@dataclass(frozen=True)
class LoctResult:
    command: list[str]
    payload: dict[str, Any]


class StructuralVerifier:
    def __init__(self, repo: Path) -> None:
        self.repo = repo
        self.command_inventory: list[list[str]] = []
        self._occurrences: dict[str, dict[str, Any]] = {}
        self._substring_occurrences: dict[str, dict[str, Any]] = {}

    def run_loct(self, *args: str) -> LoctResult:
        command = [ALLOWED_EXECUTABLE, *args]
        if command[0] != ALLOWED_EXECUTABLE:
            raise RuntimeError(f"structural verifier refused non-Loctree command: {command}")
        if "--json" not in command:
            command.append("--json")
        self.command_inventory.append(command)
        completed = subprocess.run(
            command,
            cwd=self.repo,
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            raise RuntimeError(
                f"Loctree command failed ({completed.returncode}): {' '.join(command)}\n"
                f"{completed.stderr.strip()}"
            )
        try:
            payload = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            raise RuntimeError(f"Loctree returned non-JSON for {' '.join(command)}: {error}") from error
        return LoctResult(command, payload)

    def context(self) -> dict[str, Any]:
        return self.run_loct("context").payload

    def occurrences(self, symbol: str) -> dict[str, Any]:
        if symbol not in self._occurrences:
            self._occurrences[symbol] = self.run_loct("occurrences", symbol).payload
        return self._occurrences[symbol]

    def substring_occurrences(self, needle: str) -> dict[str, Any]:
        if needle not in self._substring_occurrences:
            pattern = substring_identifier_pattern(needle)
            self._substring_occurrences[needle] = self.run_loct(
                "find", "--regex", pattern
            ).payload
        return self._substring_occurrences[needle]


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text())
    except FileNotFoundError as error:
        raise SystemExit(f"missing structural verifier input: {path}") from error
    except json.JSONDecodeError as error:
        raise SystemExit(f"invalid JSON in structural verifier input {path}: {error}") from error


def code_occurrences(
    payload: dict[str, Any], scopes: set[str]
) -> list[dict[str, Any]]:
    return [
        occurrence
        for occurrence in payload.get("occurrences", [])
        if occurrence.get("scope_classification") in scopes
        and occurrence.get("match_role") not in NON_CODE_ROLES
        and occurrence.get("file") not in VERIFIER_INFRASTRUCTURE
    ]


def production_occurrences(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return code_occurrences(payload, SOURCE_SCOPES)


def forbidden_occurrences(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return code_occurrences(payload, FORBIDDEN_SCOPES)


def pascal_twin(needle: str) -> str:
    parts = [part for part in re.split(r"[^A-Za-z0-9]+", needle) if part]
    if len(parts) < 2:
        return needle
    return "".join(part[:1].upper() + part[1:] for part in parts)


def substring_variants(needle: str) -> list[str]:
    return list(
        dict.fromkeys(
            [needle, pascal_twin(needle), *KNOWN_RESIDUE_TWINS.get(needle, ())]
        )
    )


def substring_identifier_pattern(needle: str) -> str:
    alternatives = "|".join(re.escape(variant) for variant in substring_variants(needle))
    return rf"[A-Za-z0-9_]*({alternatives})[A-Za-z0-9_]*"


def occurrence_key(occurrence: dict[str, Any]) -> tuple[str, int, int]:
    return (
        str(occurrence.get("file", "")),
        int(occurrence.get("line", 0)),
        int(occurrence.get("column", 0)),
    )


def residue_relation(matched_identifier: str, needle: str, file: str) -> str:
    twins = [variant for variant in substring_variants(needle) if variant != needle]
    if any(twin in matched_identifier for twin in twins):
        return "pascal_twin"
    if needle in matched_identifier:
        if matched_identifier == needle:
            return "verbatim_substring"
        if matched_identifier.startswith(needle):
            return "snake_prefix"
        if matched_identifier.endswith(needle):
            return "snake_suffix"
        return "snake_infix"
    if needle in Path(file).name or any(twin in Path(file).name for twin in twins):
        return "filename"
    return "verbatim_substring"


def is_fixture_path(file: str) -> bool:
    return file.startswith("tests/fixtures/") or "/fixtures/" in file


def is_test_path(file: str) -> bool:
    path = Path(file)
    return (
        file.startswith("tests/")
        or file.startswith("scripts/tests/")
        or any(part.endswith("Tests") for part in path.parts)
    )


def classify_substring_residue(
    occurrence: dict[str, Any],
    needle: str,
    *,
    disabled_cfg_any: bool = False,
) -> tuple[str, str]:
    file = str(occurrence.get("file", ""))
    matched_identifier = str(occurrence.get("matched_text", ""))
    match_role = str(occurrence.get("match_role", "unknown"))
    scope = str(occurrence.get("scope_classification", "unknown"))
    context = str(occurrence.get("context", "")).lower()

    if file in VERIFIER_LITERAL_PATHS:
        return "verifier_self_literal", "verifier-owned evidence literal"
    if is_fixture_path(file):
        return "fixture", "fixture or frozen structural manifest"
    if file.startswith("docs/"):
        return "harmless_historical_name", "historical documentation name"
    if match_role in NON_CODE_ROLES:
        return "comment_or_string", "Loctree marks the match as non-code text"
    pascal_needle = pascal_twin(needle)
    if (
        file == "core/quality/engine_contract.rs"
        and matched_identifier == f"Lexicon{pascal_needle}"
    ) or (
        file
        in {
            "app/controller/delivery_route.rs",
            "app/controller/mod.rs",
            "bridge/src/hotkeys.rs",
        }
        and matched_identifier
        in {f"{pascal_needle}Delivery", f"{pascal_needle}Result"}
    ):
        return "canonical_edge", "fixed-throne type or relay edge"
    if (
        disabled_cfg_any
        or scope == "test"
        or is_test_path(file)
        or matched_identifier.startswith("test_")
    ):
        return "test_only_mutant", "test-only or cfg-disabled evidence"
    if matched_identifier.startswith("should_drop"):
        return "executable_authority", "engine-local predicate can drop decoder output"
    if matched_identifier.endswith("Dropped") or any(
        marker in context for marker in ("display", "diagnostic", "qualityissuekind", "report")
    ):
        return "diagnostic_or_log_label", "diagnostic or report label for a drop decision"
    if "_dropped" in matched_identifier:
        return "executable_consumer", "drop-state field or flag is forwarded or consumed"
    return "unclassified_requires_review", "no deterministic taxonomy rule matched"


def inside_non_runtime_cfg_module(
    repo: Path | None, occurrence: dict[str, Any]
) -> bool:
    if repo is None:
        return False
    relative = Path(str(occurrence.get("file", "")))
    if relative.suffix != ".rs":
        return False
    source = repo / relative
    if not source.is_file():
        return False
    target = int(occurrence.get("line", 0)) - 1
    lines = source.read_text(errors="replace").splitlines()
    if not 0 <= target < len(lines):
        return False
    for attribute_index in range(target, -1, -1):
        if not re.fullmatch(
            r"\s*#\[cfg\((?:any\(\)|test)\)\]\s*", lines[attribute_index]
        ):
            continue
        module_index = attribute_index + 1
        while module_index <= target and not lines[module_index].strip():
            module_index += 1
        if module_index > target or not re.search(
            r"\bmod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{", lines[module_index]
        ):
            continue
        depth = 0
        for line in lines[module_index : target + 1]:
            depth += line.count("{") - line.count("}")
        if depth > 0:
            return True
    return False


def require_complete_regex_evidence(
    regex_payload: Any, needle: str
) -> list[dict[str, Any]]:
    if not isinstance(regex_payload, dict):
        raise RuntimeError(f"Loctree regex payload for {needle} must be an object")
    matches = regex_payload.get("matches")
    if not isinstance(matches, dict):
        raise RuntimeError(f"Loctree regex matches for {needle} must be an object")
    occurrences = matches.get("occurrences")
    if not isinstance(occurrences, list) or any(
        not isinstance(occurrence, dict) for occurrence in occurrences
    ):
        raise RuntimeError(
            f"Loctree regex occurrences for {needle} must be a list of objects"
        )

    offset = matches.get("offset")
    if isinstance(offset, bool) or not isinstance(offset, int) or offset != 0:
        raise RuntimeError(
            f"Loctree regex evidence for {needle} has invalid offset: {offset!r}"
        )
    total = matches.get("total")
    if isinstance(total, bool) or not isinstance(total, int) or total < 0:
        raise RuntimeError(
            f"Loctree regex evidence for {needle} has invalid total: {total!r}"
        )
    if total != len(occurrences):
        raise RuntimeError(
            f"Loctree regex evidence for {needle} emitted {len(occurrences)} of {total} matches"
        )
    if matches.get("truncated") is not False:
        raise RuntimeError(
            f"Loctree regex evidence for {needle} is truncated or lacks truncation proof"
        )

    universe = matches.get("universe")
    if not isinstance(universe, dict) or universe.get("scan_complete") is not True:
        raise RuntimeError(
            f"Loctree regex evidence for {needle} lacks complete scanned-universe proof"
        )
    regex_trust = regex_payload.get("regex_trust")
    if not isinstance(regex_trust, dict):
        raise RuntimeError(
            f"Loctree regex evidence for {needle} lacks regex trust metadata"
        )
    untrusted = [
        key for key in REQUIRED_REGEX_TRUST_KEYS if regex_trust.get(key) is not True
    ]
    if untrusted:
        raise RuntimeError(
            f"Loctree regex evidence for {needle} lacks required trust: {untrusted}"
        )
    return occurrences


def residue_occurrences(
    regex_payload: dict[str, Any],
    exact_payload: dict[str, Any],
    needle: str,
    *,
    repo: Path | None = None,
) -> list[dict[str, Any]]:
    complete_occurrences = require_complete_regex_evidence(regex_payload, needle)
    exact_fail_gate = {
        occurrence_key(row) for row in forbidden_occurrences(exact_payload)
    }
    rows: list[dict[str, Any]] = []
    for occurrence in complete_occurrences:
        if occurrence_key(occurrence) in exact_fail_gate:
            continue
        matched_identifier = str(occurrence.get("matched_text", ""))
        if not matched_identifier:
            continue
        residue_class, reason = classify_substring_residue(
            occurrence,
            needle,
            disabled_cfg_any=inside_non_runtime_cfg_module(repo, occurrence),
        )
        rows.append(
            {
                "file": str(occurrence.get("file", "")),
                "line": int(occurrence.get("line", 0)),
                "matched_identifier": matched_identifier,
                "needle": needle,
                "relation": residue_relation(
                    matched_identifier, needle, str(occurrence.get("file", ""))
                ),
                "match_role": str(occurrence.get("match_role", "unknown")),
                "scope_classification": str(
                    occurrence.get("scope_classification", "unknown")
                ),
                "class": residue_class,
                "fail_gate": False,
                "review_required": residue_class == "unclassified_requires_review",
                "reason": reason,
            }
        )
    return sorted(
        rows,
        key=lambda row: (
            row["file"],
            row["line"],
            row["matched_identifier"],
            row["match_role"],
        ),
    )


def residue_summary(by_needle: dict[str, list[dict[str, Any]]]) -> dict[str, Any]:
    class_counts = {residue_class: 0 for residue_class in RESIDUE_CLASSES}
    needle_counts: dict[str, int] = {}
    for needle, rows in by_needle.items():
        needle_counts[needle] = len(rows)
        for row in rows:
            class_counts[str(row["class"])] += 1
    return {
        "evidence_complete": True,
        "query_count": len(by_needle),
        "complete_query_count": len(by_needle),
        "truncated_query_count": 0,
        "total_count": sum(needle_counts.values()),
        "unclassified_count": class_counts["unclassified_requires_review"],
        "review_required_count": sum(
            1 for rows in by_needle.values() for row in rows if row["review_required"]
        ),
        "class_counts": class_counts,
        "needle_counts": needle_counts,
    }


def validate_residue_shape(residue: Any, forbidden_symbols: Any) -> None:
    if not isinstance(residue, dict) or "summary" not in residue:
        raise RuntimeError("receipt residue_by_substring must contain summary")
    if (
        not isinstance(forbidden_symbols, list)
        or any(
            not isinstance(symbol, str) or not symbol for symbol in forbidden_symbols
        )
        or len(set(forbidden_symbols)) != len(forbidden_symbols)
    ):
        raise RuntimeError(f"invalid forbidden symbol query set: {forbidden_symbols}")
    by_needle = {key: value for key, value in residue.items() if key != "summary"}
    if set(by_needle) != set(forbidden_symbols) or len(by_needle) != len(
        forbidden_symbols
    ):
        raise RuntimeError(
            "residue query buckets do not equal the forbidden symbol query set: "
            f"expected {sorted(forbidden_symbols)}, observed {sorted(by_needle)}"
        )
    for needle, rows in by_needle.items():
        if not isinstance(needle, str) or not needle or not isinstance(rows, list):
            raise RuntimeError(f"invalid residue needle bucket: {needle!r}")
        for row in rows:
            if not isinstance(row, dict) or set(row) != RESIDUE_ROW_KEYS:
                raise RuntimeError(f"invalid residue row shape for {needle}: {row}")
            if row["needle"] != needle:
                raise RuntimeError(f"residue row needle mismatch for {needle}: {row}")
            if (
                not isinstance(row["file"], str)
                or not row["file"]
                or not isinstance(row["line"], int)
                or isinstance(row["line"], bool)
                or row["line"] < 1
                or not isinstance(row["matched_identifier"], str)
                or not row["matched_identifier"]
                or not isinstance(row["match_role"], str)
                or not row["match_role"]
                or not isinstance(row["scope_classification"], str)
                or not row["scope_classification"]
                or not isinstance(row["reason"], str)
                or not row["reason"]
            ):
                raise RuntimeError(f"invalid residue evidence fields for {needle}: {row}")
            if row["relation"] not in RESIDUE_RELATIONS:
                raise RuntimeError(f"invalid residue relation for {needle}: {row}")
            if row["class"] not in RESIDUE_CLASSES:
                raise RuntimeError(f"invalid residue class for {needle}: {row}")
            if row["fail_gate"] is not False:
                raise RuntimeError(f"residue row attempted to enter fail gate: {row}")
            expected_review = row["class"] == "unclassified_requires_review"
            if row["review_required"] is not expected_review:
                raise RuntimeError(f"residue review flag contradicts class: {row}")
    expected_summary = residue_summary(by_needle)
    if residue["summary"] != expected_summary:
        raise RuntimeError(
            f"residue summary mismatch: expected {expected_summary}, "
            f"observed {residue['summary']}"
        )


def build_residue_by_substring(
    verifier: StructuralVerifier, forbidden_symbols: list[str]
) -> dict[str, Any]:
    by_needle = {
        needle: residue_occurrences(
            verifier.substring_occurrences(needle),
            verifier.occurrences(needle),
            needle,
            repo=verifier.repo,
        )
        for needle in forbidden_symbols
    }
    return {**by_needle, "summary": residue_summary(by_needle)}


def definitions(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        occurrence
        for occurrence in production_occurrences(payload)
        if occurrence.get("match_role") == "definition"
    ]


def observed_files(
    payload: dict[str, Any], *, include_tests: bool = False
) -> list[str]:
    occurrences = forbidden_occurrences(payload) if include_tests else production_occurrences(payload)
    return sorted({str(hit.get("file")) for hit in occurrences if hit.get("file")})


def rust_source_files(repo: Path) -> list[Path]:
    files = list(repo.glob("*.rs"))
    for root_name in RUST_SOURCE_ROOTS:
        root = repo / root_name
        if root.is_dir():
            files.extend(root.rglob("*.rs"))
    return sorted(set(files))


def module_candidates(source: Path, module: str) -> list[Path]:
    direct = source.parent
    nested = source.parent / source.stem
    candidates = [
        direct / f"{module}.rs",
        direct / module / "mod.rs",
        nested / f"{module}.rs",
        nested / module / "mod.rs",
    ]
    return list(dict.fromkeys(candidates))


def unresolved_module_declarations(repo: Path) -> list[dict[str, Any]]:
    unresolved: list[dict[str, Any]] = []
    for source in rust_source_files(repo):
        pending_path: str | None = None
        for line_number, line in enumerate(source.read_text(errors="replace").splitlines(), 1):
            path_match = RUST_PATH_RE.match(line)
            if path_match:
                pending_path = path_match.group(1)
                continue
            if line.lstrip().startswith("#[") or not line.strip():
                continue
            module_match = RUST_MOD_RE.match(line)
            if not module_match:
                pending_path = None
                continue
            module = module_match.group(1)
            candidates = (
                [source.parent / pending_path]
                if pending_path is not None
                else module_candidates(source, module)
            )
            pending_path = None
            if any(candidate.is_file() for candidate in candidates):
                continue
            unresolved.append(
                {
                    "file": str(source.relative_to(repo)),
                    "line": line_number,
                    "module": module,
                    "candidates": [str(path.relative_to(repo)) for path in candidates],
                }
            )
    return unresolved


def read_canary(path: Path | None) -> tuple[list[dict[str, Any]], list[str] | str]:
    if path is None:
        return [], "NOT_PROVIDED"
    payload = load_json(path)
    rows = payload.get("rows", []) if isinstance(payload, dict) else payload
    if not isinstance(rows, list):
        return [], ["canary manifest rows must be a list"]
    valid = {"THRONE", "MECHANIC", "OBSERVER", "DELETE_FILE", "DELETE_SYMBOL"}
    unclassified = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict) or row.get("classification") not in valid:
            unclassified.append(str(row.get("id", index)) if isinstance(row, dict) else str(index))
    return rows, unclassified


def verify_stage(
    verifier: StructuralVerifier,
    manifest: dict[str, Any],
    stage: str,
    scope: str | None,
    canary_path: Path | None,
) -> tuple[dict[str, Any], bool]:
    stage_contract = manifest["stages"][stage]
    context = verifier.context()
    context_receipt = context.get("receipt", {})
    project = context.get("project", {})
    failures: list[str] = []

    if context_receipt.get("authority") != "fresh":
        failures.append(f"Loctree snapshot is not fresh: {context_receipt.get('authority')}")

    forbidden_symbols = list(
        dict.fromkeys(
            [
                *manifest.get("retired_reference_symbols", []),
                *stage_contract.get("required_absent", []),
            ]
        )
    )
    forbidden_hits: dict[str, list[str]] = {}
    for symbol in forbidden_symbols:
        files = observed_files(verifier.occurrences(symbol), include_tests=True)
        if files:
            forbidden_hits[symbol] = files
            failures.append(f"forbidden symbol {symbol} remains in {files}")
    residue_by_substring = build_residue_by_substring(verifier, forbidden_symbols)

    unresolved_modules = unresolved_module_declarations(verifier.repo)
    if unresolved_modules:
        failures.append(
            f"unresolved Rust module declarations remain: {unresolved_modules}"
        )

    observed_parts: dict[str, dict[str, Any]] = {}
    expected_owners: dict[str, str] = {}
    observed_owners: dict[str, list[str]] = {}
    required_present = stage_contract.get("required_present", [])
    required_unwired = stage_contract.get("required_unwired", [])
    if scope is not None:
        scoped_symbols = ASSEMBLY_SCOPE_SYMBOLS[scope]
        required_present = [part for part in required_present if part["symbol"] in scoped_symbols]
        required_unwired = [part for part in required_unwired if part["symbol"] in scoped_symbols]

    for part in required_present:
        symbol = part["symbol"]
        owner = part["owner"]
        cardinality = int(part.get("cardinality", 1))
        defs = definitions(verifier.occurrences(symbol))
        owners = sorted({str(hit["file"]) for hit in defs})
        owner_defs = [hit for hit in defs if hit.get("file") == owner]
        observed_parts[symbol] = {
            "definitions": len(defs),
            "owner_definitions": len(owner_defs),
            "owners": owners,
        }
        expected_owners[symbol] = owner
        observed_owners[symbol] = owners
        if len(defs) != cardinality or len(owner_defs) != cardinality:
            failures.append(
                f"part {symbol} expected {cardinality} definition(s) in {owner}; "
                f"observed {len(defs)} total in {owners}"
            )

    unwired_paths: dict[str, list[str]] = {}
    for part in required_unwired:
        symbol = part["symbol"]
        allowed = set(part.get("owner_files", []))
        files = [path for path in observed_files(verifier.occurrences(symbol)) if path not in allowed]
        if files:
            unwired_paths[symbol] = files
            failures.append(f"part {symbol} is wired before W2 in {files}")

    consumer_paths: dict[str, dict[str, Any]] = {}
    bypass_paths: dict[str, list[str]] = {}
    for edge in stage_contract.get("required_edges", []):
        symbol = edge["symbol"]
        files = observed_files(verifier.occurrences(symbol))
        required = sorted(edge.get("consumers", []))
        missing = [path for path in required if path not in files]
        consumer_paths[symbol] = {
            "required": required,
            "observed": files,
            "missing": missing,
        }
        if missing:
            failures.append(f"symbol {symbol} is missing required consumer paths {missing}")
        forbidden = sorted(set(files).intersection(edge.get("forbidden_consumers", [])))
        if forbidden:
            bypass_paths[symbol] = forbidden
            failures.append(f"symbol {symbol} reaches forbidden bypass paths {forbidden}")

    canary_rows, unclassified_canary = read_canary(canary_path)
    if stage_contract.get("require_canary_classification"):
        if unclassified_canary == "NOT_PROVIDED":
            failures.append("classified Canary manifest is required for demolished receipt")
        elif unclassified_canary:
            failures.append(f"unclassified Canary rows: {unclassified_canary}")

    dangling = [
        f"forbidden:{symbol}:{path}"
        for symbol, paths in sorted(forbidden_hits.items())
        for path in paths
    ]
    dangling.extend(
        f"module:{row['file']}:{row['line']}:{row['module']}"
        for row in unresolved_modules
    )
    if stage_contract.get("require_zero_dangling"):
        if dangling:
            failures.append(f"wired stage retains computed dangling references: {dangling}")

    expected_verdict = STAGE_VERDICTS[stage]
    conformant = not failures
    receipt = {
        "$schema": "tests/fixtures/acoustic_structure_receipt.schema.json",
        "schema": RECEIPT_SCHEMA,
        "stage": stage,
        "scope": scope,
        "verdict": expected_verdict if conformant else "STRUCTURALLY_NONCONFORMANT",
        "expected_verdict": expected_verdict,
        "conformant": conformant,
        "repo": context_receipt.get("root"),
        "branch": project.get("branch"),
        "head": context_receipt.get("head_full"),
        "dirty_fingerprint": context_receipt.get("dirty_fingerprint"),
        "loctree_version": context_receipt.get("binary_id"),
        "loctree_snapshot_fingerprint": context_receipt.get("snapshot_fingerprint"),
        "expected_parts": [part["symbol"] for part in required_present],
        "observed_parts": observed_parts,
        "expected_owners": expected_owners,
        "observed_owners": observed_owners,
        "forbidden_symbols": forbidden_symbols,
        "forbidden_hits": forbidden_hits,
        "residue_by_substring": residue_by_substring,
        "consumer_paths": consumer_paths,
        "bypass_paths": bypass_paths,
        "unwired_paths": unwired_paths,
        "dangling_references": dangling,
        "unresolved_module_declarations": unresolved_modules,
        "canary_rows_observed": len(canary_rows),
        "unclassified_canary_rows": unclassified_canary,
        "failures": failures,
        "assessment": {
            "build": "NOT_ASSESSED",
            "lint": "NOT_ASSESSED",
            "unit": "NOT_ASSESSED",
            "swift": "NOT_ASSESSED",
            "runtime": "NOT_ASSESSED",
        },
        "command_inventory": [" ".join(command) for command in verifier.command_inventory],
        "command_policy": {
            "allowed_executable": ALLOWED_EXECUTABLE,
            "all_commands_allowed": all(
                command and command[0] == ALLOWED_EXECUTABLE
                for command in verifier.command_inventory
            ),
        },
    }
    return receipt, conformant


def inventory() -> dict[str, Any]:
    return {
        "allowed_executable": ALLOWED_EXECUTABLE,
        "command_templates": [
            "loct context --json",
            "loct occurrences <literal-identifier> --json",
            "loct find --regex <substring-identifier-pattern> --json",
        ],
        "direct_static_checks": ["Rust mod declaration resolves to a source file"],
        "product_execution": "FORBIDDEN",
        "build_lint_unit_swift_runtime": "NOT_ASSESSED",
    }


def validate_receipt_shape(receipt: dict[str, Any]) -> None:
    missing = sorted(REQUIRED_RECEIPT_KEYS.difference(receipt))
    if missing:
        raise RuntimeError(f"receipt schema check missing keys: {missing}")
    if receipt.get("schema") != RECEIPT_SCHEMA:
        raise RuntimeError(f"receipt schema identity mismatch: {receipt.get('schema')}")
    validate_residue_shape(
        receipt.get("residue_by_substring"), receipt.get("forbidden_symbols")
    )
    stage = receipt.get("stage")
    scope = receipt.get("scope")
    if (stage == "assembled") != (scope in ASSEMBLY_SCOPES):
        raise RuntimeError(f"receipt stage/scope mismatch: stage={stage}, scope={scope}")
    inventory_rows = receipt.get("command_inventory")
    if not isinstance(inventory_rows, list) or not inventory_rows:
        raise RuntimeError("receipt command inventory must be a non-empty list")
    if not all(isinstance(row, str) and row.startswith("loct ") for row in inventory_rows):
        raise RuntimeError(f"receipt contains a non-Loctree command: {inventory_rows}")
    policy = receipt.get("command_policy", {})
    if policy != {"allowed_executable": ALLOWED_EXECUTABLE, "all_commands_allowed": True}:
        raise RuntimeError(f"receipt command policy is not fail-closed: {policy}")
    assessment = receipt.get("assessment", {})
    if set(assessment) != {"build", "lint", "unit", "swift", "runtime"} or any(
        value != "NOT_ASSESSED" for value in assessment.values()
    ):
        raise RuntimeError(f"structural receipt inferred a forbidden assessment: {assessment}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("stage", nargs="?", choices=sorted(STAGE_VERDICTS))
    parser.add_argument("scope", nargs="?", choices=sorted(ASSEMBLY_SCOPES))
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--manifest", type=Path, default=Path(DEFAULT_MANIFEST))
    parser.add_argument("--canary-manifest", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--inventory", action="store_true")
    args = parser.parse_args()

    if args.inventory:
        print(json.dumps(inventory(), indent=2, sort_keys=True))
        return 0
    if args.stage is None:
        parser.error("stage is required unless --inventory is used")
    if args.stage == "assembled" and args.scope is None:
        parser.error("assembled stage requires one scope")
    if args.stage != "assembled" and args.scope is not None:
        parser.error("scope is valid only for assembled stage")

    repo = args.repo.resolve()
    manifest_path = args.manifest if args.manifest.is_absolute() else repo / args.manifest
    manifest = load_json(manifest_path)
    verifier = StructuralVerifier(repo)
    try:
        receipt, conformant = verify_stage(
            verifier,
            manifest,
            args.stage,
            args.scope,
            args.canary_manifest,
        )
    except RuntimeError as error:
        print(f"structural verifier failed closed: {error}", file=sys.stderr)
        return 2

    try:
        validate_receipt_shape(receipt)
    except RuntimeError as error:
        print(f"structural verifier failed closed: {error}", file=sys.stderr)
        return 2

    rendered = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered)
    print(rendered, end="")
    return 0 if conformant else 1


if __name__ == "__main__":
    raise SystemExit(main())
