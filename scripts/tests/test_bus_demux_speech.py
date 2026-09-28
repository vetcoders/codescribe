"""Fixed-origin speech transport; no credentials, network or speaker access."""

import http.client
import importlib.util
import json
import ssl
from pathlib import Path
import sys
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
        self.assertEqual(DEMUX._speak_xai("hello", "leo", 1.0), (True, None))
        self.factory.assert_called_once_with()
        self.opener.add_handler.assert_called_once()
        handler = self.opener.add_handler.call_args.args[0]
        self.assertEqual(handler._context.verify_mode, ssl.CERT_REQUIRED)
        self.assertTrue(handler._context.check_hostname)
        args, kwargs = self.opener.open.call_args
        self.assertEqual(args, ("https://api.x.ai/v1/tts",))
        self.assertEqual(kwargs["timeout"], 60)
        self.assertEqual(json.loads(kwargs["data"])["text"], "hello")
        self.response_context.__exit__.assert_called_once()
        self.assertTrue(paths)
        self.assertFalse(paths[0].exists())

    def test_redirects_and_errors_never_read_or_play_audio(self):
        for status in (301, 302, 307, 401, 500):
            with self.subTest(status=status):
                self.opener.reset_mock()
                self.response.status = status
                self.assertEqual(
                    DEMUX._speak_xai("hello", "leo", 1.0),
                    (False, f"tts request failed ({status})"),
                )
                self.response.read.assert_not_called()
                self.play.assert_not_called()
                self.opener.open.assert_called_once()
                self.response_context.__exit__.assert_called_once()

    def test_transport_failure_does_not_play_audio(self):
        self.opener.open.side_effect = http.client.RemoteDisconnected()
        self.assertEqual(
            DEMUX._speak_xai("hello", "leo", 1.0),
            (False, "tts request failed (RemoteDisconnected)"),
        )
        self.play.assert_not_called()


if __name__ == "__main__":
    unittest.main()
