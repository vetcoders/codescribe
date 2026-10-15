"""Independent refusal controls for admission of the localization gate."""

import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "l10n_admission_census", ROOT / "scripts/l10n-bridge-census.py"
)
census = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(census)


class LocalizationAdmissionTests(unittest.TestCase):
    def run_census(self, answers, args):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(census, "at_ref", lambda ref, path: answers.get(path)):
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = census.main(["l10n-bridge-census.py", *args])
        return code, out.getvalue(), err.getvalue()

    def test_missing_base_classification_refuses_the_gate(self):
        code, _, err = self.run_census({}, ["--base", "missing/base"])
        self.assertNotEqual(code, 0, err)
        self.assertIn("cannot read", err)

    def test_missing_base_bindings_refuses_even_an_unchanged_prose_list(self):
        answers = {census.CLASSIFICATION: census.CLASSIFICATION.read_text()}
        code, _, err = self.run_census(answers, ["--base", "missing/bindings"])
        self.assertNotEqual(code, 0, err)
        self.assertIn("bindings", err)

    def test_make_treats_a_base_ref_as_data_without_executing_shell_metacharacters(self):
        with tempfile.TemporaryDirectory(prefix="l10n-refusal-") as directory:
            marker = Path(directory) / "unexpected-command"
            env = dict(os.environ)
            env["L10N_BASE"] = f"missing/base; /usr/bin/touch {marker}; #"
            done = subprocess.run(
                ["make", "verify-l10n-bridge"], cwd=ROOT, env=env,
                text=True, capture_output=True, check=False, timeout=30,
            )
            self.assertFalse(marker.exists(), done.stdout + done.stderr)
            self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_empty_environment_base_does_not_disable_the_ratchet(self):
        with mock.patch.dict(os.environ, {"L10N_BASE": ""}):
            code, _, _ = self.run_census({}, ["--base-env"])
        self.assertNotEqual(code, 0)

    def test_make_does_not_expand_make_functions_in_an_environment_base(self):
        with tempfile.TemporaryDirectory(prefix="l10n-refusal-") as directory:
            marker = Path(directory) / "unexpected-make-command"
            env = dict(os.environ)
            env["L10N_BASE"] = f"missing/$(shell /usr/bin/touch {marker})"
            done = subprocess.run(
                ["make", "verify-l10n-bridge"], cwd=ROOT, env=env,
                text=True, capture_output=True, check=False, timeout=30,
            )
            self.assertFalse(marker.exists(), done.stdout + done.stderr)
            self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_environment_base_reaches_git_as_a_literal_ref(self):
        answers = {
            census.CLASSIFICATION: census.CLASSIFICATION.read_text(),
            census.BINDINGS: census.BINDINGS.read_text(),
        }
        calls = []

        def read_ref(ref, path):
            calls.append(ref)
            return answers[path]

        ref = "refs/heads/a;literal"
        with mock.patch.dict(os.environ, {"L10N_BASE": ref}):
            with mock.patch.object(census, "at_ref", read_ref):
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    code = census.main(["l10n-bridge-census.py", "--base-env"])
        self.assertEqual(code, 0)
        self.assertEqual(calls, [ref, ref])


if __name__ == "__main__":
    unittest.main()
