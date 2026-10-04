"""No browser, account mutation or network: exercise the OAuth entry argv."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import runpy
from unittest.mock import patch


class SayEntryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.record = self.root / "oauth-argv.json"
        self.grok = self.root / "grok"
        self.grok.write_text(
            f"#!{sys.executable}\nimport json,sys\n"
            f"open({str(self.record)!r}, 'w').write(json.dumps(sys.argv[1:]))\n"
        )
        self.grok.chmod(0o755)
        self.command = [
            sys.executable,
            str(Path(__file__).resolve().parents[1] / "cs-say"),
        ]
        self.environment = dict(os.environ, PATH=str(self.root))

    def test_login_uses_oauth_command_without_starting_the_app_or_agent(self):
        subprocess.run(
            self.command + ["auth", "--provider", "xai", "--login-type", "oauth"],
            env=self.environment,
            check=True,
        )
        self.assertEqual(json.loads(self.record.read_text()), ["login", "--oauth"])
        subprocess.run(
            self.command + ["auth", "--provider", "xai", "--login-type", "device-code"],
            env=self.environment,
            check=True,
        )
        self.assertEqual(
            json.loads(self.record.read_text()), ["login", "--oauth", "--device-auth"]
        )

    def test_key_login_prompts_without_a_secret_in_argv_for_every_provider(self):
        class ExecCaptured(Exception):
            pass

        for provider in ("xai", "openai", "deepinfra", "custom"):
            with (
                self.subTest(provider=provider),
                patch.object(
                    sys,
                    "argv",
                    [
                        self.command[1],
                        "auth",
                        "--provider",
                        provider,
                        "--login-type",
                        "key",
                    ],
                ),
                patch("os.execv", side_effect=ExecCaptured) as execute,
            ):
                with self.assertRaises(ExecCaptured):
                    runpy.run_path(self.command[1], run_name="__main__")
                execute.assert_called_once_with(
                    "/usr/bin/security",
                    [
                        "/usr/bin/security",
                        "add-generic-password",
                        "-U",
                        "-s",
                        "com.vetcoders.codescribe",
                        "-a",
                        f"LLM_{provider.upper()}_API_KEY",
                        "-w",
                    ],
                )
        self.assertFalse(self.record.exists())

    def test_unsupported_oauth_fails_before_launch_or_credentials(self):
        for provider in ("openai", "deepinfra", "custom"):
            for mode in ("oauth", "device-code"):
                result = subprocess.run(
                    self.command
                    + ["auth", "--provider", provider, "--login-type", mode],
                    env=self.environment,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 2)
                self.assertIn("supports --login-type key", result.stderr)
        self.assertFalse(self.record.exists())

    def test_help_describes_authentication_and_actual_provider_support(self):
        for arguments in (["--help"], ["auth", "--help"]):
            result = subprocess.run(
                self.command + arguments,
                env=self.environment,
                capture_output=True,
                text=True,
                check=True,
            )
            for word in (
                "oauth",
                "device-code",
                "Keychain",
                "Grok",
                "openai",
                "custom",
            ):
                self.assertIn(word, result.stdout)
        self.assertFalse(self.record.exists())

    def test_unknown_option_refuses_before_credentials_or_login(self):
        result = subprocess.run(
            self.command
            + [
                "auth",
                "--provider",
                "xai",
                "--login-type",
                "oauth",
                "--debug-file",
                "/private/token",
            ],
            env=self.environment,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.record.exists())

    def test_missing_oauth_client_reports_dependency_without_installing_it(self):
        self.grok.unlink()
        result = subprocess.run(
            self.command + ["auth", "--provider", "xai", "--login-type", "oauth"],
            env=self.environment,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 3)
        self.assertIn("Grok CLI", result.stderr)


if __name__ == "__main__":
    unittest.main()
