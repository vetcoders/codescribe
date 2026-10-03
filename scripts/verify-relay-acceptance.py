#!/usr/bin/env python3
"""Build and run the Relay contract witnesses; any failure or missing test is red.

No microphone, provider calls, install or host settings are needed. This suite
is deliberately red until the audited defects are implemented. See
docs/RELAY_ACCEPTANCE.md for the boundary between these tests and live acceptance.
"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys


SUITES = {
    "codescribe_core": [
        "pipeline::acoustic_ledger::slot_ops::slot_ops_tests::",
        "pipeline::acoustic_ledger::tests::w0_falsifier_",
        "pipeline::streaming::apple_live_session::live_refinement_admission_tests::",
        "pipeline::streaming::apple_live_session::composer_turn_formatter_arming_tests::",
        "pipeline::streaming::apple_live_session::word_no_speech_tests::",
        "pipeline::light_plus::tests::",
        "pipeline::trail::tests::",
        "llm::ai_formatting::tests::corrections_",
    ],
    "codescribe": [
        "presentation::emitter::tests::relay_acceptance_",
        "presentation::emitter::tests::w0_falsifier_",
        "presentation::emitter::tests::formatter_proposal_derives_without_relabeling_any_occurrence",
        "presentation::emitter::tests::corrections_losing_a_word_mints_a_floor_without_changing_raw",
        "presentation::emitter::tests::old_smart_source_is_kept_as_refused_history_without_overwriting_correction",
        "presentation::emitter::tests::smart_deadline_delivers_one_whole_corrections_version_and_keeps_raw",
        "presentation::emitter::tests::w0_saved_decisions_replay_through_production_ledger_and_reducer",
        "presentation::emitter::tests::agent_channel_five_iwo_pcm_to_ledger_reducer_and_delivery",
        "presentation::transcript_bus::p0_b_five_iwo::",
    ],
}


def main():
    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ, CODESCRIBE_DISABLE_KEYCHAIN="1")
    env.setdefault("CARGO_BUILD_JOBS", "2")
    command = [
        "cargo", "test", "-p", "codescribe-core", "-p", "codescribe",
        "--lib", "--no-run", "--message-format=json-render-diagnostics",
    ]
    # Cargo owns executable discovery; no stale target glob or hardcoded hash.
    build = subprocess.run(command, cwd=root, env=env, text=True, stdout=subprocess.PIPE)
    if build.returncode:
        print(build.stdout)
        return build.returncode
    executables = {}
    for line in build.stdout.splitlines():
        record = json.loads(line)
        if record.get("reason") == "compiler-artifact" and record.get("executable"):
            target = record["target"]
            if record["profile"]["test"] and target["name"] in SUITES:
                executables[target["name"]] = record["executable"]
    if set(executables) != set(SUITES):
        print("FAIL: Cargo did not produce both current library test binaries", file=sys.stderr)
        return 1

    failures = []
    executed = 0
    for crate, filters in SUITES.items():
        for selector in filters:
            listed = subprocess.run(
                [executables[crate], selector, "--list", "--format=terse"],
                cwd=root, env=env, text=True, stdout=subprocess.PIPE, check=True,
            )
            count = sum(line.endswith(": test") for line in listed.stdout.splitlines())
            if count == 0:
                failures.append(selector + " (zero tests)")
                continue
            print(f"\n=== {crate}: {selector} ({count} tests) ===", flush=True)
            result = subprocess.run(
                [executables[crate], selector, "--include-ignored", "--test-threads=1"],
                cwd=root, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            )
            print(result.stdout, flush=True)
            summary = re.search(
                r"test result: .*? (\d+) passed; (\d+) failed; (\d+) ignored;",
                result.stdout,
            )
            if not summary:
                failures.append(selector + " (missing completion summary)")
                continue
            passed, failed, ignored = map(int, summary.groups())
            executed += passed + failed
            if result.returncode or failed or ignored or passed + failed != count:
                failures.append(selector)
    print(f"\nRelay acceptance: {executed} executions; {len(failures)} failing groups.")
    for failure in failures:
        print("FAIL:", failure)
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
