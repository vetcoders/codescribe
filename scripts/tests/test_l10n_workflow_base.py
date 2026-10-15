"""Run the workflow's real base-selection shell block against a fake git."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


def base_selection_script():
    lines = (ROOT / ".github/workflows/l10n.yml").read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if "Fetch the base the prose ratchet measures against" in line)
    run = next(i for i in range(start, len(lines)) if lines[i].strip() == "run: |")
    indent = len(lines[run]) - len(lines[run].lstrip()) + 2
    script = []
    for line in lines[run + 1:]:
        if line.strip() and not line.startswith(" " * indent):
            break
        script.append(line[indent:])
    return "\n".join(script)


class LocalizationWorkflowBaseTests(unittest.TestCase):
    def run_selection(self, event, ref, before, base="main"):
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
                   "EVENT_NAME": event, "REF_NAME": ref, "BEFORE_SHA": before, "BASE_REF": base}
            done = subprocess.run(["/bin/bash", "-e", "-c", base_selection_script()],
                                  env=env, text=True, capture_output=True, check=False, timeout=10)
            calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
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


if __name__ == "__main__":
    unittest.main()
