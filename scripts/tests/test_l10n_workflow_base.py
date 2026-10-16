"""Run the workflow's real base-selection shell block against a fake git."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = Path(os.environ.get("WORKFLOWS_DIR", ROOT / ".github/workflows"))


def base_selection_script(workflow="l10n.yml"):
    lines = (WORKFLOWS / workflow).read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if "Fetch the base the prose ratchet measures against" in line)
    run = next(i for i in range(start, len(lines)) if lines[i].strip() == "run: |")
    indent = len(lines[run]) - len(lines[run].lstrip()) + 2
    script = []
    for line in lines[run + 1:]:
        if line.strip() and not line.startswith(" " * indent):
            break
        script.append(line[indent:])
    return "\n".join(script)

HOSTILE = 'x"; touch pwned; $(touch pwned) `touch pwned`'


class LocalizationWorkflowBaseTests(unittest.TestCase):
    workflow = "l10n.yml"

    def run_selection(self, event, ref, before, base="main", queue_base="", script=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git = root / "git"
            git.write_text(
                f"#!{sys.executable} -E\n"
                "import json,os,sys\n"
                "from pathlib import Path\n"
                "with Path(os.environ['GIT_TEST_LOG']).open('a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\n"
            )
            git.chmod(0o755)
            output = root / "github-env"
            output.touch()
            log = root / "git-args"
            env = {**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"],
                   "GITHUB_ENV": str(output), "GIT_TEST_LOG": str(log),
                   "EVENT_NAME": event, "REF_NAME": ref, "BEFORE_SHA": before, "BASE_REF": base,
                   "MERGE_GROUP_BASE_SHA": queue_base}
            done = subprocess.run(["/bin/bash", "-e", "-c", script or base_selection_script(self.workflow)],
                                  env=env, cwd=root, text=True, capture_output=True, check=False, timeout=10)
            calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            self.assertFalse((root / "pwned").exists(), "ref data reached the shell as code")
            return done.returncode, output.read_text(), calls

    def test_main_push_compares_with_the_immutable_before_commit(self):
        before = "1" * 40
        code, output, calls = self.run_selection("push", "main", before)
        self.assertEqual(code, 0)
        self.assertIn(f"L10N_BASE={before}\n", output)
        self.assertTrue(any(before in call for call in calls), calls)
        self.assertFalse(any("+refs/heads/main:refs/remotes/origin/main" in call for call in calls))

    def test_main_push_without_a_real_before_commit_refuses(self):
        for before in ["", "0" * 40, "not-a-commit"]:
            code, output, calls = self.run_selection("push", "main", before)
            self.assertNotEqual(code, 0)
            self.assertEqual(output, "")
            self.assertEqual(calls, [])

    def test_pull_request_retains_its_actual_stack_destination(self):
        base = "feat/humanize-codescribe-assets"
        code, output, calls = self.run_selection("pull_request", "170/merge", "", base)
        self.assertEqual(code, 0)
        self.assertIn(f"L10N_BASE=origin/{base}\n", output)
        self.assertTrue(any(f"+refs/heads/{base}:refs/remotes/origin/{base}" in call for call in calls))

    def test_merge_group_compares_with_the_immutable_queue_base(self):
        queue_base = "a" * 40
        code, output, calls = self.run_selection(
            "merge_group", "gh-readonly-queue/main/pr-170-" + "b" * 40, "", "main", queue_base)
        self.assertEqual(code, 0)
        self.assertEqual(output, f"L10N_BASE={queue_base}\n")
        self.assertEqual(calls, [["fetch", "--no-tags", "--depth=1", "origin", queue_base]])

    def test_merge_group_without_a_valid_queue_base_refuses_before_any_fetch(self):
        for queue_base in ["", "0" * 40, "A" * 40, "abc123", "a" * 39, "a" * 41, "main", "a" * 40 + "\n; true"]:
            code, output, calls = self.run_selection(
                "merge_group", "gh-readonly-queue/main/pr-1-x", "", "main", queue_base)
            self.assertNotEqual(code, 0, queue_base)
            self.assertEqual(output, "", queue_base)
            self.assertEqual(calls, [], queue_base)

    def test_side_effect_detector_sees_an_interpolated_ref(self):
        # Negative control: if the ref were pasted into `run:` the way
        # `${{ github.base_ref }}` would be, the same payload executes in the
        # private temp cwd and the detector in run_selection fires.
        payload = 'x"; touch pwned; echo "'  # balanced, so the pasted script still parses
        unsafe = base_selection_script(self.workflow).replace("$BASE_REF", payload)
        with self.assertRaises(AssertionError) as caught:
            self.run_selection("pull_request", "1/merge", "", "main", script=unsafe)
        self.assertIn("ref data reached the shell as code", str(caught.exception))
        self.assertFalse((ROOT / "pwned").exists())

    def test_hostile_branch_names_stay_data(self):
        hostile = HOSTILE
        code, output, calls = self.run_selection("pull_request", "1/merge", "", hostile)
        self.assertEqual(code, 0)
        self.assertEqual(output, f"L10N_BASE=origin/{hostile}\n")
        self.assertEqual(calls, [["fetch", "--no-tags", "--depth=1", "origin",
                                  f"+refs/heads/{hostile}:refs/remotes/origin/{hostile}"]])
        code, output, calls = self.run_selection("push", hostile, "", hostile)
        self.assertEqual(code, 0)


class RustQualityWorkflowBaseTests(LocalizationWorkflowBaseTests):
    workflow = "rust.yml"


if __name__ == "__main__":
    unittest.main()
