"""Fixed-origin speech transport; no credentials, network or speaker access."""

import contextlib
import http.client
import importlib.util
import io
import json
import ssl
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch
import wave


SPEC = importlib.util.spec_from_file_location(
    "bus_demux_speech_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class SpeechTransportTests(unittest.TestCase):
    def setUp(self):
        self.key = patch.object(DEMUX, "_xai_speech_key", return_value="test-only-key")
        self.key.start()
        self.addCleanup(self.key.stop)
        self.transport = patch("urllib.request.OpenerDirector")
        self.factory = self.transport.start()
        self.addCleanup(self.transport.stop)
        self.opener = self.factory.return_value
        self.response_context = self.opener.open.return_value
        self.response = self.response_context.__enter__.return_value
        self.response.status = 200
        self.response.read.return_value = b"\x00\x80\x00\x00\xff\x7f"
        self.playback = patch("subprocess.run")
        self.play = self.playback.start()
        self.addCleanup(self.playback.stop)
        self.play.return_value = MagicMock(returncode=0)

    def test_pcm_uses_fixed_tls_origin_and_removes_playback_file(self):
        paths = []

        def play(command, **kwargs):
            self.assertEqual(command[0], "afplay")
            paths.append(Path(command[1]))
            with wave.open(command[1], "rb") as audio:
                self.assertEqual(audio.getframerate(), 24000)
                self.assertEqual(audio.getnchannels(), 1)
                self.assertEqual(audio.getsampwidth(), 2)
                self.assertEqual(audio.readframes(3), self.response.read.return_value)
            return MagicMock(returncode=0)

        self.play.side_effect = play
        self.assertEqual(DEMUX._speak_xai("hello", "leo", 1.0), (True, None, None))
        self.factory.assert_called_once_with()
        self.opener.add_handler.assert_called_once()
        handler = self.opener.add_handler.call_args.args[0]
        self.assertEqual(handler._context.verify_mode, ssl.CERT_REQUIRED)
        self.assertTrue(handler._context.check_hostname)
        args, kwargs = self.opener.open.call_args
        request = args[0]
        self.assertEqual(request.full_url, "https://api.x.ai/v1/tts")
        self.assertEqual(kwargs["timeout"], 60)
        self.assertEqual(json.loads(request.data)["text"], "hello")
        # Headers must ride on the Request itself: urllib's do_request_ installs
        # a form-urlencoded Content-Type before opener.addheaders are consulted.
        self.assertEqual(request.get_header("Content-type"), "application/json")
        self.assertEqual(request.get_header("Authorization"), "Bearer test-only-key")
        self.response_context.__exit__.assert_called_once()
        self.assertTrue(paths)
        self.assertFalse(paths[0].exists())

    def test_redirects_and_errors_read_a_bounded_body_and_never_play(self):
        expected = {
            301: "http_301",
            302: "http_302",
            307: "http_307",
            401: "credential_rejected",
            500: "http_500",
        }
        for status, reason in expected.items():
            with self.subTest(status=status):
                self.opener.reset_mock()
                self.response.status = status
                self.assertEqual(
                    DEMUX._speak_xai("hello", "leo", 1.0),
                    (False, f"tts request failed ({status})", reason),
                )
                # Only a bounded classification read; the body is never audio.
                self.response.read.assert_called_once_with(DEMUX.TTS_ERROR_BODY_BYTES)
                self.play.assert_not_called()
                self.opener.open.assert_called_once()
                self.response_context.__exit__.assert_called_once()

    def test_transport_failure_does_not_play_audio(self):
        self.opener.open.side_effect = http.client.RemoteDisconnected()
        self.assertEqual(
            DEMUX._speak_xai("hello", "leo", 1.0),
            (False, "tts request failed (RemoteDisconnected)", "network"),
        )
        self.play.assert_not_called()

    def test_refusal_bodies_classify_by_error_code_fields_without_echo(self):
        # xAI answers an unvalidated token and an exhausted spending limit with
        # the same 403; only the body's error fields tell them apart.
        secret = "xai-test-secret-9f8e7d"
        cases = [
            (
                403,
                {
                    "code": "The caller does not have permission",
                    "error": f"The OAuth token could not be validated ({secret}).",
                },
                "credential_rejected",
            ),
            (
                403,
                {
                    "code": "The caller does not have permission",
                    "error": f"Your team {secret} has reached its spending-limit.",
                },
                "quota_exhausted",
            ),
            (
                429,
                {"error": {"code": "insufficient_quota", "message": secret}},
                "quota_exhausted",
            ),
            (
                429,
                {"error": {"code": "rate_limit_exceeded", "message": secret}},
                "http_429",
            ),
            (403, None, "http_403"),
        ]
        for status, body, reason in cases:
            with self.subTest(status=status, reason=reason):
                self.opener.reset_mock()
                self.response.status = status
                self.response.read.return_value = (
                    json.dumps(body).encode() if body else b"<html>denied</html>"
                )
                spoken, error, got = DEMUX._speak_xai("hello", "leo", 1.0)
                self.assertFalse(spoken)
                self.assertEqual(got, reason)
                self.assertEqual(error, f"tts request failed ({status})")
                self.assertNotIn(secret, error)
                self.play.assert_not_called()

    def test_missing_credential_is_its_own_reason(self):
        with patch.object(DEMUX, "_xai_speech_key", return_value=None):
            spoken, _error, reason = DEMUX._speak_xai("hello", "leo", 1.0)
        self.assertFalse(spoken)
        self.assertEqual(reason, "credential_missing")
        self.opener.open.assert_not_called()


class SayReplyTests(unittest.TestCase):
    """``--say`` end to end through main(); the speaker itself is mocked."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "agent-bridge"
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.home.mkdir()
        (self.home / "voices.json").write_text(
            json.dumps({"profiles": {"filip": {"voice": "rex", "speed": 1.1}}})
        )

    def attach_lease(self, session, name):
        lease = DEMUX.SessionLease(
            root=self.home,
            provider="claude-code",
            provider_session_id=session,
            name=name,
            bus=self.bus,
            requested_id=None,
            ttl_seconds=120,
            follow_from_end=True,
        )
        lease.close()

    def say(self, *extra, speaker=(True, None, None)):
        argv = [
            "bus-demux.py",
            "--bus",
            str(self.bus),
            "--bridge-home",
            str(self.home),
            "--provider",
            "claude-code",
            "--say",
            "Zrobione.",
            *extra,
        ]
        out = io.StringIO()
        with patch.object(DEMUX, "_speak_xai", return_value=speaker) as spoken, patch.object(
            sys, "argv", argv
        ), contextlib.redirect_stdout(out):
            code = DEMUX.main()
        return code, json.loads(out.getvalue()), spoken

    def test_name_comes_from_the_lease_when_not_given(self):
        self.attach_lease("session-a", "filip")
        code, reply, spoken = self.say("--session", "session-a")
        self.assertEqual(code, 0)
        self.assertEqual(reply["name"], "filip")
        # The resolved name selects the profile, exactly as an explicit one.
        spoken.assert_called_once_with("Zrobione.", "rex", 1.1)

    def test_explicit_name_still_wins(self):
        self.attach_lease("session-a", "filip")
        code, reply, spoken = self.say("--session", "session-a", "--name", "roman")
        self.assertEqual(code, 0)
        self.assertEqual(reply["name"], "roman")
        spoken.assert_called_once_with("Zrobione.", "leo", DEMUX.DEFAULT_SPEECH_SPEED)

    def test_without_lease_or_name_it_refuses(self):
        with self.assertRaises(SystemExit) as refused, contextlib.redirect_stderr(
            io.StringIO()
        ):
            self.say("--session", "no-lease-here")
        self.assertEqual(refused.exception.code, 2)
        self.assertEqual(self.bus.read_text(), "")

    def test_failed_speech_reports_the_reason_without_the_body(self):
        self.attach_lease("session-a", "filip")
        code, reply, _ = self.say(
            "--session",
            "session-a",
            speaker=(False, "tts request failed (403)", "quota_exhausted"),
        )
        self.assertEqual(code, 5)
        self.assertFalse(reply["spoken"])
        self.assertEqual(reply["reason"], "quota_exhausted")
        row = json.loads(self.bus.read_text())
        self.assertEqual(row["reason"], "quota_exhausted")
        self.assertEqual(row["tts_error"], "tts request failed (403)")


if __name__ == "__main__":
    unittest.main()
