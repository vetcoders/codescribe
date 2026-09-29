#!/usr/bin/env python3
"""Emit structure-only receipts for the one-throne transplant.

This verifier never compiles or executes the product. Loctree supplies source
evidence; one exact offline/locked Cargo command builds and runs the neutral
syn instrument. Every command is recorded and checked before dispatch. A
small local Rust-module resolution pass complements Loctree so deleted files
cannot remain referenced by `mod name;` declarations.
"""

from __future__ import annotations

import argparse
import ast
import json
import hashlib
import os
import tomllib
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


RECEIPT_SCHEMA = "codescribe.acoustic-structure-receipt.v3"
DEFAULT_MANIFEST = "tests/fixtures/acoustic_throne_stages.json"
ALLOWED_EXECUTABLE = "loct"
AST_COMMAND = (
    "cargo", "run", "--offline", "--locked", "--package",
    "codescribe-structural-ast", "--bin", "codescribe-structural-ast", "--quiet",
)
AST_IDENTITY = "codescribe-structural-ast/0.1.0;syn=2.0.118;grammar=2"
AST_BODIES = {
    "paste_text_from_overlay": "app/controller/mod.rs",
    "execute_clipboard_paste": "app/controller/mod.rs",
    "stop": "core/audio/streaming_recorder.rs",
    "complete_stop": "core/audio/streaming_recorder.rs",
    "terminal_finality": "core/pipeline/acoustic_ledger.rs",
    "has_no_capture_facts": "core/pipeline/acoustic_ledger.rs",
    "matches_refused_document": "app/presentation/transcript_bus.rs",
    "process_terminal_stop_error": "app/controller/mod.rs",
}
# The shipped default when no lease is supplied. A fleet worktree that owns a
# shared target must be able to state its own budget instead of having this
# value forced on it; nothing else about the child environment is negotiable.
AST_DEFAULT_BUILD_JOBS = 4
AST_MAX_BUILD_JOBS = 1024
# Decimal, no sign, no leading zero, no separators. `int()` alone would accept
# "+2", " 2" and non-ASCII digits, so the spelling is validated before parsing.
AST_BUILD_JOBS_PATTERN = re.compile(r"[1-9][0-9]*")
AST_INCREMENTAL_VALUES = {"0", "1"}


def resolve_ast_target(repo: Path) -> Path:
    """Select a target without creating it or following target-path symlinks."""
    try:
        repo = repo.resolve(strict=True)
        override = os.environ.get("CARGO_TARGET_DIR")
        spelling = override if override is not None else str(repo / "target")
        if (
            not spelling or any(part != part.strip() for part in spelling.split("/"))
            or any(ord(char) < 32 or ord(char) == 127 for char in spelling)
            or any(char in spelling for char in ("~", "$", "\\", ":"))
        ):
            raise RuntimeError("neutral AST target is empty or malformed")
        candidate = Path(override) if override is not None else repo / "target"
        if not candidate.is_absolute():
            candidate = repo / candidate
        # Inspect the lexical path before resolving '..': a redirect must not
        # disappear during normalization (including dangling links and loops).
        for component in (*reversed(candidate.parents), candidate):
            if component.is_symlink():
                raise RuntimeError("neutral AST target contains a symlink")
            if component.exists() and not component.is_dir():
                raise RuntimeError("neutral AST target contains a non-directory")
        target = candidate.resolve(strict=override is not None)
        home = Path.home().resolve()
        if target in {Path(target.anchor), home, repo} or target in repo.parents or target in home.parents:
            raise RuntimeError("neutral AST target is a root, home, repository or broad ancestor")
        if target.exists() and not target.is_dir():
            raise RuntimeError("neutral AST target is not a directory")
        return target
    except (OSError, ValueError) as error:
        raise RuntimeError(f"neutral AST target unavailable: {error}") from error


def resolve_ast_build_policy() -> tuple[int, int | None]:
    """Admit an explicitly supplied compiler lease; refuse malformed spellings.

    Absent values keep the shipped default: `jobs=4` and Cargo's own
    incremental policy. A supplied `CARGO_BUILD_JOBS` must be a positive
    decimal integer and `CARGO_INCREMENTAL` exactly `0` or `1`. Both are
    validated here, before any filesystem work or child process, so a
    malformed lease can never reach Cargo. Returning `None` for incremental
    records "not supplied" rather than inventing a policy the caller never
    stated.
    """
    supplied_jobs = os.environ.get("CARGO_BUILD_JOBS")
    if supplied_jobs is None:
        jobs = AST_DEFAULT_BUILD_JOBS
    elif AST_BUILD_JOBS_PATTERN.fullmatch(supplied_jobs) and int(supplied_jobs) <= AST_MAX_BUILD_JOBS:
        jobs = int(supplied_jobs)
    else:
        raise RuntimeError(
            f"neutral AST CARGO_BUILD_JOBS is not a validated positive integer: {supplied_jobs!r}"
        )
    supplied_incremental = os.environ.get("CARGO_INCREMENTAL")
    if supplied_incremental is not None and supplied_incremental not in AST_INCREMENTAL_VALUES:
        raise RuntimeError(
            f"neutral AST CARGO_INCREMENTAL must be 0 or 1 when supplied: {supplied_incremental!r}"
        )
    incremental = None if supplied_incremental is None else int(supplied_incremental)
    return jobs, incremental


def command_allowed(command: list[str] | tuple[str, ...]) -> bool:
    return bool(command) and (command[0] == ALLOWED_EXECUTABLE or tuple(command) == AST_COMMAND)


def ast_tool_digest(repo: Path) -> str:
    """Authenticate the closed neutral package before Cargo can execute it."""
    root = repo / "tools/structural-ast"
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    expected = {
        "package": {"name": "codescribe-structural-ast", "version": "0.1.0",
                    "edition": "2024", "publish": False},
        "dependencies": {
            "syn": {"version": "=2.0.118", "features": ["full", "extra-traits", "visit"]},
            "serde": {"workspace": True}, "serde_json": {"workspace": True},
        },
    }
    if manifest != expected or (root / "build.rs").exists():
        raise RuntimeError("neutral AST package dependency/build contract changed")
    # Cargo auto-discovers targets; prohibit injected bins/examples/build scripts.
    if any((root / name).exists() for name in ("src/bin", "examples", ".cargo")):
        raise RuntimeError("neutral AST package has unadmitted executable targets")
    files = [root / "Cargo.toml", *sorted((root / "src").rglob("*"))]
    if {path.relative_to(root).as_posix() for path in files} != {
        "Cargo.toml", "src/lib.rs", "src/main.rs", "src/productions.rs", "src/finality.rs",
    } or any(path.is_symlink() or not path.is_file() for path in files):
        raise RuntimeError("neutral AST source inventory changed")
    workspace = tomllib.loads((repo / "Cargo.toml").read_text())["workspace"]["dependencies"]
    if workspace["serde"] != {"version": "1", "features": ["derive"]} or workspace["serde_json"] != "1":
        raise RuntimeError("neutral AST inherited dependencies changed")
    digest = hashlib.sha256()
    for path in [repo / "Cargo.toml", repo / "Cargo.lock", *files]:
        digest.update(path.relative_to(repo).as_posix().encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def run_ast_json(repo: Path, command: list[str], payload: dict[str, Any]) -> dict[str, Any]:
    if tuple(command) != AST_COMMAND:
        raise RuntimeError(f"refused non-neutral AST command: {command}")
    # Validate the lease first: a malformed budget must be refused before the
    # target is resolved and long before a child process could exist.
    jobs, incremental = resolve_ast_build_policy()
    target = str(resolve_ast_target(repo))
    before = ast_tool_digest(repo)
    # Do not inherit compiler wrappers, Cargo overrides, injected Rust flags or
    # dynamic-library preload settings. Only the validated target and the
    # validated build lease are configurable.
    env = {key: os.environ[key] for key in ("PATH", "HOME", "TMPDIR", "SYSTEMROOT") if key in os.environ}
    env.update(CARGO_TARGET_DIR=target, CARGO_BUILD_JOBS=str(jobs))
    if incremental is not None:
        env["CARGO_INCREMENTAL"] = str(incremental)
    serialized = json.dumps(payload)
    try:
        completed = subprocess.run(command, cwd=repo, env=env, input=serialized,
                                   check=False, capture_output=True, text=True, timeout=180)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"neutral AST unavailable: {error}") from error
    if completed.returncode != 0:
        raise RuntimeError(f"neutral AST failed ({completed.returncode}): {completed.stderr.strip()}")
    if before != ast_tool_digest(repo):
        raise RuntimeError("neutral AST sources changed during invocation")
    try:
        evidence = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError("neutral AST returned malformed JSON") from error
    if not isinstance(evidence, dict) or evidence.get("identity") != AST_IDENTITY or evidence.get("schema") != "codescribe.structural-ast-evidence.v1":
        raise RuntimeError("neutral AST identity/schema mismatch")
    contracts = evidence.get("contracts")
    if (type(evidence.get("accepted")) is not bool or not isinstance(evidence.get("failures"), list)
        or not isinstance(contracts, list) or len(contracts) != len(AST_BODIES)
        or any(not isinstance(row, dict) for row in contracts)
        or {row.get("symbol") for row in contracts} != set(AST_BODIES)):
        raise RuntimeError("neutral AST malformed contract results")
    for row in contracts:
        if (type(row.get("accepted")) is not bool or not isinstance(row.get("failures"), list)
            or not isinstance(row.get("events"), list)
            or any(not isinstance(item, str) for item in row["events"] + row["failures"])
            or row["accepted"] != (not row["failures"])):
            raise RuntimeError("neutral AST contradictory contract result")
    if evidence["accepted"] != (not evidence["failures"] and all(row["accepted"] for row in contracts)):
        raise RuntimeError("neutral AST contradictory overall result")
    evidence["invocation"] = {"command": command, "cwd": str(repo.resolve()),
        "source_sha256": before, "input_sha256": hashlib.sha256(serialized.encode()).hexdigest(),
        "build_policy": "cargo-run-current-sources-offline-locked", "target_dir": target,
        "jobs": jobs, "incremental": incremental}
    return evidence


def structural_ast_evidence(verifier: StructuralVerifier) -> dict[str, Any]:
    cached = getattr(verifier, "_structural_ast_evidence", None)
    if cached is not None:
        return cached
    bodies = []
    for symbol, file in AST_BODIES.items():
        rows = corridor_body_rows(verifier.body(symbol, file), symbol=symbol, file=file, signature_contains=None)
        if len(rows) != 1:
            raise RuntimeError(f"neutral AST requires one complete body for {file}::{symbol}")
        bodies.append(rows[0])
    command = list(AST_COMMAND)
    verifier.command_inventory.append(command)
    imports = []
    for file, literal in (
        ("app/controller/mod.rs", "use tracing::{debug, error, info, warn};"),
        ("core/audio/streaming_recorder.rs", "use tracing::{debug, info, warn};"),
    ):
        rows = require_complete_literal_evidence(verifier.literal_occurrences(literal), literal)
        matching = [row for row in rows if row.get("file") == file
                    and row.get("match_role") == "import"
                    and row.get("scope_classification") == "production"]
        if len(matching) != 1:
            raise RuntimeError(f"neutral AST observational macro import not authenticated: {file}")
        imports.extend(matching)
    evidence = run_ast_json(verifier.repo, command, {"schema": "codescribe.structural-ast-input.v1", "bodies": bodies})
    evidence["macro_imports"] = imports
    verifier._structural_ast_evidence = evidence
    return evidence


STAGE_VERDICTS = {
    "demolished": "OLD_AUTHORITY_REMOVED",
    "assembled": "STRUCTURALLY_COMPLETE_NOT_WIRED",
    "wired": "STRUCTURALLY_WIRED",
}
SOURCE_SCOPES = {"production", "generated"}
FORBIDDEN_SCOPES = SOURCE_SCOPES | {"test"}
NON_CODE_ROLES = {"comment", "string_literal"}
RUST_MODULE_DECLARATION_PATTERN = (
    r"(?m)^[[:space:]]*(?:pub(?:\([^)]*\))?[[:space:]]+)?"
    r"mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*;"
)
RUST_PATH_ATTRIBUTE_PATTERN = r'#\[path[[:space:]]*=[[:space:]]*"[^"]+"\]'
RUST_TEST_ATTRIBUTE_PATTERN = r"#\[(?:[A-Za-z0-9_]+::)?test\]"
STANDARD_MODULE_SOURCE_NAMES = {"lib.rs", "main.rs", "mod.rs"}
LOCTREE_MODULE_INVENTORY_QUERY = (
    "[.files[] | {path, imports: [.imports[]? | "
    "select(.is_mod_declaration == true) | "
    "{line, source, source_raw, resolved_path, symbols}]}]"
)
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
    "forbidden_literal_hits",
    "residue_by_substring",
    "consumer_paths",
    "corridor_paths",
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
CORRIDOR_DEAD_CODE_MARKERS = ("if false", "if(false)", "cfg!", "stringify!", "#if")


@dataclass(frozen=True)
class LoctResult:
    command: list[str]
    payload: Any


class StructuralVerifier:
    def __init__(self, repo: Path) -> None:
        self.repo = repo
        self.command_inventory: list[list[str]] = []
        self._occurrences: dict[str, dict[str, Any]] = {}
        self._substring_occurrences: dict[str, dict[str, Any]] = {}
        self._literal_occurrences: dict[str, dict[str, Any]] = {}
        self._bodies: dict[tuple[str, str], dict[str, Any]] = {}

    def _run_loct_json(self, command: list[str]) -> LoctResult:
        if command[0] != ALLOWED_EXECUTABLE:
            raise RuntimeError(f"structural verifier refused non-Loctree command: {command}")
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

    def run_loct(self, *args: str) -> LoctResult:
        command = [ALLOWED_EXECUTABLE, *args]
        if "--json" not in command:
            command.append("--json")
        return self._run_loct_json(command)

    def run_loct_query(self, expression: str) -> LoctResult:
        """Run a Loctree jq expression whose stdout is already JSON."""
        return self._run_loct_json([ALLOWED_EXECUTABLE, expression])

    def context(self) -> dict[str, Any]:
        # Rebuild source positions before evidence collection and at the final
        # coherence check; a cached definition can survive a moved Swift body.
        return self.run_loct("context", "--full", "--no-aicx", "--fresh").payload

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

    def literal_occurrences(self, literal: str) -> dict[str, Any]:
        if literal not in self._literal_occurrences:
            self._literal_occurrences[literal] = self.run_loct(
                "find", "--literal", literal, "--all"
            ).payload
        return self._literal_occurrences[literal]

    def body(self, symbol: str, file: str) -> dict[str, Any]:
        key = (symbol, file)
        if key not in self._bodies:
            self._bodies[key] = self.run_loct(
                "body", symbol, "--file", file, "--line-cap", "1000"
            ).payload
        return self._bodies[key]


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
    if (file in {"tools/structural-ast/src/lib.rs", "tools/structural-ast/src/productions.rs"}
        and matched_identifier in {"OverlayPasteResult", "OverlayPasteDelivery"}):
        return "verifier_self_literal", "neutral syn grammar type token; package has no product dependency"
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


def direct_test_attribute_lines(regex_payload: Any) -> set[tuple[str, int]]:
    """Return only test attributes proven by a complete Loctree regex receipt."""
    return {
        (str(row.get("file", "")), int(row.get("line", 0)))
        for row in require_complete_regex_evidence(
            regex_payload,
            "direct Rust test attribute",
            expected_query=RUST_TEST_ATTRIBUTE_PATTERN,
        )
        if str(row.get("file", "")).endswith(".rs")
        and row.get("match_role") not in NON_CODE_ROLES
    }


def require_complete_regex_evidence(
    regex_payload: Any,
    needle: str,
    *,
    expected_query: str | None = None,
) -> list[dict[str, Any]]:
    if not isinstance(regex_payload, dict):
        raise RuntimeError(f"Loctree regex payload for {needle} must be an object")
    if regex_payload.get("mode") != "regex":
        raise RuntimeError(f"Loctree evidence for {needle} is not in regex mode")
    query = regex_payload.get("query")
    if not isinstance(query, str) or (expected_query is not None and query != expected_query):
        raise RuntimeError(
            f"Loctree regex query for {needle} is not the requested query: {query!r}"
        )
    matches = regex_payload.get("matches")
    if not isinstance(matches, dict):
        raise RuntimeError(f"Loctree regex matches for {needle} must be an object")
    if matches.get("query") != query:
        raise RuntimeError(f"Loctree regex query metadata for {needle} is incoherent")
    if matches.get("match_mode") != "regex" or matches.get("source") != "regex":
        raise RuntimeError(f"Loctree evidence source for {needle} is not regex")
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
    emitted = matches.get("emitted")
    if isinstance(emitted, bool) or not isinstance(emitted, int) or emitted < 0:
        raise RuntimeError(
            f"Loctree regex evidence for {needle} has invalid emitted count: {emitted!r}"
        )
    if total != emitted or emitted != len(occurrences):
        raise RuntimeError(
            f"Loctree regex evidence for {needle} emitted {emitted}/{total} "
            f"with {len(occurrences)} rows"
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
    indexed_files = universe.get("indexed_files")
    scanned_files = universe.get("scanned_files")
    if (
        isinstance(indexed_files, bool)
        or not isinstance(indexed_files, int)
        or indexed_files <= 0
        or isinstance(scanned_files, bool)
        or not isinstance(scanned_files, int)
        or scanned_files != indexed_files
    ):
        raise RuntimeError(
            f"Loctree regex universe for {needle} is incomplete: "
            f"indexed={indexed_files!r}, scanned={scanned_files!r}"
        )
    scope = matches.get("scope")
    if (
        not isinstance(scope, dict)
        or scope.get("files_in_universe") != indexed_files
        or scope.get("files_scanned") != indexed_files
    ):
        raise RuntimeError(f"Loctree regex scope for {needle} disagrees with its universe")
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
    identities: set[tuple[str, int, int, str]] = set()
    for occurrence in occurrences:
        file = occurrence.get("file")
        line = occurrence.get("line")
        column = occurrence.get("column")
        matched_text = occurrence.get("matched_text")
        if (
            not isinstance(file, str)
            or not file
            or Path(file).is_absolute()
            or isinstance(line, bool)
            or not isinstance(line, int)
            or line <= 0
            or isinstance(column, bool)
            or not isinstance(column, int)
            or column <= 0
            or not isinstance(matched_text, str)
            or not matched_text
        ):
            raise RuntimeError(f"Loctree regex row for {needle} is malformed: {occurrence}")
        identity = (file, line, column, matched_text)
        if identity in identities:
            raise RuntimeError(f"Loctree regex rows for {needle} contain duplicate {identity}")
        identities.add(identity)
    return occurrences


def residue_occurrences(
    regex_payload: dict[str, Any],
    exact_payload: dict[str, Any],
    needle: str,
    *,
    direct_test_functions: set[tuple[str, int, str]] | None = None,
) -> list[dict[str, Any]]:
    complete_occurrences = require_complete_regex_evidence(
        regex_payload,
        needle,
        expected_query=substring_identifier_pattern(needle),
    )
    exact_fail_gate = {
        occurrence_key(row) for row in forbidden_occurrences(exact_payload)
    }
    proven_test_functions = direct_test_functions or set()
    rows: list[dict[str, Any]] = []
    for occurrence in complete_occurrences:
        if occurrence_key(occurrence) in exact_fail_gate:
            continue
        matched_identifier = str(occurrence.get("matched_text", ""))
        if not matched_identifier:
            continue
        is_direct_test_function = (
            str(occurrence.get("file", "")),
            int(occurrence.get("line", 0)),
            str(occurrence.get("matched_text", "")),
        ) in proven_test_functions
        residue_class, reason = classify_substring_residue(
            occurrence,
            needle,
            # Keep the legacy keyword used by frozen wrapper tests, but feed it
            # only a complete Loctree receipt. Raw Rust source is not anatomy.
            disabled_cfg_any=is_direct_test_function,
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


def exact_function_receipt(
    payload: Any,
    *,
    file: str,
    line: int,
    identifier: str,
) -> bool:
    if not isinstance(payload, dict):
        raise RuntimeError(f"Loctree exact payload for {identifier} must be an object")
    if (
        payload.get("query") != identifier
        or payload.get("query_kind") != "identifier"
        or payload.get("match_mode") != "identifier_boundary"
        or payload.get("source") != "literal"
    ):
        raise RuntimeError(f"Loctree exact payload for {identifier} has wrong query mode")
    occurrences = payload.get("occurrences")
    total = payload.get("total")
    emitted = payload.get("emitted")
    offset = payload.get("offset")
    if (
        not isinstance(occurrences, list)
        or any(not isinstance(row, dict) for row in occurrences)
        or isinstance(total, bool)
        or not isinstance(total, int)
        or isinstance(emitted, bool)
        or not isinstance(emitted, int)
        or total != emitted
        or emitted != len(occurrences)
        or isinstance(offset, bool)
        or not isinstance(offset, int)
        or offset != 0
        or payload.get("truncated") is not False
    ):
        raise RuntimeError(f"Loctree exact payload for {identifier} is incomplete")
    universe = payload.get("universe")
    if (
        not isinstance(universe, dict)
        or universe.get("scan_complete") is not True
        or universe.get("scanned_files") != universe.get("indexed_files")
    ):
        raise RuntimeError(f"Loctree exact payload for {identifier} lacks full universe")
    matching = [
        row
        for row in occurrences
        if row.get("file") == file
        and row.get("line") == line
        and row.get("matched_text") == identifier
    ]
    if len(matching) != 1:
        return False
    enclosing = matching[0].get("enclosing_symbol")
    return (
        isinstance(enclosing, dict)
        and enclosing.get("name") == identifier
        and enclosing.get("file") == file
        and enclosing.get("line") == line
        and enclosing.get("kind") == "function"
    )


def direct_test_function_keys(
    verifier: StructuralVerifier,
    regex_payloads: dict[str, Any],
    test_attribute_lines: set[tuple[str, int]],
) -> set[tuple[str, int, str]]:
    candidates: set[tuple[str, int, str]] = set()
    for needle, payload in regex_payloads.items():
        rows = require_complete_regex_evidence(
            payload,
            needle,
            expected_query=substring_identifier_pattern(needle),
        )
        for row in rows:
            file = str(row.get("file", ""))
            line = int(row.get("line", 0))
            identifier = str(row.get("matched_text", ""))
            if (file, line - 1) not in test_attribute_lines:
                continue
            if exact_function_receipt(
                verifier.occurrences(identifier),
                file=file,
                line=line,
                identifier=identifier,
            ):
                candidates.add((file, line, identifier))
    return candidates


def build_residue_by_substring(
    verifier: StructuralVerifier, forbidden_symbols: list[str]
) -> dict[str, Any]:
    test_attribute_lines = direct_test_attribute_lines(
        verifier.run_loct(
            "find", "--regex", RUST_TEST_ATTRIBUTE_PATTERN, "--all"
        ).payload
    )
    regex_payloads = {
        needle: verifier.substring_occurrences(needle) for needle in forbidden_symbols
    }
    exact_payloads = {
        needle: verifier.occurrences(needle) for needle in forbidden_symbols
    }
    proven_test_functions = direct_test_function_keys(
        verifier, regex_payloads, test_attribute_lines
    )
    by_needle = {
        needle: residue_occurrences(
            regex_payloads[needle],
            exact_payloads[needle],
            needle,
            direct_test_functions=proven_test_functions,
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


def require_complete_literal_evidence(
    payload: Any, literal: str
) -> list[dict[str, Any]]:
    if not isinstance(payload, dict) or payload.get("mode") != "literal":
        raise RuntimeError(f"Loctree evidence for {literal!r} is not in literal mode")
    if payload.get("query") != literal:
        raise RuntimeError(f"Loctree literal query for {literal!r} is incoherent")
    matches = payload.get("matches")
    if (
        not isinstance(matches, dict)
        or matches.get("query") != literal
        or matches.get("source") != "literal"
    ):
        raise RuntimeError(f"Loctree literal matches for {literal!r} are malformed")
    occurrences = matches.get("occurrences")
    if not isinstance(occurrences, list) or any(
        not isinstance(row, dict) for row in occurrences
    ):
        raise RuntimeError(f"Loctree literal rows for {literal!r} are malformed")
    offset = matches.get("offset")
    total = matches.get("total")
    emitted = matches.get("emitted")
    if (
        isinstance(offset, bool)
        or offset != 0
        or isinstance(total, bool)
        or not isinstance(total, int)
        or total < 0
        or isinstance(emitted, bool)
        or not isinstance(emitted, int)
        or emitted != total
        or emitted != len(occurrences)
        or matches.get("truncated") is not False
    ):
        raise RuntimeError(
            f"Loctree literal evidence for {literal!r} is incomplete: "
            f"offset={offset!r}, emitted={emitted!r}, total={total!r}, "
            f"rows={len(occurrences)}"
        )
    universe = matches.get("universe")
    indexed_files = universe.get("indexed_files") if isinstance(universe, dict) else None
    scanned_files = universe.get("scanned_files") if isinstance(universe, dict) else None
    if (
        not isinstance(universe, dict)
        or universe.get("scan_complete") is not True
        or isinstance(indexed_files, bool)
        or not isinstance(indexed_files, int)
        or indexed_files <= 0
        or isinstance(scanned_files, bool)
        or scanned_files != indexed_files
    ):
        raise RuntimeError(f"Loctree literal universe for {literal!r} is incomplete")
    trust = payload.get("literal_trust")
    required_trust = (
        "absence_trustworthy_for_scanned",
        "file_scope_resolved",
        "matched_as_exact_string",
    )
    if not isinstance(trust, dict) or any(trust.get(key) is not True for key in required_trust):
        raise RuntimeError(f"Loctree literal evidence for {literal!r} lacks trust metadata")
    if trust.get("multi_literal") is not False:
        raise RuntimeError(f"Loctree literal evidence for {literal!r} is multi-literal")
    for row in occurrences:
        file = row.get("file")
        line = row.get("line")
        column = row.get("column")
        if (
            not isinstance(file, str)
            or not file
            or Path(file).is_absolute()
            or isinstance(line, bool)
            or not isinstance(line, int)
            or line <= 0
            or isinstance(column, bool)
            or not isinstance(column, int)
            or column <= 0
            or row.get("matched_text") != literal
        ):
            raise RuntimeError(f"Loctree literal row for {literal!r} is malformed: {row}")
    return occurrences


def verify_forbidden_executable_literals(
    verifier: StructuralVerifier, literals: Any
) -> tuple[dict[str, list[str]], list[str]]:
    if literals is None:
        return {}, []
    if not isinstance(literals, list) or any(
        not isinstance(literal, str) or not literal for literal in literals
    ):
        raise RuntimeError("forbidden_executable_literals must be non-empty strings")
    hits: dict[str, list[str]] = {}
    failures: list[str] = []
    for literal in literals:
        rows = production_occurrences(
            {
                "occurrences": require_complete_literal_evidence(
                    verifier.literal_occurrences(literal), literal
                )
            }
        )
        files = sorted({str(row["file"]) for row in rows})
        hits[literal] = files
        if files:
            failures.append(
                f"forbidden executable literal {literal!r} remains in {files}"
            )
    return hits, failures


def code_without_comments_or_strings(source: str) -> str:
    """Return code tokens while refusing comments and string literals as evidence."""
    rendered: list[str] = []
    index = 0
    state = "code"
    block_depth = 0
    while index < len(source):
        char = source[index]
        next_char = source[index + 1] if index + 1 < len(source) else ""
        if state == "code":
            if char == "/" and next_char == "/":
                rendered.extend("  ")
                index += 2
                state = "line_comment"
                continue
            if char == "/" and next_char == "*":
                rendered.extend("  ")
                index += 2
                state = "block_comment"
                block_depth = 1
                continue
            if char == '"':
                rendered.append(" ")
                index += 1
                state = "string"
                continue
            rendered.append(char)
            index += 1
            continue
        if state == "line_comment":
            rendered.append("\n" if char == "\n" else " ")
            index += 1
            if char == "\n":
                state = "code"
            continue
        if state == "block_comment":
            if char == "/" and next_char == "*":
                rendered.extend("  ")
                index += 2
                block_depth += 1
                continue
            if char == "*" and next_char == "/":
                rendered.extend("  ")
                index += 2
                block_depth -= 1
                if block_depth == 0:
                    state = "code"
                continue
            rendered.append("\n" if char == "\n" else " ")
            index += 1
            continue
        if state == "string":
            if char == "\\" and next_char:
                rendered.extend("  ")
                index += 2
                continue
            rendered.append("\n" if char == "\n" else " ")
            index += 1
            if char == '"':
                state = "code"
            continue
    if state in {"block_comment", "string"}:
        raise RuntimeError(f"unterminated {state} in Loctree body evidence")
    return collapse_code_whitespace("".join(rendered))


def collapse_code_whitespace(source: str) -> str:
    """Drop layout whitespace while preserving identifier-token boundaries."""
    collapsed: list[str] = []
    index = 0
    while index < len(source):
        char = source[index]
        if not char.isspace():
            collapsed.append(char)
            index += 1
            continue
        next_index = index + 1
        while next_index < len(source) and source[next_index].isspace():
            next_index += 1
        previous = collapsed[-1] if collapsed else ""
        following = source[next_index] if next_index < len(source) else ""
        if (
            (previous.isalnum() or previous == "_")
            and (following.isalnum() or following == "_")
        ):
            collapsed.append(" ")
        index = next_index
    return "".join(collapsed)


def truncating_integer_quotient(left: int, right: int) -> int | None:
    """Divide integers with the truncation-toward-zero used by Rust and Swift."""
    if right == 0:
        return None
    magnitude = abs(left) // abs(right)
    return -magnitude if (left < 0) != (right < 0) else magnitude


def constant_expression_value(expression: str) -> bool | int | None:
    """Evaluate an allowlisted constant expression without executing source code."""
    translated = re.sub(
        r"(?<=\d)_?(?:u|i)(?:8|16|32|64|128|size)\b",
        "",
        expression,
    )
    translated = re.sub(r"\btrue\b", "True", translated)
    translated = re.sub(r"\bfalse\b", "False", translated)
    translated = translated.replace("&&", " and ").replace("||", " or ")
    translated = re.sub(r"!(?!=)", " not ", translated).strip()
    try:
        node = ast.parse(translated, mode="eval").body
    except (SyntaxError, ValueError):
        return None

    def evaluate(candidate: ast.AST) -> bool | int | None:
        if isinstance(candidate, ast.Constant) and type(candidate.value) in {bool, int}:
            return candidate.value
        if isinstance(candidate, ast.UnaryOp):
            operand = evaluate(candidate.operand)
            if isinstance(candidate.op, ast.Not) and type(operand) is bool:
                return not operand
            if type(operand) is int and isinstance(candidate.op, ast.USub):
                return -operand
            if type(operand) is int and isinstance(candidate.op, ast.UAdd):
                return operand
            return None
        if isinstance(candidate, ast.BinOp):
            left = evaluate(candidate.left)
            right = evaluate(candidate.right)
            if type(left) is not int or type(right) is not int:
                return None
            if isinstance(candidate.op, ast.Add):
                return left + right
            if isinstance(candidate.op, ast.Sub):
                return left - right
            if isinstance(candidate.op, ast.Mult):
                return left * right
            if isinstance(candidate.op, (ast.Div, ast.Mod)):
                quotient = truncating_integer_quotient(left, right)
                if quotient is None:
                    return None
                if isinstance(candidate.op, ast.Div):
                    return quotient
                return left - quotient * right
            return None
        if isinstance(candidate, ast.BoolOp):
            values = [evaluate(value) for value in candidate.values]
            if any(type(value) is not bool for value in values):
                return None
            if isinstance(candidate.op, ast.And):
                return all(values)
            if isinstance(candidate.op, ast.Or):
                return any(values)
            return None
        if isinstance(candidate, ast.Compare) and len(candidate.ops) == 1:
            left = evaluate(candidate.left)
            right = evaluate(candidate.comparators[0])
            if type(left) is not type(right) or type(left) not in {bool, int}:
                return None
            operator = candidate.ops[0]
            if isinstance(operator, ast.Eq):
                return left == right
            if isinstance(operator, ast.NotEq):
                return left != right
            if type(left) is int and isinstance(operator, ast.Lt):
                return left < right
            if type(left) is int and isinstance(operator, ast.LtE):
                return left <= right
            if type(left) is int and isinstance(operator, ast.Gt):
                return left > right
            if type(left) is int and isinstance(operator, ast.GtE):
                return left >= right
        return None

    return evaluate(node)


def constant_false_predicate(condition: str) -> bool:
    """Recognize a bounded Rust/Swift predicate that is provably false."""
    return constant_expression_value(condition) is False


def corridor_false_block_ranges(code: str) -> list[tuple[int, int, str]]:
    """Return block spans whose `if` or `while` predicate is provably false."""
    open_blocks: list[tuple[int, str | None]] = []
    false_ranges: list[tuple[int, int, str]] = []
    for index, char in enumerate(code):
        if char == "{":
            boundary = max(
                code.rfind(";", 0, index),
                code.rfind("{", 0, index),
                code.rfind("}", 0, index),
            )
            header = code[boundary + 1 : index]
            control = re.search(
                r"(?<![A-Za-z0-9_])(?:else)?(if|while)\s*(.+)$",
                header,
            )
            reason: str | None = None
            if control and constant_false_predicate(control.group(2)):
                reason = (
                    f"inside statically false {control.group(1)} condition "
                    f"{control.group(2)!r}"
                )
            open_blocks.append((index, reason))
        elif char == "}" and open_blocks:
            open_index, reason = open_blocks.pop()
            if reason is not None:
                false_ranges.append((open_index, index, reason))
    return false_ranges


def return_is_braceless_closure_body(
    code: str,
    *,
    body_open: int,
    return_index: int,
) -> bool:
    """Recognize Rust closure expressions whose `return` belongs to the closure."""
    prefix = code[body_open + 1 : return_index]
    boundary = max(
        prefix.rfind(";"),
        prefix.rfind("{"),
        prefix.rfind("}"),
    )
    statement_prefix = prefix[boundary + 1 :]
    return (
        re.search(
            r"(?:^|[=(:,])(?:move)?\|[^|]*\|[^;{}]*$",
            statement_prefix,
        )
        is not None
    )


def top_level_return_offsets(code: str) -> list[int]:
    """Locate unconditional returns owned directly by the function body."""
    body_open = code.find("{")
    if body_open < 0:
        return []
    depth = 0
    offsets: list[int] = []
    index = body_open
    while index < len(code):
        char = code[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        elif depth == 1 and code.startswith("return", index):
            before = code[index - 1] if index > 0 else ""
            after_index = index + len("return")
            after = code[after_index] if after_index < len(code) else ""
            if not (before.isalnum() or before == "_") and not (
                after.isalnum() or after == "_"
            ):
                if not return_is_braceless_closure_body(
                    code,
                    body_open=body_open,
                    return_index=index,
                ):
                    offsets.append(index)
                index = after_index
                continue
        index += 1
    return offsets


def corridor_unreachable_required_code(
    code: str,
    required_occurrences: list[tuple[str, int]],
) -> list[dict[str, Any]]:
    """Apply bounded reachability checks to ordered required-code witnesses."""
    top_level_returns = top_level_return_offsets(code)
    false_ranges = corridor_false_block_ranges(code)
    unreachable: list[dict[str, Any]] = []
    for required_code, position in required_occurrences:
        reasons = [
            "after unconditional top-level return"
            for return_position in top_level_returns
            if return_position < position
        ]
        reasons.extend(
            reason
            for start, end, reason in false_ranges
            if start < position < end
        )
        if reasons:
            unreachable.append(
                {
                    "required_code": required_code,
                    "reason": reasons[0],
                }
            )
    return unreachable


def corridor_body_rows(
    payload: Any,
    *,
    symbol: str,
    file: str,
    signature_contains: str | None,
) -> list[dict[str, Any]]:
    if not isinstance(payload, dict) or payload.get("symbol") != symbol:
        raise RuntimeError(f"Loctree body payload for {symbol} is malformed")
    bodies = payload.get("bodies")
    if not isinstance(bodies, list) or any(not isinstance(row, dict) for row in bodies):
        raise RuntimeError(f"Loctree bodies for {symbol} must be a list of objects")
    rows: list[dict[str, Any]] = []
    for row in bodies:
        if row.get("file") != file:
            continue
        source = row.get("source")
        start_line = row.get("start_line")
        end_line = row.get("end_line")
        if (
            not isinstance(source, str)
            or not source
            or isinstance(start_line, bool)
            or not isinstance(start_line, int)
            or start_line <= 0
            or isinstance(end_line, bool)
            or not isinstance(end_line, int)
            or end_line < start_line
            or row.get("truncated") is not False
            or row.get("extent") != "brace"
            or type(row.get("total_lines")) is not int
            or row["total_lines"] != end_line - start_line + 1
            or len(source.splitlines()) != row["total_lines"]
        ):
            raise RuntimeError(f"Loctree body for {symbol} is incomplete: {row}")
        # Select the declaration, never a comment, string or nested function
        # in another typed method. Ambiguous declarations still fail upstream.
        header = code_without_comments_or_strings(source).partition("{")[0]
        signature = (
            code_without_comments_or_strings(signature_contains)
            if signature_contains is not None
            else None
        )
        if signature is not None and not signature:
            raise RuntimeError(f"empty executable signature selector for {symbol}")
        if signature is None or signature in header:
            rows.append(row)
    return rows


def parse_corridor_ordering(name: str, ordering: Any) -> list[dict[str, Any]]:
    """Validate declarative same-caller invocation ordering constraints."""
    if ordering is None:
        return []
    if not isinstance(ordering, list) or any(
        not isinstance(row, dict) for row in ordering
    ):
        raise RuntimeError(f"corridor {name} ordering must be a list of objects")

    expected_row_keys = {"caller", "caller_file", "before", "after"}
    expected_selector_keys = {"corridor", "callee"}
    expected_barrier_keys = {"required_code", "selection"}
    seen: set[tuple[str, ...]] = set()
    validated: list[dict[str, Any]] = []
    for row in ordering:
        if set(row) not in (expected_row_keys, expected_row_keys | {"barrier"}):
            raise RuntimeError(f"corridor {name} has malformed ordering entry: {row}")
        caller = row.get("caller")
        caller_file = row.get("caller_file")
        before = row.get("before")
        after = row.get("after")
        barrier = row.get("barrier")
        if (
            not isinstance(caller, str)
            or not caller
            or not isinstance(caller_file, str)
            or not caller_file
            or Path(caller_file).is_absolute()
            or not isinstance(before, dict)
            or set(before) != expected_selector_keys
            or not isinstance(after, dict)
            or set(after) != expected_selector_keys
        ):
            raise RuntimeError(f"corridor {name} has malformed ordering entry: {row}")
        if any(
            not isinstance(selector.get(key), str) or not selector[key]
            for selector in (before, after)
            for key in expected_selector_keys
        ):
            raise RuntimeError(f"corridor {name} has malformed ordering entry: {row}")
        if before == after:
            raise RuntimeError(
                f"corridor {name} ordering entry compares one invocation to itself: {row}"
            )
        if barrier is not None and (
            not isinstance(barrier, dict)
            or set(barrier) != expected_barrier_keys
            or not isinstance(barrier.get("required_code"), str)
            or not barrier["required_code"]
            or barrier.get("selection") != "last_before_after"
        ):
            raise RuntimeError(f"corridor {name} has malformed ordering barrier: {row}")
        identity = (
            caller,
            caller_file,
            str(before["corridor"]),
            str(before["callee"]),
            str(after["corridor"]),
            str(after["callee"]),
            str(barrier["required_code"]) if barrier is not None else "",
            str(barrier["selection"]) if barrier is not None else "",
        )
        if identity in seen:
            raise RuntimeError(f"corridor {name} duplicates ordering entry: {row}")
        seen.add(identity)
        validated.append(row)
    return validated


def invocation_receipts_for_ordering(
    observations: dict[str, Any],
    selector: dict[str, str],
    *,
    caller: str,
    caller_file: str,
) -> list[dict[str, Any]]:
    corridor = observations.get(selector["corridor"])
    if not isinstance(corridor, dict):
        return []
    invocations = corridor.get("invocations")
    if not isinstance(invocations, list):
        return []
    return [
        row
        for row in invocations
        if isinstance(row, dict)
        and row.get("caller") == caller
        and row.get("caller_file") == caller_file
        and row.get("callee") == selector["callee"]
    ]


def barrier_lines_for_ordering(
    verifier: StructuralVerifier,
    barrier: dict[str, str],
    *,
    caller: str,
    caller_file: str,
    after_lines: list[int],
) -> list[int]:
    """Select the caller-body offset of the declared barrier fragment."""
    if not after_lines:
        return []
    bodies = corridor_body_rows(
        verifier.body(caller, caller_file),
        symbol=caller,
        file=caller_file,
        signature_contains=None,
    )
    if len(bodies) != 1:
        return []
    body = bodies[0]
    fragment = code_without_comments_or_strings(barrier["required_code"])
    if not fragment:
        return []
    candidates = [
        int(body["start_line"]) + offset
        for offset, line in enumerate(str(body["source"]).splitlines())
        if fragment in code_without_comments_or_strings(line)
        and int(body["start_line"]) + offset < min(after_lines)
    ]
    return candidates[-1:] if barrier["selection"] == "last_before_after" else []


def verify_code_corridors(
    verifier: StructuralVerifier,
    contracts: Any,
) -> tuple[dict[str, Any], list[str]]:
    if contracts is None:
        return {}, []
    if not isinstance(contracts, list) or any(not isinstance(row, dict) for row in contracts):
        raise RuntimeError("required_corridors must be a list of objects")
    observations: dict[str, Any] = {}
    failures: list[str] = []
    seen_names: set[str] = set()
    ordering_contracts: dict[str, list[dict[str, Any]]] = {}
    for corridor in contracts:
        name = corridor.get("name")
        hops = corridor.get("hops")
        required_invocations = corridor.get("required_invocations")
        if not isinstance(name, str) or not name or name in seen_names:
            raise RuntimeError(f"corridor name is missing or duplicated: {name!r}")
        if not isinstance(hops, list) or not hops or any(not isinstance(hop, dict) for hop in hops):
            raise RuntimeError(f"corridor {name} must declare non-empty hops")
        if (
            not isinstance(required_invocations, list)
            or not required_invocations
            or any(not isinstance(row, dict) for row in required_invocations)
        ):
            raise RuntimeError(
                f"corridor {name} must declare non-empty required_invocations"
            )
        seen_names.add(name)
        ordering_contracts[name] = parse_corridor_ordering(
            name, corridor.get("ordering")
        )
        observed_hops: list[dict[str, Any]] = []
        for hop in hops:
            symbol = hop.get("symbol")
            file = hop.get("file")
            signature = hop.get("signature_contains")
            required_code = hop.get("required_code")
            ast_contract = hop.get("ast_contract")
            if ast_contract is not None and (
                ast_contract != symbol or AST_BODIES.get(symbol) != file
            ):
                raise RuntimeError("unadmitted AST hop contract")
            if (
                not isinstance(symbol, str)
                or not symbol
                or not isinstance(file, str)
                or not file
                or Path(file).is_absolute()
                or (signature is not None and not isinstance(signature, str))
                or not isinstance(required_code, list)
                or (not required_code and ast_contract is None)
                or any(not isinstance(item, str) or not item for item in required_code)
            ):
                raise RuntimeError(f"corridor {name} has malformed hop: {hop}")
            rows = corridor_body_rows(
                verifier.body(symbol, file),
                symbol=symbol,
                file=file,
                signature_contains=signature,
            )
            missing_code: list[str] = []
            production_definition = False
            start_line: int | None = None
            required_code_in_order = False
            dead_code_markers: list[str] = []
            unreachable_required_code: list[dict[str, Any]] = []
            if len(rows) == 1:
                body = rows[0]
                start_line = int(body["start_line"])
                code = code_without_comments_or_strings(str(body["source"]))
                normalized_required_code = [
                    code_without_comments_or_strings(snippet)
                    for snippet in required_code
                ]
                missing_code = [
                    snippet
                    for snippet, normalized in zip(
                        required_code, normalized_required_code, strict=True
                    )
                    if normalized not in code
                ]
                cursor = 0
                required_code_in_order = True
                required_occurrences: list[tuple[str, int]] = []
                for snippet, normalized in zip(
                    required_code, normalized_required_code, strict=True
                ):
                    position = code.find(normalized, cursor)
                    if position < 0:
                        required_code_in_order = False
                        break
                    required_occurrences.append((snippet, position))
                    cursor = position + len(normalized)
                dead_code_markers = [
                    marker
                    for marker in CORRIDOR_DEAD_CODE_MARKERS
                    if code_without_comments_or_strings(marker) in code
                ]
                if required_code_in_order:
                    unreachable_required_code = corridor_unreachable_required_code(
                        code, required_occurrences
                    )
                production_definition = any(
                    row.get("file") == file
                    and row.get("line") == start_line
                    and (
                        row.get("match_role") == "definition"
                        or (
                            row.get("match_role") == "local_binding"
                            and isinstance(row.get("enclosing_symbol"), dict)
                            and row["enclosing_symbol"].get("name") == symbol
                            and row["enclosing_symbol"].get("file") == file
                            and row["enclosing_symbol"].get("line") == start_line
                            and row["enclosing_symbol"].get("kind") == "function"
                        )
                    )
                    for row in production_occurrences(verifier.occurrences(symbol))
                )
            ast_result = None
            if ast_contract is not None:
                evidence = structural_ast_evidence(verifier)
                ast_result = next(row for row in evidence["contracts"] if row["symbol"] == symbol)
                companion = {"paste_text_from_overlay": "execute_clipboard_paste", "stop": "complete_stop"}.get(symbol)
                companion_failures = [failure for row in evidence["contracts"]
                                      if row["symbol"] == companion for failure in row["failures"]]
                if not ast_result["accepted"] or evidence["failures"] or companion_failures:
                    failures.append(f"corridor {name} AST {symbol} refused: {ast_result['failures'] + evidence['failures'] + companion_failures}")
                # This hop has no fragment-order claim; the complete AST contract
                # owns its control-flow evidence, including callbacks and exits.
                required_code_in_order = True
            observed_hops.append(
                {
                    "ast_contract": ast_result,
                    "symbol": symbol,
                    "file": file,
                    "signature_contains": signature,
                    "body_count": len(rows),
                    "start_line": start_line,
                    "production_definition": production_definition,
                    "required_code": required_code,
                    "missing_code": missing_code,
                    "required_code_in_order": required_code_in_order,
                    "dead_code_markers": dead_code_markers,
                    "unreachable_required_code": unreachable_required_code,
                }
            )
            if len(rows) != 1:
                failures.append(
                    f"corridor {name} hop {symbol} expected one body in {file}; observed {len(rows)}"
                )
            elif not production_definition:
                failures.append(
                    f"corridor {name} hop {symbol} is not a production definition in {file}"
                )
            elif missing_code:
                failures.append(
                    f"corridor {name} hop {symbol} is missing executable code {missing_code}"
                )
            elif not required_code_in_order:
                failures.append(
                    f"corridor {name} hop {symbol} has executable code out of required order"
                )
            elif dead_code_markers:
                failures.append(
                    f"corridor {name} hop {symbol} contains dead-code markers {dead_code_markers}"
                )
            elif unreachable_required_code:
                failures.append(
                    f"corridor {name} hop {symbol} has unreachable required code "
                    f"{unreachable_required_code}"
                )

        observed_invocations: list[dict[str, Any]] = []
        for invocation in required_invocations:
            callee = invocation.get("callee")
            callee_file = invocation.get("callee_file")
            caller = invocation.get("caller")
            caller_file = invocation.get("caller_file")
            minimum_count = invocation.get("minimum_count", 1)
            if (
                not isinstance(callee, str)
                or not callee
                or not isinstance(callee_file, str)
                or not callee_file
                or Path(callee_file).is_absolute()
                or not isinstance(caller, str)
                or not caller
                or not isinstance(caller_file, str)
                or not caller_file
                or Path(caller_file).is_absolute()
                or type(minimum_count) is not int
                or minimum_count < 1
            ):
                raise RuntimeError(
                    f"corridor {name} has malformed required invocation: {invocation}"
                )
            occurrence_rows = production_occurrences(verifier.occurrences(callee))
            callee_definition_in_file = any(
                row.get("file") == callee_file
                and (
                    row.get("match_role") == "definition"
                    or (
                        row.get("match_role") == "local_binding"
                        and isinstance(row.get("enclosing_symbol"), dict)
                        and row["enclosing_symbol"].get("name") == callee
                        and row["enclosing_symbol"].get("file") == callee_file
                    )
                )
                for row in occurrence_rows
            )
            matching_rows = [
                row
                for row in occurrence_rows
                if row.get("file") == caller_file
                and row.get("match_role") == "reference"
                and isinstance(row.get("enclosing_symbol"), dict)
                and row["enclosing_symbol"].get("name") == caller
                and row["enclosing_symbol"].get("file") == caller_file
            ]
            if caller in AST_BODIES and caller_file == AST_BODIES[caller]:
                caller_rows = corridor_body_rows(verifier.body(caller, caller_file),
                    symbol=caller, file=caller_file, signature_contains=None)
                if len(caller_rows) != 1:
                    matching_rows = []
                else:
                    caller_body = caller_rows[0]
                    matching_rows = [row for row in matching_rows if type(row.get("line")) is int
                                     and caller_body["start_line"] <= row["line"] <= caller_body["end_line"]]
            observed_invocations.append(
                {
                    "callee": callee,
                    "callee_file": callee_file,
                    "caller": caller,
                    "caller_file": caller_file,
                    "minimum_count": minimum_count,
                    "callee_definition_in_file": callee_definition_in_file,
                    "observed_count": len(matching_rows),
                    "observed_lines": sorted(
                        int(row["line"])
                        for row in matching_rows
                        if type(row.get("line")) is int
                    ),
                }
            )
            if not callee_definition_in_file:
                failures.append(
                    f"corridor {name} invocation {caller} -> {callee} has no production callee definition in {callee_file}"
                )
            if len(matching_rows) < minimum_count:
                failures.append(
                    f"corridor {name} invocation {caller} -> {callee} expected at least {minimum_count} production callsite(s) in {caller_file}; observed {len(matching_rows)}"
                )
        observations[name] = {
            "hops": observed_hops,
            "invocations": observed_invocations,
        }

    for name, ordering in ordering_contracts.items():
        if not ordering:
            continue
        observed_ordering: list[dict[str, Any]] = []
        for constraint in ordering:
            caller = str(constraint["caller"])
            caller_file = str(constraint["caller_file"])
            before = constraint["before"]
            after = constraint["after"]
            barrier = constraint.get("barrier")
            before_receipts = invocation_receipts_for_ordering(
                observations, before, caller=caller, caller_file=caller_file
            )
            after_receipts = invocation_receipts_for_ordering(
                observations, after, caller=caller, caller_file=caller_file
            )
            before_lines = (
                list(before_receipts[0].get("observed_lines", []))
                if len(before_receipts) == 1
                else []
            )
            after_lines = (
                list(after_receipts[0].get("observed_lines", []))
                if len(after_receipts) == 1
                else []
            )
            ordered = bool(
                before_lines
                and after_lines
                and all(line < min(after_lines) for line in before_lines)
            )
            barrier_lines = (
                barrier_lines_for_ordering(
                    verifier,
                    barrier,
                    caller=caller,
                    caller_file=caller_file,
                    after_lines=after_lines,
                )
                if barrier is not None
                else []
            )
            barrier_ordered = barrier is None or bool(
                before_lines
                and barrier_lines
                and all(line < min(barrier_lines) for line in before_lines)
            )
            verdict = "GREEN" if ordered and barrier_ordered else "RED"
            observed_row = {
                "caller": caller,
                "caller_file": caller_file,
                "before": before,
                "after": after,
                "before_observed_lines": before_lines,
                "after_observed_lines": after_lines,
                "verdict": verdict,
            }
            if barrier is not None:
                observed_row["barrier"] = barrier
                observed_row["barrier_observed_lines"] = barrier_lines
            observed_ordering.append(observed_row)
            if len(before_receipts) != 1 or len(after_receipts) != 1:
                failures.append(
                    f"corridor {name} ordering {caller} expected one declared "
                    f"invocation receipt for {before['corridor']}:{before['callee']} "
                    f"before {after['corridor']}:{after['callee']}; observed "
                    f"before {len(before_receipts)}, after {len(after_receipts)}"
                )
            elif not before_lines or not after_lines:
                missing = []
                if not before_lines:
                    missing.append(f"before {before['corridor']}:{before['callee']}")
                if not after_lines:
                    missing.append(f"after {after['corridor']}:{after['callee']}")
                failures.append(
                    f"corridor {name} ordering {caller} has no observed production "
                    f"callsite(s) for {', '.join(missing)}"
                )
            elif not ordered:
                failures.append(
                    f"corridor {name} ordering {caller} requires "
                    f"{before['corridor']}:{before['callee']} before "
                    f"{after['corridor']}:{after['callee']}; observed before lines "
                    f"{before_lines}, after lines {after_lines}"
                )
            elif barrier is not None and not barrier_lines:
                failures.append(
                    f"corridor {name} ordering {caller} has no observed "
                    f"required-code barrier {barrier['required_code']!r} selected as "
                    f"{barrier['selection']}"
                )
            elif not barrier_ordered:
                failures.append(
                    f"corridor {name} ordering {caller} requires "
                    f"{before['corridor']}:{before['callee']} before "
                    f"required-code barrier {barrier['required_code']!r} and "
                    f"{after['corridor']}:{after['callee']}; observed before "
                    f"lines {before_lines}, barrier lines {barrier_lines}, after "
                    f"lines {after_lines}"
                )
        observations[name]["ordering"] = observed_ordering
    return observations, failures


def normalized_repo_path(path: Path) -> str:
    """Normalize a Loctree-owned repo path without consulting the filesystem."""
    if path.is_absolute():
        raise RuntimeError(f"Loctree module path escaped the repository: {path}")
    parts: list[str] = []
    for part in path.parts:
        if part in {"", "."}:
            continue
        if part == "..":
            if not parts:
                raise RuntimeError(f"Loctree module path escaped the repository: {path}")
            parts.pop()
            continue
        parts.append(part)
    if not parts:
        raise RuntimeError(f"Loctree module path resolved to repository root: {path}")
    return "/".join(parts)


def module_candidates(source: str, module: str) -> list[str]:
    source_path = Path(source)
    if source_path.name in STANDARD_MODULE_SOURCE_NAMES:
        direct = source_path.parent
    else:
        # Rust 2018 file module: `foo.rs` declares `mod bar;` and rustc looks
        # in `foo/bar.rs` or `foo/bar/mod.rs` (e.g. core/llm/speech.rs ->
        # core/llm/speech/tests.rs). Before this arm the verifier reported
        # such declarations as unresolved although cargo compiled them.
        direct = source_path.parent / source_path.stem
    candidates = [
        direct / f"{module}.rs",
        direct / module / "mod.rs",
    ]
    return list(dict.fromkeys(normalized_repo_path(path) for path in candidates))


def module_inventory(payload: Any) -> tuple[set[str], list[dict[str, Any]]]:
    if not isinstance(payload, list) or any(not isinstance(row, dict) for row in payload):
        raise RuntimeError("Loctree module inventory must be a JSON list of file rows")
    paths: list[str] = []
    edges: list[dict[str, Any]] = []
    for row in payload:
        path = row.get("path")
        imports = row.get("imports")
        if not isinstance(path, str) or not isinstance(imports, list):
            raise RuntimeError(f"Loctree module inventory row is malformed: {row}")
        normalized_path = normalized_repo_path(Path(path))
        if normalized_path != path:
            raise RuntimeError(f"Loctree inventory path is not normalized: {path}")
        paths.append(path)
        for edge in imports:
            if not isinstance(edge, dict):
                raise RuntimeError(f"Loctree module edge is malformed: {edge}")
            edges.append({"file": path, **edge})
    normalized = [normalized_repo_path(Path(path)) for path in paths]
    if len(set(normalized)) != len(normalized):
        raise RuntimeError("Loctree file inventory contains duplicate normalized paths")
    return set(normalized), edges


def regex_indexed_file_count(payload: Any, label: str) -> int:
    try:
        value = payload["matches"]["universe"]["indexed_files"]
    except (KeyError, TypeError) as error:
        raise RuntimeError(f"Loctree regex payload for {label} lacks indexed count") from error
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise RuntimeError(f"Loctree regex payload for {label} has invalid indexed count")
    return value


def unresolved_module_declarations(
    module_payload: Any,
    path_payload: Any,
    inventory_payload: Any,
) -> list[dict[str, Any]]:
    module_rows = [
        row
        for row in require_complete_regex_evidence(
            module_payload,
            "Rust module declarations",
            expected_query=RUST_MODULE_DECLARATION_PATTERN,
        )
        if str(row.get("file", "")).endswith(".rs")
        and row.get("match_role") not in NON_CODE_ROLES
    ]
    path_rows = [
        row
        for row in require_complete_regex_evidence(
            path_payload,
            "Rust path attributes",
            expected_query=RUST_PATH_ATTRIBUTE_PATTERN,
        )
        if str(row.get("file", "")).endswith(".rs")
        and row.get("match_role") not in NON_CODE_ROLES
    ]

    indexed_paths, module_edges = module_inventory(inventory_payload)
    for payload, label in (
        (module_payload, "Rust module declarations"),
        (path_payload, "Rust path attributes"),
    ):
        if regex_indexed_file_count(payload, label) != len(indexed_paths):
            raise RuntimeError(
                f"Loctree {label} universe disagrees with module inventory"
            )
    for row in [*module_rows, *path_rows]:
        if row.get("file") not in indexed_paths:
            raise RuntimeError(f"Loctree regex row is outside module inventory: {row}")

    modules_by_key: dict[tuple[str, int], str] = {}
    for row in module_rows:
        file = str(row.get("file", ""))
        line = int(row.get("line", 0))
        module_match = RUST_MOD_RE.match(str(row.get("matched_text", "")))
        if module_match is None:
            raise RuntimeError(f"Loctree returned an unparsable Rust module declaration: {row}")
        key = (file, line)
        if key in modules_by_key:
            raise RuntimeError(f"Loctree module census has duplicate declaration: {key}")
        modules_by_key[key] = module_match.group(1)

    resolved_edge_keys: set[tuple[str, int]] = set()
    for edge in module_edges:
        file = edge.get("file")
        line = edge.get("line")
        target = edge.get("resolved_path")
        symbols = edge.get("symbols")
        if (
            not isinstance(file, str)
            or isinstance(line, bool)
            or not isinstance(line, int)
            or line <= 0
            or not isinstance(target, str)
            or not target
            or not isinstance(symbols, list)
            or len(symbols) != 1
            or not isinstance(symbols[0], dict)
            or not isinstance(symbols[0].get("name"), str)
        ):
            raise RuntimeError(f"Loctree module edge is incomplete: {edge}")
        key = (file, line)
        if key in resolved_edge_keys:
            raise RuntimeError(f"Loctree module inventory has duplicate edge: {key}")
        if key not in modules_by_key:
            raise RuntimeError(f"Loctree module edge has no regex declaration: {edge}")
        if symbols[0]["name"] != modules_by_key[key]:
            raise RuntimeError(f"Loctree module edge name disagrees with declaration: {edge}")
        normalized_target = normalized_repo_path(Path(target))
        if normalized_target != target or target not in indexed_paths:
            raise RuntimeError(f"Loctree module edge target is outside inventory: {edge}")
        resolved_edge_keys.add(key)

    immediate_path_overrides: dict[tuple[str, int], str] = {}
    for row in path_rows:
        file = str(row.get("file", ""))
        line = int(row.get("line", 0))
        path_match = RUST_PATH_RE.match(str(row.get("matched_text", "")))
        if path_match is None:
            raise RuntimeError(f"Loctree returned an unparsable Rust path attribute: {row}")
        key = (file, line + 1)
        if key in immediate_path_overrides:
            raise RuntimeError(
                f"Loctree module has more than one immediate path override: {key}"
            )
        immediate_path_overrides[key] = path_match.group(1)

    unresolved: list[dict[str, Any]] = []
    missing_edge_keys = sorted(set(modules_by_key).difference(resolved_edge_keys))
    for source, line in missing_edge_keys:
        module = modules_by_key[(source, line)]
        override = immediate_path_overrides.get((source, line))
        candidates = (
            [normalized_repo_path(Path(source).parent / override)]
            if override is not None
            else module_candidates(source, module)
        )
        existing = [candidate for candidate in candidates if candidate in indexed_paths]
        if len(existing) == 1:
            continue
        unresolved.append(
            {
                "file": source,
                "line": line,
                "module": module,
                "candidates": candidates,
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
    forbidden_literal_hits, forbidden_literal_failures = (
        verify_forbidden_executable_literals(
            verifier, manifest.get("forbidden_executable_literals")
        )
    )
    failures.extend(forbidden_literal_failures)
    residue_by_substring = build_residue_by_substring(verifier, forbidden_symbols)
    residue_gate = residue_by_substring["summary"]
    if residue_gate["unclassified_count"] or residue_gate["review_required_count"]:
        failures.append(
            "forbidden residue retains unresolved classifications: "
            f"unclassified={residue_gate['unclassified_count']}, "
            f"review_required={residue_gate['review_required_count']}"
        )
    executable_residue = sum(
        residue_gate["class_counts"][name]
        for name in ("executable_authority", "executable_consumer")
    )
    if executable_residue:
        failures.append(f"forbidden residue retains {executable_residue} executable rows")

    module_payload = verifier.run_loct(
        "find", "--regex", RUST_MODULE_DECLARATION_PATTERN, "--all"
    ).payload
    path_payload = verifier.run_loct(
        "find", "--regex", RUST_PATH_ATTRIBUTE_PATTERN, "--all"
    ).payload
    inventory_payload = verifier.run_loct_query(LOCTREE_MODULE_INVENTORY_QUERY).payload
    unresolved_modules = unresolved_module_declarations(
        module_payload, path_payload, inventory_payload
    )
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

    corridor_paths, corridor_failures = verify_code_corridors(
        verifier, stage_contract.get("required_corridors")
    )
    failures.extend(corridor_failures)

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

    ending_context = verifier.context()
    ending_receipt = ending_context.get("receipt", {})
    coherence_keys = (
        "head_full",
        "dirty_fingerprint",
        "snapshot_fingerprint",
        "binary_id",
        "authority",
    )
    context_drift = {
        key: (context_receipt.get(key), ending_receipt.get(key))
        for key in coherence_keys
        if context_receipt.get(key) != ending_receipt.get(key)
    }
    if context_drift:
        failures.append(f"Loctree context changed during structural verification: {context_drift}")

    expected_verdict = STAGE_VERDICTS[stage]
    conformant = not failures
    receipt = {
        "$schema": "tests/fixtures/acoustic_throne_stages.json#/tool_contract/receipt_schema",
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
        "forbidden_literal_hits": forbidden_literal_hits,
        "residue_by_substring": residue_by_substring,
        "consumer_paths": consumer_paths,
        "corridor_paths": corridor_paths,
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
        "neutral_ast": getattr(verifier, "_structural_ast_evidence", None),
        "command_policy": {
            "allowed_executable": ALLOWED_EXECUTABLE,
            "neutral_ast_command": list(AST_COMMAND),
            "all_commands_allowed": all(command_allowed(command) for command in verifier.command_inventory),
        },
    }
    return receipt, conformant


def inventory() -> dict[str, Any]:
    return {
        "allowed_executable": ALLOWED_EXECUTABLE,
        "neutral_ast_command": list(AST_COMMAND),
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
    if not all(isinstance(row, str) and (row.startswith("loct ") or row == " ".join(AST_COMMAND)) for row in inventory_rows):
        raise RuntimeError(f"receipt contains an unadmitted command: {inventory_rows}")
    policy = receipt.get("command_policy", {})
    if policy != {"allowed_executable": ALLOWED_EXECUTABLE, "neutral_ast_command": list(AST_COMMAND), "all_commands_allowed": True}:
        raise RuntimeError(f"receipt command policy is not fail-closed: {policy}")
    if " ".join(AST_COMMAND) in inventory_rows:
        evidence = receipt.get("neutral_ast")
        if (not isinstance(evidence, dict) or evidence.get("identity") != AST_IDENTITY
            or evidence.get("invocation", {}).get("command") != list(AST_COMMAND)
            or not re.fullmatch(r"[0-9a-f]{64}", evidence.get("invocation", {}).get("source_sha256", ""))):
            raise RuntimeError("neutral AST invocation lacks authenticated tool receipt")
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
