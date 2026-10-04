"""Hermetic registry/parser and real shell-entrypoint regression tests."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]
VALID = '''[meta]
version = "test"
# A quoted key and whitespace are valid TOML, invisible to the old header grep.
  [vars."KNOWN"] # registered
default = ""
description = "fixture"
[vars.Z_2]
default = ""
deprecated = "KNOWN"
'''


class EnvRegistryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in ("docs", "scripts", "core", "app", "bin"):
            (self.root / directory).mkdir()
        for name in ("validate-envs.sh", "validate_env_registry.py"):
            shutil.copyfile(SCRIPTS / name, self.root / "scripts" / name)
        self.registry = self.root / "docs/ENV_REGISTRY.toml"
        self.registry.write_text(VALID)
        (self.root / ".env.example").write_text("# fixture\nKNOWN=fixture\nZ_2=old\n")
        (self.root / "core/fixture.rs").write_text('env::var("KNOWN");\n')

    def parser(self, *prefix):
        return subprocess.run(
            [sys.executable, *prefix, str(self.root / "scripts/validate_env_registry.py"),
             str(self.registry)],
            cwd=self.root, capture_output=True, text=True, check=False,
        )

    def shell(self, *args):
        return subprocess.run(
            ["bash", str(self.root / "scripts/validate-envs.sh"), *args],
            cwd=self.root, capture_output=True, text=True, check=False,
        )

    def test_valid_names_and_deprecated_entries(self):
        result = self.parser()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "KNOWN\nZ_2\n")

    def assert_invalid(self, content):
        if content is None:
            self.registry.unlink()
        else:
            self.registry.write_text(content)
        result = self.parser()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("ERROR: invalid env registry", result.stderr)
        # Even a valid prefix must not produce output or its parent directory.
        output = self.root / "output/e2e.env"
        for args in ((), ("--fix",), ("--env-example",),
                     ("--emit-e2e-env", str(output))):
            with self.subTest(args=args):
                result = self.shell(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("ERROR: invalid env registry", result.stderr)
                self.assertNotIn("Scanning Rust", result.stdout)
                self.assertFalse(output.parent.exists())
        output.parent.mkdir()
        output.write_text("existing output\n")
        result = self.shell("--emit-e2e-env", str(output))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output.read_text(), "existing output\n")

    def test_duplicate_table(self):
        self.assert_invalid(VALID + '\n[vars.KNOWN]\ndefault = ""\n')

    def test_syntax_error_after_valid_tables(self):
        self.assert_invalid(VALID + '\n[broken\n')

    def test_duplicate_field(self):
        self.assert_invalid(VALID + '\ndefault = "again"\n')

    def test_missing_registry(self):
        self.assert_invalid(None)

    def test_invalid_variable_table_shapes(self):
        for content in ('[meta]\nversion = "test"\n', '[vars]\n',
                        '[vars]\nKNOWN = "scalar"\n', '[[vars.KNOWN]]\n',
                        '[vars."BAD.NAME"]\n', '[vars."BAD\\nNAME"]\n'):
            with self.subTest(content=content):
                self.registry.write_text(content)
                result = self.parser()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")

    def test_missing_tomllib_fails_clearly(self):
        # Simulate a Python installation without tomllib, without installing one.
        (self.root / "scripts/tomllib.py").write_text('raise ImportError("fixture")\n')
        result = self.parser()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("Python 3.11+ with stdlib tomllib is required", result.stderr)
        output = self.root / "output/e2e.env"
        result = self.shell("--emit-e2e-env", str(output))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Python 3.11+", result.stderr)
        self.assertFalse(output.parent.exists())

    def test_shell_valid_example_and_emission(self):
        example = self.root / "custom example.env"
        example.write_text("# ignored\nKNOWN=fixture\n\nZ_2=old\n")
        output = self.root / "output/e2e.env"
        result = self.shell("--env-example", "--env-example-path", str(example),
                            "--emit-e2e-env", str(output))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output.read_text(), "KNOWN=fixture\nZ_2=old\n")

    def test_shell_missing_variable_and_fix(self):
        (self.root / "core/fixture.rs").write_text('env::var("MISSING");\n')
        for args in ((), ("--fix",)):
            with self.subTest(args=args):
                result = self.shell(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("[vars.MISSING]", result.stdout)
        self.assertEqual(self.registry.read_text(), VALID)

    def test_shell_unknown_example_variable(self):
        (self.root / ".env.example").write_text("UNKNOWN=value\n")
        result = self.shell("--env-example")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown var", result.stdout)

    def test_repository_registry_parses(self):
        self.registry.write_bytes((SCRIPTS.parent / "docs/ENV_REGISTRY.toml").read_bytes())
        result = self.parser()
        self.assertEqual(result.returncode, 0, result.stderr)
        names = result.stdout.splitlines()
        self.assertGreater(len(names), 0)
        for name in ("LLM_XAI_API_KEY", "LLM_ANTHROPIC_API_KEY", "CODESCRIBE_EMBED_MODEL"):
            self.assertEqual(names.count(name), 1)


if __name__ == "__main__":
    unittest.main()
