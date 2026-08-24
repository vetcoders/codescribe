#!/usr/bin/env python3
"""Emit Loctree-only structural receipts for the one-throne transplant.

This verifier deliberately knows no Cargo, compiler, Swift runner, application
launcher, or runtime probe.  Its only subprocess executable is `loct`; every
executed command is recorded in the receipt and checked before dispatch.
"""

from __future__ import annotations

import argparse
import json
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
REQUIRED_RECEIPT_KEYS = {
    "schema",
    "stage",
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
    "consumer_paths",
    "bypass_paths",
    "unwired_paths",
    "dangling_references",
    "canary_rows_observed",
    "unclassified_canary_rows",
    "failures",
    "assessment",
    "command_inventory",
    "command_policy",
}


@dataclass(frozen=True)
class LoctResult:
    command: list[str]
    payload: dict[str, Any]


class StructuralVerifier:
    def __init__(self, repo: Path) -> None:
        self.repo = repo
        self.command_inventory: list[list[str]] = []
        self._occurrences: dict[str, dict[str, Any]] = {}

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


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text())
    except FileNotFoundError as error:
        raise SystemExit(f"missing structural verifier input: {path}") from error
    except json.JSONDecodeError as error:
        raise SystemExit(f"invalid JSON in structural verifier input {path}: {error}") from error


def production_occurrences(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        occurrence
        for occurrence in payload.get("occurrences", [])
        if occurrence.get("scope_classification") in SOURCE_SCOPES
    ]


def definitions(payload: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        occurrence
        for occurrence in production_occurrences(payload)
        if occurrence.get("match_role") == "definition"
    ]


def observed_files(payload: dict[str, Any]) -> list[str]:
    return sorted({str(hit.get("file")) for hit in production_occurrences(payload) if hit.get("file")})


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


def read_dangling(path: Path | None) -> list[Any] | str:
    if path is None:
        return "NOT_PROVIDED"
    payload = load_json(path)
    rows = payload.get("dangling_references", []) if isinstance(payload, dict) else payload
    return rows if isinstance(rows, list) else ["dangling manifest must be a list"]


def verify_stage(
    verifier: StructuralVerifier,
    manifest: dict[str, Any],
    stage: str,
    canary_path: Path | None,
    dangling_path: Path | None,
) -> tuple[dict[str, Any], bool]:
    stage_contract = manifest["stages"][stage]
    context = verifier.context()
    context_receipt = context.get("receipt", {})
    project = context.get("project", {})
    failures: list[str] = []

    if context_receipt.get("authority") != "fresh":
        failures.append(f"Loctree snapshot is not fresh: {context_receipt.get('authority')}")

    forbidden_hits: dict[str, list[str]] = {}
    for symbol in stage_contract.get("required_absent", []):
        files = observed_files(verifier.occurrences(symbol))
        if files:
            forbidden_hits[symbol] = files
            failures.append(f"forbidden symbol {symbol} remains in {files}")

    observed_parts: dict[str, dict[str, Any]] = {}
    expected_owners: dict[str, str] = {}
    observed_owners: dict[str, list[str]] = {}
    for part in stage_contract.get("required_present", []):
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
    for part in stage_contract.get("required_unwired", []):
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

    dangling = read_dangling(dangling_path)
    if stage_contract.get("require_zero_dangling"):
        if dangling == "NOT_PROVIDED":
            failures.append("dangling-reference manifest is required for wired receipt")
        elif dangling:
            failures.append(f"wired stage retains dangling references: {dangling}")

    expected_verdict = STAGE_VERDICTS[stage]
    conformant = not failures
    receipt = {
        "$schema": "tests/fixtures/acoustic_structure_receipt.schema.json",
        "schema": RECEIPT_SCHEMA,
        "stage": stage,
        "verdict": expected_verdict if conformant else "STRUCTURALLY_NONCONFORMANT",
        "expected_verdict": expected_verdict,
        "conformant": conformant,
        "repo": context_receipt.get("root"),
        "branch": project.get("branch"),
        "head": context_receipt.get("head_full"),
        "dirty_fingerprint": context_receipt.get("dirty_fingerprint"),
        "loctree_version": context_receipt.get("binary_id"),
        "loctree_snapshot_fingerprint": context_receipt.get("snapshot_fingerprint"),
        "expected_parts": [part["symbol"] for part in stage_contract.get("required_present", [])],
        "observed_parts": observed_parts,
        "expected_owners": expected_owners,
        "observed_owners": observed_owners,
        "forbidden_symbols": stage_contract.get("required_absent", []),
        "forbidden_hits": forbidden_hits,
        "consumer_paths": consumer_paths,
        "bypass_paths": bypass_paths,
        "unwired_paths": unwired_paths,
        "dangling_references": dangling,
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
        ],
        "product_execution": "FORBIDDEN",
        "build_lint_unit_swift_runtime": "NOT_ASSESSED",
    }


def validate_receipt_shape(receipt: dict[str, Any]) -> None:
    missing = sorted(REQUIRED_RECEIPT_KEYS.difference(receipt))
    if missing:
        raise RuntimeError(f"receipt schema check missing keys: {missing}")
    if receipt.get("schema") != RECEIPT_SCHEMA:
        raise RuntimeError(f"receipt schema identity mismatch: {receipt.get('schema')}")
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
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--manifest", type=Path, default=Path(DEFAULT_MANIFEST))
    parser.add_argument("--canary-manifest", type=Path)
    parser.add_argument("--dangling-manifest", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--inventory", action="store_true")
    args = parser.parse_args()

    if args.inventory:
        print(json.dumps(inventory(), indent=2, sort_keys=True))
        return 0
    if args.stage is None:
        parser.error("stage is required unless --inventory is used")

    repo = args.repo.resolve()
    manifest_path = args.manifest if args.manifest.is_absolute() else repo / args.manifest
    manifest = load_json(manifest_path)
    verifier = StructuralVerifier(repo)
    try:
        receipt, conformant = verify_stage(
            verifier,
            manifest,
            args.stage,
            args.canary_manifest,
            args.dangling_manifest,
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
