"""Declarative contract: the required workflows can run for the native merge queue.

A protected `main` with required status checks deadlocks a merge queue unless
every required workflow also triggers on `merge_group`, never cancels a queue
entry, and keeps the same jobs on public GitHub-hosted runners. This test reads
the real workflow files and checks those declarations. It proves nothing about
hosted execution; that is observed only on GitHub.
"""
import copy
import re
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"

# workflow file -> {job id: required job name}. These are the required status
# names; renaming one silently detaches the branch ruleset from the check.
REQUIRED = {
    "rust.yml": {
        "msrv": "Rust 1.94 MSRV",
        "format": "Format Check",
        "quality": "Clippy + Tests",
    },
    "semgrep.yml": {"semgrep": "Security Scan"},
}

CANCEL_ONLY_PR = "${{ github.event_name == 'pull_request' }}"

SUITE = "scripts/tests/test_ci_workflow_merge_queue.py"
SUITE_STEP = "Run workflow merge-queue contract tests"


def suite_step_violations(wf):
    """Ways the `quality` job fails to run this suite as a required gate."""
    steps = wf["jobs"]["quality"]["steps"]
    named = [i for i, s in enumerate(steps) if s.get("name") == SUITE_STEP]
    if len(named) != 1:
        return [f"exactly one {SUITE_STEP!r} step required, found {len(named)}"]
    idx = named[0]
    step = steps[idx]
    found = []
    run = step.get("run", "")
    if "if" in step or step.get("continue-on-error"):
        found.append("suite step must be unconditional and must not ignore failure")
    if SUITE not in run:
        found.append("suite step must execute the suite")
    if "PyYAML==6.0.3" not in run:
        found.append("PyYAML must be pinned to 6.0.3")
    if "$RUNNER_TEMP" not in run or "-m venv" not in run:
        found.append("PyYAML must be installed into a venv under RUNNER_TEMP")
    if re.search(r"sudo|--user|--break-system-packages|(?<!-m )\bpip3? install", run, re.M):
        found.append("no system or global pip installs")
    later = [s.get("run", "") for s in steps[idx + 1 :]]
    for gate in ("cargo clippy", "make verify"):
        if not any(gate in r for r in later):
            found.append(f"suite step must run before {gate!r}")
    return found


def load(name):
    return yaml.safe_load((WORKFLOWS / name).read_text())


def triggers(wf):
    # PyYAML (YAML 1.1) reads the bare key `on` as boolean True.
    return wf.get("on", wf.get(True))


def violations(wf, jobs):
    """Every way `wf` fails the merge-queue contract; empty means it holds."""
    found = []
    on = triggers(wf)
    if not isinstance(on, dict):
        return ["`on` is not a mapping"]

    mg = on.get("merge_group")
    if not isinstance(mg, dict) or mg.get("types") != ["checks_requested"]:
        found.append("merge_group must trigger with types: [checks_requested]")
    for event, cfg in on.items():
        if event == "pull_request_target":
            found.append("pull_request_target is privileged and forbidden")
        if isinstance(cfg, dict) and ("paths" in cfg or "paths-ignore" in cfg):
            found.append(f"{event} must not use path filters")
    if on.get("pull_request", {}).get("branches") != ["main", "develop"]:
        found.append("pull_request must keep branches [main, develop]")

    conc = wf.get("concurrency")
    if not isinstance(conc, dict):
        found.append("concurrency block missing")
    else:
        if "github.event_name" not in str(conc.get("group")):
            found.append("concurrency group must isolate by github.event_name")
        if conc.get("cancel-in-progress") != CANCEL_ONLY_PR:
            found.append("cancel-in-progress must be true only for pull_request")

    for job_id, job_name in jobs.items():
        job = wf.get("jobs", {}).get(job_id)
        if job is None:
            found.append(f"job {job_id} missing")
            continue
        if job.get("name") != job_name:
            found.append(f"job {job_id} must keep name {job_name!r}")
        if job.get("continue-on-error"):
            found.append(f"job {job_id} must not continue-on-error")
        if "if" in job:
            found.append(f"job {job_id} must not be conditional")
        runs_on = str(job.get("runs-on"))
        if re.search(r"self-hosted|macos-public", runs_on):
            found.append(f"job {job_id} routes to a self-hosted runner")
        for step in job.get("steps", []):
            if step.get("continue-on-error"):
                found.append(f"job {job_id} step {step.get('name')!r} ignores failure")
    return found


class MergeQueueContract(unittest.TestCase):
    def test_real_workflows_satisfy_contract(self):
        for name, jobs in REQUIRED.items():
            with self.subTest(workflow=name):
                self.assertEqual(violations(load(name), jobs), [])

    def test_quality_gate_commands_are_preserved(self):
        steps = load("rust.yml")["jobs"]["quality"]["steps"]
        runs = [s.get("run", "") for s in steps]
        for command in (
            "make verify-l10n-catalog",
            "make verify-l10n-bridge",
            "make verify-l10n-sync",
            "make test-l10n-sync",
            "make verify-install-bus",
            "cargo clippy --workspace --all-targets -- -D warnings",
            "make verify",
        ):
            self.assertIn(command, runs)

    def test_suite_runs_in_required_quality_job(self):
        self.assertEqual(suite_step_violations(load("rust.yml")), [])

    def test_semgrep_scan_command_is_preserved(self):
        steps = load("semgrep.yml")["jobs"]["semgrep"]["steps"]
        scan = [s["run"] for s in steps if s.get("name") == "Run Semgrep"]
        self.assertEqual(len(scan), 1)
        self.assertIn("--error", scan[0])
        self.assertIn("--config .semgrep.yaml", scan[0])

    def test_no_workflow_uses_self_hosted_routing(self):
        for path in sorted(WORKFLOWS.glob("*.yml")):
            with self.subTest(workflow=path.name):
                for job_id, job in load(path.name).get("jobs", {}).items():
                    self.assertNotRegex(
                        str(job.get("runs-on")),
                        r"self-hosted|macos-public",
                        f"{path.name}:{job_id}",
                    )

    def test_no_merge_bot_or_privileged_trigger(self):
        for path in sorted(WORKFLOWS.glob("*.yml")):
            with self.subTest(workflow=path.name):
                on = triggers(load(path.name))
                events = on if isinstance(on, dict) else [on] if isinstance(on, str) else on
                self.assertNotIn("pull_request_target", events)


class HostileControls(unittest.TestCase):
    """The checker must reject each deliberately broken variant of the real file."""

    def mutated(self, name, mutate):
        wf = copy.deepcopy(load(name))
        mutate(wf)
        return violations(wf, REQUIRED[name])

    def assertRejects(self, mutate, needle):
        for name in REQUIRED:
            with self.subTest(workflow=name):
                self.assertTrue(
                    any(needle in v for v in self.mutated(name, mutate)),
                    f"{needle!r} not detected in {name}",
                )

    def suite_mutated(self, mutate):
        wf = copy.deepcopy(load("rust.yml"))
        mutate(wf["jobs"]["quality"]["steps"])
        return suite_step_violations(wf)

    def test_absent_suite_step_is_rejected(self):
        def mutate(steps):
            steps[:] = [s for s in steps if s.get("name") != SUITE_STEP]

        self.assertTrue(self.suite_mutated(mutate))

    def test_conditional_or_failing_suite_step_is_rejected(self):
        for key, value in (("if", "false"), ("continue-on-error", True)):
            with self.subTest(key=key):
                def mutate(steps, key=key, value=value):
                    next(s for s in steps if s.get("name") == SUITE_STEP)[key] = value

                self.assertTrue(self.suite_mutated(mutate))

    def test_suite_step_after_compiler_gate_is_rejected(self):
        def mutate(steps):
            i = next(i for i, s in enumerate(steps) if s.get("name") == SUITE_STEP)
            steps.append(steps.pop(i))

        self.assertTrue(self.suite_mutated(mutate))

    def test_unpinned_or_global_install_is_rejected(self):
        def mutate(steps):
            step = next(s for s in steps if s.get("name") == SUITE_STEP)
            step["run"] = step["run"].replace("PyYAML==6.0.3", "PyYAML")

        self.assertTrue(self.suite_mutated(mutate))

    def test_missing_merge_group_is_rejected(self):
        self.assertRejects(lambda wf: triggers(wf).pop("merge_group", None), "merge_group")

    def test_wrong_merge_group_types_are_rejected(self):
        self.assertRejects(
            lambda wf: triggers(wf).update(merge_group={"types": ["destroyed"]}),
            "merge_group",
        )

    def test_unconditional_cancel_is_rejected(self):
        self.assertRejects(
            lambda wf: wf["concurrency"].update({"cancel-in-progress": True}),
            "cancel-in-progress",
        )

    def test_ref_only_concurrency_group_is_rejected(self):
        self.assertRejects(
            lambda wf: wf["concurrency"].update({"group": "${{ github.ref }}"}),
            "github.event_name",
        )

    def test_path_filter_is_rejected(self):
        self.assertRejects(
            lambda wf: triggers(wf)["pull_request"].update({"paths": ["docs/**"]}),
            "path filters",
        )

    def test_pull_request_target_is_rejected(self):
        self.assertRejects(
            lambda wf: triggers(wf).update(pull_request_target={}),
            "pull_request_target",
        )

    def test_self_hosted_runner_is_rejected(self):
        def mutate(wf):
            for job in wf["jobs"].values():
                job["runs-on"] = ["self-hosted", "macos-public"]

        self.assertRejects(mutate, "self-hosted")

    def test_renamed_required_job_is_rejected(self):
        def mutate(wf):
            next(iter(wf["jobs"].values()))["name"] = "Renamed"

        self.assertRejects(mutate, "must keep name")

    def test_continue_on_error_is_rejected(self):
        def mutate(wf):
            next(iter(wf["jobs"].values()))["continue-on-error"] = True

        self.assertRejects(mutate, "continue-on-error")


if __name__ == "__main__":
    unittest.main()
