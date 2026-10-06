#!/usr/bin/env python3
"""Check actual GUI proof receipts; never launch audio or manufacture results."""

import argparse
import hashlib
import json
from pathlib import Path


def verify(proof: Path, expected_commit: str) -> list[str]:
    failures = []
    cost = json.loads((proof / "analysis-cost.json").read_text())
    runtime = json.loads((proof / "installed-runtime.json").read_text())
    if runtime["plist"]["CSBuildCommit"] != expected_commit:
        failures.append("installed generation does not match requested commit")
    cases = cost["cases"]
    if len(cases) != 10 or not cost["complete_ten_case_snapshot"]:
        failures.append("ten complete actual GUI cases required")
    sessions = set()
    source_hashes = set()
    for case in cases:
        take = case["take"]
        receipt = json.loads((proof / take / "receipt.json").read_text())
        if receipt["controls"] != "actual Codescribe GUI via mcp__cua_repl":
            failures.append(f"{take}: actual GUI control receipt required")
        if receipt.get("playback_exit") != 0:
            failures.append(f"{take}: source playback failed")
        fixture = receipt["fixture"]
        source = Path(fixture["source_path"])
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        if digest != fixture["wav_sha256"]:
            failures.append(f"{take}: input WAV hash changed")
        source_hashes.add(digest)
        sid = case["session_id"]
        if sid in sessions or receipt["session_ids"] != [sid]:
            failures.append(f"{take}: unique matching session required")
        sessions.add(sid)
        events = [json.loads(line) for line in (proof / take / "bus.jsonl").read_text().splitlines()]
        ends = [e for e in events if e.get("session_id") == sid and e.get("status") == "session_ended"]
        if len(ends) != 1 or ends[0].get("end_reason") != "completed":
            failures.append(f"{take}: one completed terminal required")
        evidence = [e for e in events if e.get("session_id") == sid and e.get("schema") == "codescribe.transcript-evidence.v1"]
        final = evidence[-1] if evidence else {}
        if final.get("seal_coverage", {}).get("status") != "complete" or not final.get("rendered_text", "").strip():
            failures.append(f"{take}: nonempty transcript with complete seal coverage required")
        if case["cost"]["maximum_source_window_multiplicity"] > 3:
            failures.append(f"{take}: source PCM decoded more than three times")
        if case["counts"]["successful_logical_whisper_tail_calls"] < 1:
            failures.append(f"{take}: no successful decode receipt")
    if len(source_hashes) != 10:
        failures.append("ten distinct immutable input WAVs required")
    # Old ASR output is not a human reference. This verifier deliberately does
    # not certify word accuracy, failed inference counts or physical encoders.
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("proof", type=Path)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    failures = verify(args.proof, args.commit)
    print(json.dumps({"pass": not failures, "failures": failures}, ensure_ascii=False, indent=2))
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
