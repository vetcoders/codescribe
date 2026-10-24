"""Hermetic entrypoint checks for the system speaker and authentication argv."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import runpy
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "cs_say_entry_demux", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class SayEntryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.record = self.root / "oauth-argv.json"
        self.grok = self.root / "grok"
        self.grok.write_text(
            f"#!{sys.executable}\nimport argparse,json,sys\n"
            "parser=argparse.ArgumentParser()\n"
            "parser.add_argument('command', choices=['login'])\n"
            "modes=parser.add_mutually_exclusive_group(required=True)\n"
            "modes.add_argument('--oauth', action='store_true')\n"
            "modes.add_argument('--device-auth', action='store_true')\n"
            "parser.parse_args()\n"
            f"open({str(self.record)!r}, 'w').write(json.dumps(sys.argv[1:]))\n"
        )
        self.grok.chmod(0o755)
        self.command = [
            sys.executable,
            str(Path(__file__).resolve().parents[1] / "cs-say"),
        ]
        self.environment = dict(os.environ, PATH=str(self.root))

    def system_reply(self, *flags, env=None):
        class ExecCaptured(Exception):
            pass

        environment = {"CS_SAY_VOICE": "", "CS_SAY_SPEED": "", "CS_SAY_TTS_VENDOR": ""}
        environment.update(env or {})
        with (
            patch.dict(os.environ, environment),
            patch.object(sys, "argv", [self.command[1], "Gotowe.", *flags]),
            patch("os.execv", side_effect=ExecCaptured) as execute,
        ):
            with self.assertRaises(ExecCaptured):
                runpy.run_path(self.command[1], run_name="__main__")
            forwarded = execute.call_args.args[1]
            self.assertEqual(Path(forwarded[1]).name, "cs-bus")
            with (
                patch.object(sys, "argv", forwarded[1:]),
                patch.object(DEMUX, "bridge_home", return_value=self.root / "bridge"),
                patch.object(DEMUX, "bus_path", return_value=self.root / "bus.jsonl"),
                patch.object(DEMUX, "publish_reply_event", return_value={
                    "stream_dev": 1, "stream_inode": 1, "offset": 0,
                    "length": 1, "stream_id": "entry-fixture",
                }) as publish,
                patch.object(DEMUX, "speak_published_reply", return_value=0),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(DEMUX.main(), 0)
            return publish.call_args.args[1]

    def test_text_alone_selects_codescribe_carina_without_an_agent_profile(self):
        home = self.root / "bridge"
        home.mkdir()
        (home / "voices.json").write_text(json.dumps({"profiles": {
            "codescribe": {"voice": "rex", "speed": 1.9, "provider": "openai"}
        }}))
        reply = self.system_reply()
        self.assertEqual((reply["name"], reply["voice"], reply["tts_vendor"], reply["speed"]),
                         ("Codescribe", "carina", "xai", 1.0))
        self.assertEqual((reply["provider"], reply["provider_session_id"], reply["system"]),
                         ("codescribe", "system", True))
        self.assertEqual(reply["association"], "unsolicited")

    def test_environment_overrides_system_defaults(self):
        reply = self.system_reply(env={
            "CS_SAY_VOICE": "nova", "CS_SAY_SPEED": "1.4", "CS_SAY_TTS_VENDOR": "openai",
        })
        self.assertEqual((reply["voice"], reply["speed"], reply["tts_vendor"]),
                         ("nova", 1.4, "openai"))

    def test_explicit_flags_override_environment(self):
        reply = self.system_reply("--name", "Notice", "--voice", "rex", "--speed", "1.2",
                                  "--tts-vendor", "xai", env={
            "CS_SAY_VOICE": "nova", "CS_SAY_SPEED": "invalid", "CS_SAY_TTS_VENDOR": "invalid",
        })
        self.assertEqual((reply["name"], reply["voice"], reply["speed"], reply["tts_vendor"]),
                         ("Notice", "rex", 1.2, "xai"))

    def test_system_environment_does_not_change_agent_defaults(self):
        reply = self.system_reply("--provider", "codex", "--session", "agent", "--name", "agent",
                                  env={"CS_SAY_VOICE": "carina", "CS_SAY_SPEED": "invalid",
                                       "CS_SAY_TTS_VENDOR": "invalid"})
        self.assertNotIn("system", reply)
        self.assertEqual((reply["voice"], reply["speed"], reply["tts_vendor"]),
                         (DEMUX.DEFAULT_VOICE, DEMUX.DEFAULT_SPEECH_SPEED, "xai"))

    def test_invalid_system_environment_refuses_before_publication(self):
        for env in ({"CS_SAY_SPEED": "bad"}, {"CS_SAY_SPEED": "0"},
                    {"CS_SAY_SPEED": "nan"}, {"CS_SAY_SPEED": "inf"},
                    {"CS_SAY_VOICE": " "}, {"CS_SAY_TTS_VENDOR": "other"}):
            with (self.subTest(env=env), self.assertRaises(SystemExit) as result,
                  contextlib.redirect_stderr(io.StringIO())):
                self.system_reply(env=env)
            self.assertEqual(result.exception.code, 2)
        self.assertFalse((self.root / "bridge/runtime").exists())

    def test_login_selects_one_grok_auth_mode_without_starting_the_app_or_agent(self):
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
            json.loads(self.record.read_text()), ["login", "--device-auth"]
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

    def test_help_starts_with_the_system_voice_form(self):
        result = subprocess.run(self.command + ["--help"], env=self.environment,
                                capture_output=True, text=True, check=True)
        self.assertTrue(result.stdout.startswith("cs-say TEXT — Codescribe system voice"))
        for word in ("Carina", "1.0", "CS_SAY_VOICE", "CS_SAY_SPEED", "CS_SAY_TTS_VENDOR"):
            self.assertIn(word, result.stdout)

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
