"""Run the actual workflow classifier with synthetic Git output, without GitHub."""

from contextlib import redirect_stderr, redirect_stdout
import io
import os
from pathlib import Path
import textwrap
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]


def classifier_source(workflow):
    lines = workflow.splitlines()
    start = next(i for i, line in enumerate(lines) if line.strip() == "python3 - <<'PY'")
    end = next(i for i in range(start + 1, len(lines)) if lines[i].strip() == "PY")
    return textwrap.dedent("\n".join(lines[start + 1 : end]))


POLICY = classifier_source((ROOT / ".github/workflows/pr-risk-gate.yml").read_text())


class PrRiskVocabularyTests(unittest.TestCase):
    # Counterexample fixture terms are assembled here; they are not application
    # identifiers or a replacement spelling sent to the provider.
    word = "leg" + "acy"
    wire_tier = word + "-tier"
    owner = "core/llm/account_auth/google.rs"

    def evaluate(self, added, path=None):
        path = path or self.owner
        diff = (
            f"diff --git a/{path} b/{path}\n"
            f"--- a/{path}\n+++ b/{path}\n@@ -0,0 +1 @@\n+{added}\n"
        )
        output, error = io.StringIO(), io.StringIO()
        status = 0
        with (
            mock.patch.dict(
                os.environ,
                {
                    "BASE_SHA": "a" * 40,
                    "HEAD_SHA": "b" * 40,
                    "PR_BODY": "## Why\nA behavior fix.\n## Testing\nRegressions.\n"
                    "## System recap\nRisk: high\n",
                },
                clear=True,
            ),
            mock.patch("subprocess.check_output", side_effect=[path + "\n", diff]) as git,
            redirect_stdout(output),
            redirect_stderr(error),
        ):
            try:
                exec(compile(POLICY, "pr-risk-gate.yml:classifier", "exec"), {})
            except SystemExit as stopped:
                status = stopped.code
            self.assertEqual(git.call_count, 2)
        return status, output.getvalue(), error.getvalue()

    def assert_rejected(self, added, path=None):
        status, _, error = self.evaluate(added, path)
        self.assertEqual(status, 1)
        self.assertIn("Banned vocabulary", error)

    def test_exact_wire_tier_is_allowed_in_its_owner(self):
        for indentation in ["", "    ", "\t"]:
            with self.subTest(indentation=repr(indentation)):
                status, _, error = self.evaluate(indentation + '"' + self.wire_tier + '"')
                self.assertEqual(status, 0, error)

    def test_the_same_literal_in_another_module_is_rejected(self):
        self.assert_rejected('"' + self.wire_tier + '"', "core/llm/other.rs")

    def test_comment_about_the_wire_tier_is_not_an_exception(self):
        self.assert_rejected("// " + self.wire_tier)

    def test_identifier_is_not_an_exception(self):
        self.assert_rejected("struct " + self.word.title() + " {}")

    def test_literal_with_additional_code_is_not_an_exception(self):
        self.assert_rejected('"' + self.wire_tier + '"; // extra code')

    def test_another_value_in_the_same_owner_is_rejected(self):
        self.assert_rejected('"' + self.word + '-cache"')

    def test_changed_case_is_not_the_external_protocol_value(self):
        self.assert_rejected('"' + self.wire_tier.upper() + '"')

    def test_unrelated_source_still_passes(self):
        status, _, error = self.evaluate("pub fn start() {}")
        self.assertEqual(status, 0, error)


if __name__ == "__main__":
    unittest.main()
