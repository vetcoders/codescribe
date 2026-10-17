"""Declarative contract of the CI workflows that a native merge queue relies on.

Parses the real YAML. The l10n base-selection shell is executed separately in
test_l10n_workflow_base.py; this module only judges triggers, concurrency and the
required `Clippy + Tests` job shape.
"""

import os
from pathlib import Path
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = Path(os.environ.get("WORKFLOWS_DIR", ROOT / ".github/workflows"))
CHECKS = ("rust.yml", "l10n.yml", "semgrep.yml")


def load(name):
    document = yaml.safe_load((WORKFLOWS / name).read_text())
    # PyYAML (YAML 1.1) reads the bare key `on` as boolean True.
    document["on"] = document.pop(True, document.get("on"))
    return document


class CiWorkflowContractTests(unittest.TestCase):
    def test_every_check_runs_for_merge_queue_entries(self):
        for name in CHECKS:
            with self.subTest(workflow=name):
                trigger = load(name)["on"]
                self.assertIn("merge_group", trigger)
                self.assertEqual(trigger["merge_group"], {"types": ["checks_requested"]})

    def test_concurrency_isolates_event_and_never_cancels_queue_runs(self):
        for name in CHECKS:
            with self.subTest(workflow=name):
                concurrency = load(name)["concurrency"]
                self.assertIn("github.event_name", concurrency["group"])
                self.assertIn("github.ref", concurrency["group"])
                cancel = concurrency["cancel-in-progress"]
                expected = (
                    "${{ github.event_name != 'merge_group' }}"
                    if name == "l10n.yml"
                    else "${{ github.event_name == 'pull_request' }}"
                )
                self.assertEqual(cancel, expected)

    def test_pull_request_and_push_triggers_survive(self):
        for name in ("rust.yml", "semgrep.yml"):
            trigger = load(name)["on"]
            self.assertEqual(trigger["push"]["branches"], ["main", "develop"])
            self.assertEqual(trigger["pull_request"]["branches"], ["main", "develop"])
        self.assertEqual(load("l10n.yml")["on"]["push"]["branches"], ["**"])

    def test_required_status_names_are_preserved(self):
        jobs = load("rust.yml")["jobs"]
        self.assertEqual(jobs["quality"]["name"], "Clippy + Tests")
        self.assertEqual(jobs["msrv"]["name"], "Rust 1.94 MSRV")
        self.assertEqual(jobs["format"]["name"], "Format Check")
        self.assertEqual(load("l10n.yml")["jobs"]["l10n"]["name"], "Localization")
        self.assertEqual(load("semgrep.yml")["jobs"]["semgrep"]["name"], "Security Scan")

    def test_quality_job_stays_unconditional_and_hosted(self):
        quality = load("rust.yml")["jobs"]["quality"]
        self.assertNotIn("if", quality)
        self.assertNotIn("continue-on-error", quality)
        self.assertNotIn("self-hosted", str(quality["runs-on"]))
        commands = [step.get("run", "") for step in quality["steps"]]
        for expected in ("cargo clippy --workspace --all-targets -- -D warnings", "make verify"):
            self.assertIn(expected, commands)
        for step in quality["steps"]:
            self.assertNotIn("if", step)
            self.assertNotIn("continue-on-error", step)

    def test_contract_tests_are_a_required_step_before_compiler_stages(self):
        steps = load("rust.yml")["jobs"]["quality"]["steps"]
        index = next(
            (i for i, step in enumerate(steps) if "test_ci_workflow_contract.py" in step.get("run", "")),
            None,
        )
        self.assertIsNotNone(index, "no quality step runs the queue contract tests")
        run = steps[index]["run"]
        self.assertIn("scripts/tests/test_l10n_workflow_base.py", run)
        self.assertIn("PyYAML==6.0.3", run)
        self.assertIn("$RUNNER_TEMP", run)
        self.assertIn("-m venv", run)
        self.assertNotIn("sudo", run)
        self.assertNotIn("--user", run)
        # Cheap contract check precedes every Swift/Rust/Make stage.
        for earlier in steps[:index]:
            self.assertNotIn("make ", earlier.get("run", ""))
            self.assertNotIn("xcodebuild", earlier.get("run", ""))
            self.assertNotIn("cargo", earlier.get("run", ""))
            self.assertNotIn("rust-toolchain", earlier.get("uses", ""))

    def test_pull_request_target_is_not_used(self):
        for name in CHECKS:
            self.assertNotIn("pull_request_target", load(name)["on"])


if __name__ == "__main__":
    unittest.main()
