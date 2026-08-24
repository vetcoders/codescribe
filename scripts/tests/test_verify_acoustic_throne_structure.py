from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "verify-acoustic-throne-structure.py"
SPEC = importlib.util.spec_from_file_location("acoustic_structure_verifier", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class OccurrenceScopeTests(unittest.TestCase):
    def test_forbidden_scan_includes_test_code_but_not_non_code(self) -> None:
        payload = {
            "occurrences": [
                {
                    "file": "core/live.rs",
                    "scope_classification": "production",
                    "match_role": "reference",
                },
                {
                    "file": "tests/oracle.rs",
                    "scope_classification": "test",
                    "match_role": "import",
                },
                {
                    "file": "tests/fixture.json",
                    "scope_classification": "test",
                    "match_role": "string_literal",
                },
                {
                    "file": "core/live.rs",
                    "scope_classification": "production",
                    "match_role": "comment",
                },
            ]
        }

        files = {row["file"] for row in VERIFIER.forbidden_occurrences(payload)}

        self.assertEqual(files, {"core/live.rs", "tests/oracle.rs"})


class RustModuleResolutionTests(unittest.TestCase):
    def test_reports_missing_module_and_accepts_standard_module_file(self) -> None:
        with tempfile.TemporaryDirectory() as raw_repo:
            repo = Path(raw_repo)
            core = repo / "core"
            core.mkdir()
            (core / "mod.rs").write_text("mod present;\nmod missing;\n")
            (core / "present.rs").write_text("pub fn marker() {}\n")

            unresolved = VERIFIER.unresolved_module_declarations(repo)

        self.assertEqual(len(unresolved), 1)
        self.assertEqual(unresolved[0]["module"], "missing")
        self.assertEqual(unresolved[0]["file"], "core/mod.rs")

    def test_honours_explicit_path_attribute(self) -> None:
        with tempfile.TemporaryDirectory() as raw_repo:
            repo = Path(raw_repo)
            app = repo / "app"
            shared = repo / "shared"
            app.mkdir()
            shared.mkdir()
            (app / "lib.rs").write_text(
                '#[path = "../shared/oracle.rs"]\n#[cfg(test)]\nmod oracle;\n'
            )
            (shared / "oracle.rs").write_text("pub fn marker() {}\n")

            unresolved = VERIFIER.unresolved_module_declarations(repo)

        self.assertEqual(unresolved, [])


if __name__ == "__main__":
    unittest.main()
