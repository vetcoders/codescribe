"""Fixed-origin speech transport; no credentials, network or speaker access."""

import contextlib
import datetime
import http.client
import importlib.util
import io
import json
import multiprocessing
import os
import fcntl
import ssl
import subprocess
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import ANY, MagicMock, patch
import wave


SPEC = importlib.util.spec_from_file_location(
    "bus_demux_speech_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


def fixture_publish(bus, event, *, bridge_root=None):
    """Isolate speech/interlocks from publication; real CLI has its own gate."""
    raw = (json.dumps(event, ensure_ascii=False) + "\n").encode()
    descriptor = os.open(bus, os.O_RDWR | os.O_CREAT | os.O_APPEND, 0o600)
    with os.fdopen(descriptor, "ab") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        offset = handle.tell()
        handle.write(raw)
        handle.flush()
        os.fsync(handle.fileno())
        metadata = os.fstat(handle.fileno())
    return {"stream_id": "speech-fixture", "stream_dev": metadata.st_dev,
            "stream_inode": metadata.st_ino, "offset": offset, "length": len(raw)}


class SpeechCredentialTests(unittest.TestCase):
    def test_xai_explicit_key_wins_over_oidc_without_reading_other_credentials(self):
        with patch("subprocess.run", return_value=MagicMock(returncode=0, stdout="test-key\n")), patch.object(Path, "read_text", side_effect=AssertionError("must not read OAuth store")):
            self.assertEqual(DEMUX._xai_speech_key(), "test-key")

    def test_only_xai_oidc_session_is_used_when_no_key_is_stored(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            (home / ".grok").mkdir()
            auth = home / ".grok/auth.json"
            auth.write_text(json.dumps({
                "https://other.example::account": {"auth_mode": "oidc", "key": "foreign-test-token"},
                "https://auth.x.ai::account": {"auth_mode": "oidc", "key": "xai-test-token"},
            }))
            with patch.object(Path, "home", return_value=home), patch("subprocess.run", return_value=MagicMock(returncode=44, stdout="")):
                self.assertEqual(DEMUX._xai_speech_key(), "xai-test-token")
                auth.write_text(json.dumps({"https://other.example": {"auth_mode": "oidc", "key": "foreign-test-token"}}))
                self.assertIsNone(DEMUX._xai_speech_key())


class SpeechTransportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        for name, value in [
            ("bridge_home", root / "agent-bridge"),
            ("bus_path", root / "bus.jsonl"),
        ]:
            scoped = patch.object(DEMUX, name, return_value=value)
            scoped.start()
            self.addCleanup(scoped.stop)
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
        self.playback = patch("subprocess.Popen")
        self.play = self.playback.start()
        self.addCleanup(self.playback.stop)
        self.play.return_value = MagicMock()
        self.play.return_value.poll.return_value = 0

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
            player = MagicMock()
            player.poll.return_value = 0
            return player

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


class SpeechCacheTests(unittest.TestCase):
    """Paid syntheses stay on disk; repeats of the same words never call TTS."""

    setUp = SpeechTransportTests.setUp

    def cached(self):
        directory = DEMUX.bridge_home() / "speech"
        if not directory.exists():
            return []
        return sorted(path.name for path in directory.iterdir() if path.suffix in (".wav", ".json"))

    def test_same_words_and_voice_synthesize_once_and_keep_an_aligned_pair(self):
        for _ in range(2):
            self.assertEqual(DEMUX._speak_xai("hello", "leo", 1.0), (True, None, None))
        self.assertEqual(self.opener.open.call_count, 1)
        key = DEMUX.speech_cache_key({"provider": "xai", "voice": "leo", "speed": 1.0,
                                      "language": DEMUX.DEFAULT_TTS_LANGUAGE, "text": "hello"})
        self.assertEqual(self.cached(), [f"{key}.json", f"{key}.wav"])
        directory = DEMUX.bridge_home() / "speech"
        sidecar = json.loads((directory / f"{key}.json").read_text())
        self.assertEqual((sidecar["provider"], sidecar["voice"], sidecar["text"]), ("xai", "leo", "hello"))
        self.assertEqual(sidecar["format"], {"sample_rate": 24000, "encoding": "s16le",
                                             "channels": 1, "samples": 3})
        self.assertEqual((sidecar["alignment"], sidecar["tts_fidelity"]), ("utterance", "unverified"))
        with wave.open(str(directory / f"{key}.wav"), "rb") as audio:
            self.assertEqual(audio.readframes(3), self.response.read.return_value)

    def test_refused_playback_after_synthesis_keeps_the_recording(self):
        for reason in ("take_started", "playback_busy", "stopped"):
            with self.subTest(reason=reason), patch.object(
                    DEMUX, "_play_pcm_24k", return_value=(False, "refused", reason)):
                text = f"kept after {reason}"
                self.assertEqual(DEMUX._speak_xai(text, "leo", 1.0), (False, "refused", reason))
                key = DEMUX.speech_cache_key({"provider": "xai", "voice": "leo", "speed": 1.0,
                                              "language": DEMUX.DEFAULT_TTS_LANGUAGE, "text": text})
                self.assertIn(f"{key}.wav", self.cached())
                self.assertIn(f"{key}.json", self.cached())
        calls = self.opener.open.call_count
        self.assertEqual(DEMUX._speak_xai("kept after stopped", "leo", 1.0), (True, None, None))
        self.assertEqual(self.opener.open.call_count, calls)

    def test_other_voice_speed_or_vendor_is_a_new_synthesis(self):
        for voice, speed in (("leo", 1.0), ("rex", 1.0), ("leo", 1.1), ("leo", 1.0)):
            DEMUX._speak_xai("hello", voice, speed)
        self.assertEqual(self.opener.open.call_count, 3)
        with patch.object(DEMUX, "_openai_speech_key", return_value="test-only-key"):
            DEMUX._speak_openai("hello", "leo", 1.0)
        self.assertEqual(self.opener.open.call_count, 4)
        self.assertEqual(len(self.cached()), 8)

    def test_concurrent_misses_of_one_key_pay_once(self):
        import threading

        calls = []
        started = threading.Event()
        release = threading.Event()

        def synthesize_held():
            calls.append("first")
            started.set()
            self.assertTrue(release.wait(5))
            return b"\x02\x00\x03\x00", None, None

        def synthesize_again():
            calls.append("second")
            return b"\xff\x7f", None, None

        def synthesize_other():
            calls.append("other")
            return b"\x05\x00", None, None

        fields = {"provider": "xai", "voice": "leo", "speed": 1.0, "language": "auto", "text": "once"}
        results = {}

        def speak(label, payload, synthesize):
            results[label] = DEMUX._cached_speech(None, payload, synthesize)

        first = threading.Thread(target=speak, args=("first", fields, synthesize_held))
        first.start()
        self.assertTrue(started.wait(5))
        second = threading.Thread(target=speak, args=("second", fields, synthesize_again))
        second.start()
        other = threading.Thread(target=speak, args=("other", {**fields, "text": "another key"}, synthesize_other))
        other.start()
        other.join(5)
        self.assertFalse(other.is_alive(), "another key must not wait for this one")
        self.assertTrue(first.is_alive() and second.is_alive(), "the same key waits for the first synthesis")
        release.set()
        first.join(5)
        second.join(5)
        self.assertEqual(sorted(calls), ["first", "other"])
        self.assertEqual(results["first"], (b"\x02\x00\x03\x00", None, None))
        self.assertEqual(results["second"], results["first"])

    def test_lock_timeout_synthesizes_instead_of_refusing(self):
        fields = {"provider": "xai", "voice": "leo", "speed": 1.0, "language": "auto", "text": "late"}
        directory = DEMUX.bridge_home() / "speech"
        directory.mkdir(parents=True)
        lock = directory / f"{DEMUX.speech_cache_key(fields)}.lock"
        with os.fdopen(os.open(lock, os.O_RDWR | os.O_CREAT, 0o600), "a+b") as holder:
            fcntl.flock(holder.fileno(), fcntl.LOCK_EX)
            with patch.object(DEMUX, "SPEECH_LOCK_WAIT_SECONDS", 0.1):
                result = DEMUX._cached_speech(None, fields, lambda: (b"\x04\x00", None, None))
        self.assertEqual(result, (b"\x04\x00", None, None))
        self.assertIn(f"{DEMUX.speech_cache_key(fields)}.wav", self.cached())

    def test_failed_synthesis_stores_nothing(self):
        self.response.status = 500
        self.assertFalse(DEMUX._speak_xai("hello", "leo", 1.0)[0])
        self.assertEqual(self.cached(), [])


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
        canonical_bus = patch.object(DEMUX, "bus_path", return_value=self.bus)
        canonical_bus.start()
        self.addCleanup(canonical_bus.stop)
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
        receipt = lease.attach_receipt()
        lease.close()
        return receipt

    def say(self, *extra, speaker=(True, None, None), text="Zrobione."):
        argv = [
            "bus-demux.py",
            "--bus",
            str(self.bus),
            "--bridge-home",
            str(self.home),
            "--provider",
            "claude-code",
            "--say",
            text,
            *extra,
        ]
        out = io.StringIO()
        with (
            patch.object(DEMUX, "_speak_xai", return_value=speaker) as spoken,
            patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
            patch.object(sys, "argv", argv),
            contextlib.redirect_stdout(out),
        ):
            code = DEMUX.main()
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.outcome = rows[-1]
        return code, next(row for row in rows if row["kind"] == "agent_reply"), spoken

    def test_name_comes_from_the_lease_when_not_given(self):
        self.attach_lease("session-a", "filip")
        code, reply, spoken = self.say("--session", "session-a")
        self.assertEqual(code, 0)
        self.assertEqual(reply["name"], "filip")
        # The resolved name selects the profile, exactly as an explicit one.
        spoken.assert_called_once_with(
            "Zrobione.", "rex", 1.1, playback_root=self.home, bus=self.bus, control=ANY
        )

    def test_voice_gets_spoken_words_while_the_reply_keeps_the_written_text(self):
        self.attach_lease("session-a", "filip")
        written = "filip → as: gotowe, commit 0786cc92 w scripts/bus-demux.py:120."
        code, reply, spoken = self.say("--session", "session-a", text=written)
        self.assertEqual(code, 0)
        self.assertEqual(reply["text"], written)
        rows = [json.loads(line) for line in self.bus.read_text().splitlines()]
        self.assertEqual(rows[0]["text"], written)
        spoken.assert_called_once_with(
            "Filip: gotowe, commit w bus demux.", "rex", 1.1,
            playback_root=self.home, bus=self.bus, control=ANY,
        )

    def mute(self, session, muted=True, bus=None):
        args = DEMUX.argparse.Namespace(
            provider="claude-code", session=session, bus=bus or self.bus,
            bridge_home=self.home, mute_agent=muted,
        )
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(DEMUX.set_agent_muted(args), 0)
        return json.loads(out.getvalue())

    def test_muted_reply_keeps_text_without_synthesis_and_unmute_allows_next_reply(self):
        self.attach_lease("session-a", "filip")
        self.mute("session-a")
        code, reply, spoken = self.say("--session", "session-a")
        self.assertEqual(code, 0)
        spoken.assert_not_called()
        self.assertEqual(self.outcome["state"], "refused")
        self.assertEqual(self.outcome["reason"], "muted")
        rows = [json.loads(line) for line in self.bus.read_text().splitlines()]
        self.assertEqual(rows[0]["reply_id"], reply["reply_id"])
        self.assertEqual(rows[0]["text"], "Zrobione.")
        self.mute("session-a", False)
        code, _, spoken = self.say("--session", "session-a")
        self.assertEqual(code, 0)
        spoken.assert_called_once()

    def test_same_name_other_session_stays_audible_and_wrong_bus_cannot_mute(self):
        self.attach_lease("session-a", "filip")
        self.attach_lease("session-b", "filip")
        self.mute("session-a")
        code, _, spoken = self.say("--session", "session-b")
        self.assertEqual(code, 0)
        spoken.assert_called_once()
        with self.assertRaises(ValueError):
            self.mute("session-b", bus=self.bus.with_name("other.jsonl"))
        self.assertFalse(DEMUX.agent_playback_muted(
            self.home, "claude-code", "session-b", self.bus))

    def test_large_valid_mailbox_can_be_muted_without_losing_history(self):
        self.attach_lease("session-a", "filip")
        lease = self.home / "leases" / f"{DEMUX.lease_identifier('claude-code', 'session-a')}.json"
        row = json.loads(lease.read_text())
        row["pending"] = [{"text": "x" * (5 << 20)}]
        lease.write_text(json.dumps(row))
        before = lease.read_bytes()
        self.mute("session-a")
        self.assertEqual(lease.read_bytes(), before)
        self.assertTrue(DEMUX.agent_playback_muted(
            self.home, "claude-code", "session-a", self.bus))

    def test_manual_replay_works_while_automatic_speech_is_muted(self):
        self.attach_lease("session-a", "filip")
        self.mute("session-a")
        _, reply, _ = self.say("--session", "session-a")
        args = DEMUX.argparse.Namespace(bridge_home=self.home)
        with (
            patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as spoken,
            patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(DEMUX.speak_published_reply(
                args, self.bus, reply, "a" * 24), 0)
        spoken.assert_called_once()

    def test_replay_of_a_cached_reply_needs_no_network_or_credential(self):
        self.attach_lease("session-a", "filip")
        _, reply, _ = self.say("--session", "session-a")
        args = DEMUX.argparse.Namespace(bridge_home=self.home)
        played = (True, None, None)
        with (
            patch.object(DEMUX, "_xai_speech_key", return_value="test-only-key"),
            patch.object(DEMUX, "_tts_exchange", return_value=(b"\x01\x00", None, None)) as paid,
            patch.object(DEMUX, "_play_pcm_24k", return_value=played),
            patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(DEMUX.speak_published_reply(args, self.bus, reply, "a" * 24), 0)
        paid.assert_called_once()
        with (
            patch.object(DEMUX, "_xai_speech_key", return_value=None),
            patch.object(DEMUX, "_tts_exchange", side_effect=AssertionError("network on a hit")),
            patch.object(DEMUX, "_play_pcm_24k", return_value=played) as player,
            patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(DEMUX.speak_published_reply(args, self.bus, reply, "b" * 24), 0)
        self.assertEqual(player.call_args.args[0], b"\x01\x00")
        sidecars = list((self.home / "speech").glob("*.json"))
        self.assertEqual(len(sidecars), 1)
        sidecar = json.loads(sidecars[0].read_text())
        self.assertEqual((sidecar["reply_id"], sidecar["text"]), (reply["reply_id"], reply["text"]))

    def test_reply_audio_reopens_in_fresh_process_without_synthesis_or_credentials(self):
        self.attach_lease("session-a", "filip")
        _, reply, _ = self.say("--session", "session-a")
        args = DEMUX.argparse.Namespace(bridge_home=self.home)
        with (
            patch.object(DEMUX, "_xai_speech_key", return_value="fixture-only"),
            patch.object(DEMUX, "_tts_exchange", return_value=(b"\x01\x00", None, None)),
            patch.object(DEMUX, "_play_pcm_24k", return_value=(True, None, None)),
            patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(DEMUX.speak_published_reply(args, self.bus, reply, "a" * 24), 0)
        program = r'''
import contextlib, importlib.util, io, json, sys
from pathlib import Path
from unittest.mock import patch
spec = importlib.util.spec_from_file_location("restart_demux", sys.argv[1])
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)
request = json.loads(sys.stdin.read())
args = module.argparse.Namespace(bridge_home=Path(request["home"]))
with (
    patch.object(module, "_xai_speech_key", side_effect=AssertionError("credential lookup on cache hit")),
    patch.object(module, "_tts_exchange", side_effect=AssertionError("paid synthesis after restart")),
    patch.object(module, "_play_pcm_24k", return_value=(True, None, None)) as player,
    patch.object(module, "publish_reply_event", return_value={}),
    contextlib.redirect_stdout(io.StringIO()),
):
    code = module.speak_published_reply(args, Path(request["bus"]), request["reply"], "b" * 24)
assert code == 0, code
assert player.call_count == 1
assert player.call_args.args[0] == b"\x01\x00"
print("reopened_cached_reply")
'''
        result = subprocess.run(
            [sys.executable, "-c", program, str(Path(DEMUX.__file__).resolve())],
            input=json.dumps({"home": str(self.home), "bus": str(self.bus), "reply": reply}),
            text=True, capture_output=True, timeout=15, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "reopened_cached_reply")

    def test_malformed_mute_receipt_never_authorizes_automatic_speech(self):
        self.attach_lease("session-a", "filip")
        self.mute("session-a")
        path = DEMUX.playback_mute_path(self.home, "claude-code", "session-a", self.bus)
        row = json.loads(path.read_text())
        for field, invalid in (("muted", "false"), ("provider_session_id", "other")):
            with self.subTest(field=field):
                path.write_text(json.dumps({**row, field: invalid}))
                code, _, spoken = self.say("--session", "session-a")
                self.assertEqual(code, 0)
                spoken.assert_not_called()

    def global_mute(self, muted=True):
        with contextlib.redirect_stdout(io.StringIO()), patch.object(DEMUX.sys, "argv", [
                "cs-bus", "--mute-all" if muted else "--unmute-all", "--bridge-home", str(self.home)]):
            self.assertEqual(DEMUX.main(), 0)

    def test_attach_and_playback_report_all_mute_combinations(self):
        self.attach_lease("session-a", "filip")
        for global_muted, agent_muted in ((False, False), (False, True), (True, False), (True, True)):
            with self.subTest(global_muted=global_muted, agent_muted=agent_muted):
                self.global_mute(global_muted)
                self.mute("session-a", agent_muted)
                expected = {"global_muted": global_muted, "agent_muted": agent_muted,
                            "effective_muted": global_muted or agent_muted,
                            "mute_scope": "global" if global_muted else "agent" if agent_muted else None}
                receipt = self.attach_lease("session-a", "filip")
                self.assertEqual(receipt["playback"], expected)
                code, reply, spoken = self.say("--session", "session-a")
                self.assertEqual(code, 0)
                self.assertEqual(self.outcome["reply_id"], reply["reply_id"])
                if expected["effective_muted"]:
                    spoken.assert_not_called()
                    self.assertEqual(self.outcome["reason"], "muted")
                    self.assertEqual(self.outcome["mute_scope"], expected["mute_scope"])
                    self.assertEqual(self.outcome["playback"], expected)
                    persisted = list((self.home / "runtime/reply-playback").glob("*.json"))
                    terminal = next(json.loads(path.read_text()) for path in persisted
                                    if json.loads(path.read_text()).get("reply_id") == reply["reply_id"])
                    self.assertEqual(terminal["playback"], expected)
                else:
                    spoken.assert_called_once()

    def test_unmute_agent_receipt_reports_global_block_without_changing_global_mute(self):
        self.attach_lease("session-a", "filip")
        self.mute("session-a")
        self.global_mute()
        global_path = self.home / "runtime/playback-mutes/global.json"
        before = global_path.read_bytes()
        receipt = self.mute("session-a", False)
        self.assertFalse(receipt["muted"])
        self.assertEqual(receipt["playback"], {
            "global_muted": True, "agent_muted": False,
            "effective_muted": True, "mute_scope": "global",
        })
        self.assertEqual(global_path.read_bytes(), before)

    def test_playback_snapshot_preserves_missing_and_malformed_receipt_policy(self):
        snapshot = lambda: DEMUX.playback_mute_snapshot(self.home, "claude-code", "session-a", self.bus)
        self.assertEqual(snapshot(), {"global_muted": False, "agent_muted": False,
                                      "effective_muted": False, "mute_scope": None})
        global_path = self.home / "runtime/playback-mutes/global.json"
        agent_path = DEMUX.playback_mute_path(self.home, "claude-code", "session-a", self.bus)
        for path, field, scope in ((global_path, "global_muted", "global"),
                                   (agent_path, "agent_muted", "agent")):
            path.parent.mkdir(parents=True, exist_ok=True)
            for raw in (b"not-json", b"[]", b"\xff", json.dumps({
                    "schema": DEMUX.PLAYBACK_MUTE_SCHEMA, "scope": "all", "muted": "false",
            }).encode()):
                with self.subTest(scope=scope, raw=raw):
                    path.write_bytes(raw)
                    before = path.read_bytes()
                    result = snapshot()
                    self.assertTrue(result[field])
                    self.assertTrue(result["effective_muted"])
                    self.assertEqual(result["mute_scope"], scope)
                    self.assertEqual(path.read_bytes(), before)
            path.unlink()

    def test_manual_replay_lifts_global_and_agent_mute_for_that_one_reply(self):
        # Maciej, 2026-10-23: an explicit Play is the opt-in for that reply;
        # both mutes govern automatic speech only, and the next automatic
        # reply stays muted.
        self.attach_lease("session-a", "filip")
        self.mute("session-a")
        self.global_mute()
        _, reply, _ = self.say("--session", "session-a")
        out = io.StringIO()
        with (patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as spoken,
              patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
              contextlib.redirect_stdout(out)):
            self.assertEqual(DEMUX.speak_published_reply(
                DEMUX.argparse.Namespace(bridge_home=self.home), self.bus, reply, "c" * 24), 0)
        spoken.assert_called_once()
        result = json.loads(out.getvalue().splitlines()[-1])
        self.assertEqual(result["state"], "spoken")
        self.assertNotIn("reason", result)
        self.assertNotIn("mute_scope", result)
        code, _, automatic = self.say("--session", "session-a")
        self.assertEqual(code, 0)
        automatic.assert_not_called()
        self.assertEqual(self.outcome["reason"], "muted")
        self.assertEqual(self.outcome["mute_scope"], "global")
        self.assertEqual(self.outcome["playback"], {"global_muted": True, "agent_muted": True,
                                                   "effective_muted": True, "mute_scope": "global"})

    def test_global_mute_covers_existing_future_and_other_bus_without_changing_individuals(self):
        self.attach_lease("session-a", "filip")
        self.attach_lease("session-b", "filip")
        self.mute("session-a")
        individual = DEMUX.playback_mute_path(self.home, "claude-code", "session-a", self.bus)
        original = individual.read_bytes()
        self.global_mute()
        self.attach_lease("future-session", "filip")
        for session in ("session-a", "session-b", "future-session"):
            _, _, spoken = self.say("--session", session)
            spoken.assert_not_called()
        _, reply, _ = self.say("--session", "session-b")
        args = DEMUX.argparse.Namespace(bridge_home=self.home)
        # Automatic speech on any bus stays muted; an explicit Play admitted
        # under the master mute is the opt-in for that reply and runs.
        for automatic in (True, False):
            control = DEMUX.ReplyPlaybackControl(
                args, self.bus.with_name("other-channel.jsonl"), reply,
                "a" * 24 if automatic else "b" * 24, automatic=automatic)
            try:
                self.assertEqual(control.muted(), automatic)
                self.assertEqual(control.stopped(), automatic)
            finally:
                control.close()
        self.global_mute(False)
        self.assertEqual(individual.read_bytes(), original)
        _, _, spoken = self.say("--session", "session-b")
        spoken.assert_called_once()
        _, _, spoken = self.say("--session", "session-a")
        spoken.assert_not_called()

    def test_global_switch_is_observed_by_an_existing_player_and_invalid_receipt_refuses(self):
        self.attach_lease("session-a", "filip")
        _, reply, _ = self.say("--session", "session-a")
        control = DEMUX.ReplyPlaybackControl(
            DEMUX.argparse.Namespace(bridge_home=self.home), self.bus, reply, "f" * 24)
        try:
            self.assertFalse(control.stopped())
            self.global_mute()
            self.assertTrue(control.stopped(), "master mute must stop a running player too")
            path = self.home / "runtime/playback-mutes/global.json"
            for row in ({"schema": DEMUX.PLAYBACK_MUTE_SCHEMA, "scope": "all", "muted": "false"},
                        {"schema": DEMUX.PLAYBACK_MUTE_SCHEMA, "scope": "single", "muted": False}):
                path.write_text(json.dumps(row))
                self.assertTrue(control.muted())
            path.write_text("not-json")
            self.assertTrue(control.muted())
        finally:
            control.close()

    def test_global_mute_rejects_other_commands_before_any_write(self):
        for flags in (["--version"], ["--provider", "codex", "--session", "s"],
                      ["--send", "text", "--to", "0"], ["--read-pending"], ["--channel", "0"]):
            with (self.subTest(flags=flags), self.assertRaises(SystemExit),
                  contextlib.redirect_stderr(io.StringIO()),
                  patch.object(DEMUX.sys, "argv", ["cs-bus", "--mute-all", "--bridge-home", str(self.home), *flags])):
                DEMUX.main()
        self.assertFalse((self.home / "runtime/playback-mutes/global.json").exists())

    def test_explicit_name_still_wins(self):
        self.attach_lease("session-a", "filip")
        code, reply, spoken = self.say("--session", "session-a", "--name", "roman")
        self.assertEqual(code, 0)
        self.assertEqual(reply["name"], "roman")
        spoken.assert_called_once_with(
            "Zrobione.",
            "leo",
            DEMUX.DEFAULT_SPEECH_SPEED,
            playback_root=self.home,
            bus=self.bus, control=ANY,
        )

    def test_without_lease_or_name_it_refuses(self):
        with (
            self.assertRaises(SystemExit) as refused,
            contextlib.redirect_stderr(io.StringIO()),
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
        self.assertEqual(self.outcome["reason"], "quota_exhausted")
        rows = [json.loads(line) for line in self.bus.read_text().splitlines()]
        self.assertEqual(rows[0]["text"], "Zrobione.")
        row = rows[-1]
        self.assertEqual(row["reason"], "quota_exhausted")
        self.assertEqual(row["tts_error"], "tts request failed (403)")


class SystemSayTests(unittest.TestCase):
    """System replies use the durable owner and the same live-take guard."""

    setUp = SayReplyTests.setUp
    global_mute = SayReplyTests.global_mute

    def say_system(self, *, mock_speaker=True, flags=(), bus_override=True):
        out = io.StringIO()
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {
                "CS_SAY_VOICE": "", "CS_SAY_SPEED": "", "CS_SAY_TTS_VENDOR": "",
            }))
            stack.enter_context(patch.object(sys, "argv", [
                "cs-bus", "--say", "Zrobione", "--bridge-home", str(self.home),
                *(["--bus", str(self.bus)] if bus_override else []), *flags,
            ]))
            stack.enter_context(patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish))
            stack.enter_context(contextlib.redirect_stdout(out))
            spoken = (stack.enter_context(patch.object(DEMUX, "_speak_xai",
                      return_value=(True, None, None))) if mock_speaker else None)
            code = DEMUX.main()
        rows = [json.loads(line) for line in out.getvalue().splitlines()]
        self.outcome = rows[-1]
        return code, next(row for row in rows if row["kind"] == "agent_reply"), spoken

    def test_system_reply_is_durable_and_plays_under_mute_all_without_unmuting_agents(self):
        self.global_mute()
        mute = self.home / "runtime/playback-mutes/global.json"
        before = mute.read_bytes()
        code, reply, spoken = self.say_system()
        self.assertEqual(code, 0)
        self.assertEqual((reply["provider"], reply["provider_session_id"], reply["name"]),
                         ("codescribe", "system", "Codescribe"))
        self.assertEqual((reply["system"], reply["association"], reply["voice"],
                          reply["speed"], reply["tts_vendor"]),
                         (True, "unsolicited", "carina", 1.0, "xai"))
        spoken.assert_called_once_with("Zrobione", "carina", 1.0,
                                       playback_root=self.home, bus=self.bus, control=ANY)
        self.assertEqual(self.outcome["state"], "spoken")
        self.assertEqual(mute.read_bytes(), before)
        source = json.loads((self.home / "runtime/reply-sources" / f"{reply['reply_id']}.json").read_text())
        raw = self.bus.read_bytes()
        offset, length = source["source"]["offset"], source["source"]["length"]
        self.assertEqual(json.loads(raw[offset:offset + length]), reply)
        self.assertEqual(source["bus"], str(self.bus.resolve()))

    def test_default_system_bus_cannot_be_redirected_by_an_agent_lease(self):
        other = self.root / "other-channel.jsonl"
        lease_id = DEMUX.lease_identifier("codescribe", "system")
        DEMUX.atomic_json(self.home / "leases" / f"{lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "provider": "codescribe",
            "provider_session_id": "system", "lease_id": lease_id,
            "bus": str(other.resolve()), "pending": [],
        })
        code, _, _ = self.say_system(bus_override=False)
        self.assertEqual(code, 0)
        self.assertFalse(other.exists())
        self.assertEqual(json.loads(self.bus.read_text().splitlines()[0])["kind"], "agent_reply")

    def test_addressed_reply_still_requires_an_explicit_agent_identity(self):
        with self.assertRaises(SystemExit) as refused, contextlib.redirect_stderr(io.StringIO()):
            self.say_system(flags=("--reply-to", "a" * 24))
        self.assertEqual(refused.exception.code, 2)
        self.assertEqual(self.bus.read_text(), "")

    def test_muted_system_reply_reaches_the_serialized_player_and_keeps_cached_audio(self):
        self.global_mute()
        player = MagicMock()
        player.poll.return_value = 0
        with (
            patch.object(DEMUX, "_xai_speech_key", return_value="fixture-only"),
            patch.object(DEMUX, "_tts_exchange", return_value=(b"\x01\x00", None, None)),
            patch("subprocess.Popen", return_value=player) as play,
        ):
            code, reply, _ = self.say_system(mock_speaker=False)
        self.assertEqual((code, self.outcome["state"]), (0, "spoken"))
        self.assertEqual(play.call_args.args[0][0], "afplay")
        sidecar, = (self.home / "speech").glob("*.json")
        self.assertEqual(json.loads(sidecar.read_text())["reply_id"], reply["reply_id"])

    def test_system_marker_bypasses_mute_flips_but_keeps_its_stop_ticket(self):
        _, reply, _ = self.say_system()
        control = DEMUX.ReplyPlaybackControl(
            DEMUX.argparse.Namespace(bridge_home=self.home), self.bus, reply,
            "e" * 24, automatic=True)
        try:
            self.global_mute()
            self.assertFalse(control.stopped())
            self.assertFalse(control.mute_snapshot()["effective_muted"])
            DEMUX.atomic_json(control.stop_path, {"schema": DEMUX.REPLY_CONTROL_SCHEMA,
                              **control.owner(), "playback_ticket": control.ticket})
            self.assertTrue(control.stopped())
        finally:
            control.close()

    def test_system_reply_refuses_a_live_take_even_when_agents_are_muted(self):
        self.global_mute()
        self.bus.write_text(json.dumps({"status": "session_started", "session_id": "live-take"}) + "\n")
        with (
            patch.object(DEMUX, "_xai_speech_key", return_value="fixture-only"),
            patch.object(DEMUX, "_tts_exchange", return_value=(b"\x01\x00", None, None)),
            patch.object(DEMUX, "TAKE_WAIT_SECONDS", 0.05),
            patch("subprocess.Popen") as player,
        ):
            code, reply, _ = self.say_system(mock_speaker=False)
        self.assertEqual(code, 5)
        self.assertTrue(reply["system"])
        self.assertEqual((self.outcome["state"], self.outcome["reason"], self.outcome["spoken"]),
                         ("refused", "take_live", False))
        player.assert_not_called()
        self.assertTrue((self.home / "runtime/reply-sources" / f"{reply['reply_id']}.json").exists())

    def test_system_reply_with_private_bus_still_observes_the_app_capture_bus(self):
        canonical = self.root / "app-bus.jsonl"
        canonical.write_text(json.dumps({"status": "session_started", "session_id": "app-take"}) + "\n")
        with (
            patch.object(DEMUX, "bus_path", return_value=canonical),
            patch.object(DEMUX, "_xai_speech_key", return_value="fixture-only"),
            patch.object(DEMUX, "_tts_exchange", return_value=(b"\x01\x00", None, None)),
            patch.object(DEMUX, "TAKE_WAIT_SECONDS", 0.05),
            patch("subprocess.Popen") as player,
        ):
            code, _, _ = self.say_system(mock_speaker=False)
        self.assertEqual((code, self.outcome["reason"]), (5, "take_live"))
        player.assert_not_called()


def queued_say(
    home,
    bus,
    name,
    synthesized,
    waiting,
    started,
    release,
    wait_seconds=2.0,
    vendor="xai",
    speech_bus=None,
    take_wait_seconds=0.15,
    scan_delay=0.0,
    slow_scans=None,
    finish_in_scan=None,
    take_in_scan=False,
):
    """Real --say and flock in a process; only synthesis and afplay are replaced."""
    DEMUX.PLAYBACK_WAIT_SECONDS = wait_seconds
    DEMUX.TAKE_WAIT_SECONDS = take_wait_seconds
    DEMUX.PLAYBACK_POLL_SECONDS = 0.01
    acquire = DEMUX._acquire_playback_lock
    playback_log = Path(home) / "playback.jsonl"
    sentinel = Path(home) / "playing"
    probe = DEMUX.installation_idle

    delayed = [0]
    scans = [0]
    ended = [False]

    def delayed_probe(*args, **kwargs):
        if not sentinel.exists():
            waiting.set()
        if sentinel.exists():
            scans[0] += 1
            if scans[0] == finish_in_scan:
                # The player exits while this scan runs, after the loop's last
                # poll; with take_in_scan a take starts right after the last sample.
                ended[0] = True
                if take_in_scan:
                    with Path(speech_bus or bus).open("a") as stream:
                        stream.write(
                            json.dumps(
                                {"status": "session_started", "session_id": "channel-3-take"}
                            )
                            + "\n"
                        )
        # Delay the real safety probe only after playback has started; with
        # slow_scans set, only that many scans are slow.
        if scan_delay and sentinel.exists() and (slow_scans is None or delayed[0] < slow_scans):
            delayed[0] += 1
            time.sleep(scan_delay)
        return probe(*args, **kwargs)

    def synthesize(_request):
        synthesized.set()
        return b"\x00\x00", None, None

    def lock(descriptor, control=None):
        waiting.set()
        return acquire(descriptor, control)

    class Player:
        def __init__(self, _command, **_kwargs):
            # O_EXCL catches overlapping playback even across processes.
            descriptor = os.open(sentinel, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            os.close(descriptor)
            self.result = None
            self.log("start")
            started.set()

        def log(self, action):
            with playback_log.open("a") as stream:
                stream.write(json.dumps([name, action]) + "\n")

        def finish(self, result):
            if self.result is None:
                self.log("end")
                sentinel.unlink()
                self.result = result
            return self.result

        def poll(self):
            if self.result is not None:
                return self.result
            if ended[0] or release is None or release.is_set():
                return self.finish(0)
            return None

        def terminate(self):
            self.log("terminate")
            self.finish(-15)

        def kill(self):
            self.finish(-9)

        def wait(self, timeout=None):
            return self.result

    argv = [
        "bus-demux.py",
        "--say",
        "Test reply",
        "--name",
        name,
        "--provider",
        "codex",
        "--session",
        name,
        "--bridge-home",
        str(home),
        "--bus",
        str(bus),
        "--tts-vendor",
        vendor,
    ]
    with (
        patch.object(sys, "argv", argv),
        patch.object(DEMUX, "_tts_exchange", side_effect=synthesize),
        patch.object(DEMUX, "_xai_speech_key", return_value="test-only-key"),
        patch.object(DEMUX, "_openai_speech_key", return_value="test-only-key"),
        patch.object(DEMUX, "_acquire_playback_lock", side_effect=lock),
        patch.object(DEMUX, "installation_idle", side_effect=delayed_probe),
        patch("subprocess.Popen", side_effect=Player),
        patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish),
        patch.object(DEMUX, "bus_path", return_value=speech_bus or Path(bus)),
        contextlib.redirect_stdout(io.StringIO()),
    ):
        result = DEMUX.main()
    if result not in (0, 5):
        raise AssertionError(f"unexpected --say exit {result}")


class PlaybackQueueTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name) / "agent-bridge"
        self.home.mkdir()
        self.bus = Path(self.tmp.name) / "bus.jsonl"
        self.bus.touch()
        self.context = multiprocessing.get_context("spawn")
        self.children = []
        self.addCleanup(self.stop_children)

    def stop_children(self):
        for child in self.children:
            if child.is_alive():
                child.terminate()
            child.join(5)

    def start_say(
        self,
        name,
        release=None,
        wait_seconds=2.0,
        vendor="xai",
        speech_bus=None,
        take_wait_seconds=0.15,
        scan_delay=0.0,
        slow_scans=None,
        finish_in_scan=None,
        take_in_scan=False,
    ):
        events = [self.context.Event() for _ in range(3)]
        child = self.context.Process(
            target=queued_say,
            args=(
                self.home,
                self.bus,
                name,
                *events,
                release,
                wait_seconds,
                vendor,
                speech_bus,
                take_wait_seconds,
                scan_delay,
                slow_scans,
                finish_in_scan,
                take_in_scan,
            ),
        )
        child.start()
        child.test_events = events
        self.children.append(child)
        return child, events

    def join(self, child):
        child.join(5)
        self.assertFalse(child.is_alive(), "--say must finish within its bounded wait")
        self.assertEqual(child.exitcode, 0)

    def hold_lock(self):
        path = self.home / "runtime" / "playback.lock"
        path.parent.mkdir(parents=True, exist_ok=True)
        lock = path.open("a")
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.addCleanup(lock.close)
        return lock

    def outcomes(self):
        return [
            row
            for line in self.bus.read_text().splitlines()
            if (row := json.loads(line)).get("kind") == "agent_reply_playback"
            and row.get("state") not in ("waiting", "playing")
        ]

    def test_parallel_say_synthesizes_in_parallel_and_plays_in_lock_order(self):
        release = self.context.Event()
        first, (_, _, playing) = self.start_say("filip", release)
        self.assertTrue(playing.wait(5))
        second, (synthesized, waiting, second_playing) = self.start_say(
            "astra", vendor="openai"
        )
        self.assertTrue(
            synthesized.wait(5), "TTS must proceed while another agent plays"
        )
        self.assertTrue(waiting.wait(5))
        self.assertFalse(
            second_playing.wait(0.15), "no overlap while the first holds flock"
        )
        release.set()
        self.join(first)
        self.join(second)
        log = [
            json.loads(line)
            for line in (self.home / "playback.jsonl").read_text().splitlines()
        ]
        self.assertEqual(
            log,
            [
                ["filip", "start"],
                ["filip", "end"],
                ["astra", "start"],
                ["astra", "end"],
            ],
        )
        self.assertEqual(len(self.outcomes()), 2)
        self.assertTrue(all(row["spoken"] for row in self.outcomes()))

    def test_timeout_writes_playback_busy_and_never_plays(self):
        lock = self.hold_lock()
        child, (_, waiting, playing) = self.start_say("astra", wait_seconds=0.1)
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set())
        (reply,) = self.outcomes()
        self.assertFalse(reply["spoken"])
        self.assertEqual(reply["reason"], "playback_busy")
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
        # The timed-out waiter must not remove the shared lock inode.
        self.assertTrue((self.home / "runtime" / "playback.lock").exists())

    def test_slow_scan_keeps_playback_and_lock_until_release(self):
        release = self.context.Event()
        first, (_, _, playing) = self.start_say(
            "filip", release, scan_delay=0.03
        )
        self.assertTrue(playing.wait(5))
        second, (_, waiting, second_playing) = self.start_say("astra")
        self.assertTrue(waiting.wait(5))
        early = second_playing.wait(0.15)
        release.set()
        self.join(first)
        self.join(second)
        log = (self.home / "playback.jsonl").read_text()
        self.assertFalse(early, f"premature lock release: {log}; {self.outcomes()}")
        self.assertTrue(all(row["spoken"] for row in self.outcomes()))

    def test_one_scan_beyond_budget_does_not_cut_the_voice(self):
        # CI, release/0.17.0 @ 100668a1f: a single scan that ran out of budget on
        # a loaded runner stopped the first voice as "take_started" and let the
        # second play. An unfinished scan is not a take; the cursor resumes.
        release = self.context.Event()
        child, (_, _, playing) = self.start_say(
            "filip", release, scan_delay=0.6, slow_scans=1
        )
        self.assertTrue(playing.wait(5))
        time.sleep(0.9)
        release.set()
        self.join(child)
        log = (self.home / "playback.jsonl").read_text()
        self.assertNotIn('["filip", "terminate"]', log)
        (reply,) = self.outcomes()
        self.assertTrue(reply["spoken"], f"{reply}; {log}")

    def test_scan_beyond_budget_still_terminates_playback(self):
        release = self.context.Event()
        child, (_, _, playing) = self.start_say(
            "filip", release, scan_delay=0.6
        )
        self.assertTrue(playing.wait(5))
        self.join(child)
        (reply,) = self.outcomes()
        self.assertEqual(reply["reason"], "take_started")
        self.assertFalse(reply["spoken"])
        self.assertIn('["filip", "terminate"]', (self.home / "playback.jsonl").read_text())

    def test_take_after_the_last_sample_keeps_a_spoken_receipt(self):
        # Founder, 2026-10-23 (Miksa ch6 20:12Z, Hania ch4 21:08Z): she heard the
        # whole reply, pressed Fn+4 right after it, and the receipt said
        # refused/take_started. The player had already exited inside the scan
        # that saw the take, so nothing was cut and the reply was spoken.
        canonical = self.home / "canonical-bus.jsonl"
        canonical.touch()
        release = self.context.Event()
        child, (_, _, playing) = self.start_say(
            "filip", release, speech_bus=canonical, finish_in_scan=1, take_in_scan=True
        )
        self.assertTrue(playing.wait(5))
        self.join(child)
        (reply,) = self.outcomes()
        self.assertTrue(reply["spoken"], reply)
        self.assertEqual(reply["state"], "spoken")
        log = [
            json.loads(line)
            for line in (self.home / "playback.jsonl").read_text().splitlines()
        ]
        self.assertEqual(log, [["filip", "start"], ["filip", "end"]])

    def test_unfinished_scans_after_the_player_ended_keep_a_spoken_receipt(self):
        # Three over-budget scans stop a voice that is still playing (see
        # test_scan_beyond_budget_still_terminates_playback), not one that ended.
        release = self.context.Event()
        child, (_, _, playing) = self.start_say(
            "filip", release, scan_delay=0.6, finish_in_scan=3
        )
        self.assertTrue(playing.wait(5))
        self.join(child)
        (reply,) = self.outcomes()
        self.assertTrue(reply["spoken"], reply)
        self.assertNotIn(
            '["filip", "terminate"]', (self.home / "playback.jsonl").read_text()
        )

    def test_take_started_while_waiting_is_checked_after_acquisition(self):
        lock = self.hold_lock()
        child, (_, waiting, playing) = self.start_say("astra")
        self.assertTrue(waiting.wait(5))
        with self.bus.open("a") as stream:
            for status in ["session_started", "transcript_sealed"]:
                stream.write(
                    json.dumps({"status": status, "session_id": "take"}) + "\n"
                )
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
        self.join(child)
        self.assertFalse(
            playing.is_set(), "utterance seal must not end a live microphone take"
        )
        (reply,) = self.outcomes()
        self.assertFalse(reply["spoken"])
        self.assertEqual(reply["reason"], "take_live")
        with self.bus.open("a") as stream:
            stream.write(
                json.dumps({"status": "session_ended", "session_id": "take"}) + "\n"
            )
        next_child, (_, _, next_playing) = self.start_say("filip")
        self.join(next_child)
        self.assertTrue(
            next_playing.is_set(), "the refusal must release the playback lock"
        )

    def test_canonical_take_on_another_channel_times_out_without_playback(self):
        canonical = self.home / "canonical-bus.jsonl"
        canonical.write_text(
            json.dumps({"status": "session_started", "session_id": "channel-3-take"})
            + "\n"
        )
        child, (_, waiting, playing) = self.start_say("filip", speech_bus=canonical)
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set())
        (reply,) = self.outcomes()
        self.assertEqual(reply["reason"], "take_live")
        self.assertFalse(reply["spoken"])

    def test_take_ends_while_waiting_then_playback_starts(self):
        canonical = self.home / "canonical-bus.jsonl"
        canonical.write_text(
            json.dumps({"status": "session_started", "session_id": "take"}) + "\n"
        )
        child, (_, waiting, playing) = self.start_say(
            "filip", speech_bus=canonical, take_wait_seconds=2.0
        )
        self.assertTrue(waiting.wait(5))
        self.assertFalse(playing.wait(0.15))
        with canonical.open("a") as stream:
            stream.write(
                json.dumps({"status": "session_ended", "session_id": "take"}) + "\n"
            )
        self.join(child)
        self.assertTrue(playing.is_set())
        (reply,) = self.outcomes()
        self.assertTrue(reply["spoken"])

    def test_take_start_during_playback_terminates_player_and_reports_it(self):
        canonical = self.home / "canonical-bus.jsonl"
        canonical.touch()
        release = self.context.Event()
        child, (_, _, playing) = self.start_say("filip", release, speech_bus=canonical)
        self.assertTrue(playing.wait(5))
        with canonical.open("a") as stream:
            stream.write(
                json.dumps(
                    {"status": "session_started", "session_id": "channel-3-take"}
                )
                + "\n"
            )
        self.join(child)
        (reply,) = self.outcomes()
        self.assertEqual(reply["reason"], "take_started")
        self.assertFalse(reply["spoken"])
        log = [
            json.loads(line)
            for line in (self.home / "playback.jsonl").read_text().splitlines()
        ]
        self.assertEqual(
            log, [["filip", "start"], ["filip", "terminate"], ["filip", "end"]]
        )
        lock = self.hold_lock()
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)

    def test_idle_cursor_consumes_tail_and_resets_after_rotation_or_truncation(self):
        cursor = {}
        self.bus.write_text(
            json.dumps({"status": "session_started", "session_id": "first"}) + "\n"
        )
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        initial_offset = cursor["offset"]
        with self.bus.open("a") as stream:
            stream.write(
                json.dumps({"status": "session_ended", "session_id": "first"}) + "\n"
            )
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        self.assertGreater(cursor["offset"], initial_offset)
        self.bus.rename(self.bus.with_suffix(".old"))
        self.bus.write_text(
            json.dumps({"status": "session_started", "session_id": "second"}) + "\n"
        )
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        self.bus.write_text("")
        # Erasing history is not a terminal receipt for the observed live take.
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        self.bus.write_text(
            json.dumps({"status": "session_ended", "session_id": "second"}) + "\n"
        )
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )

    def test_idle_cursor_refuses_partial_tail_until_the_row_completes(self):
        cursor = {}
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        self.bus.write_text('{"status": "session_started", "session_id":')
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        with self.bus.open("a") as stream:
            stream.write('"take"}\n')
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )
        with self.bus.open("a") as stream:
            stream.write(
                json.dumps({"status": "session_ended", "session_id": "take"}) + "\n"
            )
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.home)
        )

    # Integrator (2026-10-01): two replies played into the Founder's open
    # channel 1 because the guard read only the shared bus. Since W5 a channel
    # take lives on its dedicated bus as channel-session rows.
    def channel_row(self, bus, session, state):
        bus.parent.mkdir(parents=True, exist_ok=True)
        row = {
            "schema": "codescribe.channel-session.v1",
            "channel": session.split("-")[2],
            "session_id": session,
            "state": state,
            "emitted_at": datetime.datetime.now(datetime.timezone.utc)
            .isoformat()
            .replace("+00:00", "Z"),
        }
        if state != "open":
            row["reason"] = "silence"
        with bus.open("a") as stream:
            stream.write(json.dumps(row) + "\n")
        return bus

    def test_open_channel_on_its_dedicated_bus_blocks_speech(self):
        channel = self.home / "buses" / "channel-1.jsonl"
        self.channel_row(channel, "agent-channel-1-live", "open")
        child, (_, waiting, playing) = self.start_say("filip")
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set(), "an open channel microphone is a live take")
        (reply,) = self.outcomes()
        self.assertFalse(reply["spoken"])
        self.assertEqual(reply["reason"], "take_live")

    def test_open_broadcast_channel_on_the_shared_bus_blocks_speech(self):
        # 19:31:44Z: Fn+0 wrote its channel-session rows to the shared bus.
        self.channel_row(self.bus, "agent-channel-0-live", "open")
        child, (_, waiting, playing) = self.start_say("filip")
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set())
        (reply,) = self.outcomes()
        self.assertEqual(reply["reason"], "take_live")

    def test_open_channel_on_a_bus_the_binding_names_blocks_speech(self):
        elsewhere = Path(self.tmp.name) / "elsewhere" / "channel-4.jsonl"
        self.channel_row(elsewhere, "agent-channel-4-live", "open")
        binding = {
            "schema": "vc.agent-audience-binding.v1",
            "bindings": {
                "4": {
                    "audience": "leon",
                    "provider": "codex",
                    "provider_session_id": "leon",
                    "bus": str(elsewhere),
                }
            },
        }
        (self.home / "vc.agent-audience-binding.v1.json").write_text(
            json.dumps(binding)
        )
        child, (_, waiting, playing) = self.start_say("filip")
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set())
        (reply,) = self.outcomes()
        self.assertEqual(reply["reason"], "take_live")

    def test_channel_opening_during_playback_stops_the_player(self):
        channel = self.home / "buses" / "channel-1.jsonl"
        self.channel_row(channel, "agent-channel-1-old", "open")
        self.channel_row(channel, "agent-channel-1-old", "sealed")
        release = self.context.Event()
        child, (_, _, playing) = self.start_say("filip", release)
        self.assertTrue(playing.wait(5))
        self.channel_row(channel, "agent-channel-1-new", "open")
        self.join(child)
        (reply,) = self.outcomes()
        self.assertFalse(reply["spoken"])
        self.assertEqual(reply["reason"], "take_started")

    def test_late_seal_of_the_predecessor_keeps_the_reopened_channel_live(self):
        # 2026-10-22 20:23: the app reopened channel 2 before the previous
        # session's seal landed, and a reply played into the live microphone.
        channel = self.home / "buses" / "channel-2.jsonl"
        self.channel_row(channel, "agent-channel-2-first", "open")
        self.channel_row(channel, "agent-channel-2-second", "open")
        self.channel_row(channel, "agent-channel-2-first", "sealed")
        child, (_, waiting, playing) = self.start_say("filip")
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set(), "the reopened session still owns the microphone")
        (reply,) = self.outcomes()
        self.assertEqual(reply["reason"], "take_live")
        self.channel_row(channel, "agent-channel-2-second", "sealed")
        child, (_, _, playing) = self.start_say("filip")
        self.join(child)
        self.assertTrue(playing.is_set(), "its own seal releases it")

    def test_sealed_channel_lets_speech_play(self):
        channel = self.home / "buses" / "channel-2.jsonl"
        self.channel_row(channel, "agent-channel-2-done", "open")
        self.channel_row(channel, "agent-channel-2-done", "sealed")
        child, (_, _, playing) = self.start_say("filip")
        self.join(child)
        self.assertTrue(playing.is_set())
        (reply,) = self.outcomes()
        self.assertTrue(reply["spoken"])

    # Maciej 2026-10-22 17:44: a reply waits while someone speaks, not while
    # the microphone is merely open. The app publishes the measured phase.
    def speech_now(self, session, phase, *, age=5.0, pid=None):
        channel = session.split("-")[2]
        path = self.home / "runtime" / f"channel-speech-{channel}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({
            "schema": "codescribe.channel-speech-now.v1",
            "channel": channel,
            "session_id": session,
            "phase": phase,
            "since_ms": int((time.time() - age) * 1000),
            "pid": os.getpid() if pid is None else pid,
        }))

    def last_outcome(self):
        return self.outcomes()[-1]

    def bind(self, name, channel):
        path = self.home / "vc.agent-audience-binding.v1.json"
        state = (json.loads(path.read_text()) if path.exists()
                 else {"schema": "vc.agent-audience-binding.v1", "bindings": {}})
        state["bindings"][channel] = {
            "audience": name, "provider": "codex", "provider_session_id": name,
        }
        path.write_text(json.dumps(state))

    def test_quiet_open_channel_admits_the_reply(self):
        channel = self.home / "buses" / "channel-1.jsonl"
        self.bind("filip", "1")
        self.channel_row(channel, "agent-channel-1-live", "open")
        marker = self.home / "runtime" / "agent-playback-now.json"
        for phase in ("quiet", "fresh"):
            with self.subTest(phase=phase):
                # Each reply starts clean; back-to-back is its own test.
                marker.unlink(missing_ok=True)
                self.speech_now("agent-channel-1-live", phase)
                child, (_, _, playing) = self.start_say("filip")
                self.join(child)
                self.assertTrue(playing.is_set())
                self.assertTrue(self.last_outcome()["spoken"])

    def test_speech_or_an_unproven_phase_holds_the_reply(self):
        channel = self.home / "buses" / "channel-1.jsonl"
        self.bind("filip", "1")
        self.channel_row(channel, "agent-channel-1-live", "open")
        gone = subprocess.Popen(["true"])
        gone.wait()
        cases = {
            "speaking": dict(phase="speaking"),
            "broken edge chain": dict(phase="unmeasured"),
            "quiet too short": dict(phase="quiet", age=0.2),
            "app gone": dict(phase="quiet", pid=gone.pid),
        }
        for label, case in cases.items():
            with self.subTest(label):
                self.speech_now("agent-channel-1-live", **case)
                child, (_, waiting, playing) = self.start_say("filip")
                self.assertTrue(waiting.wait(5))
                self.join(child)
                self.assertFalse(playing.is_set())
                self.assertEqual(self.last_outcome()["reason"], "take_live")
        with self.subTest("another session's file"):
            self.speech_now("agent-channel-1-older", "quiet")
            child, (_, _, playing) = self.start_say("filip")
            self.join(child)
            self.assertFalse(playing.is_set())

    def test_reply_survives_the_seal_and_reopen_of_its_quiet_channel(self):
        channel = self.home / "buses" / "channel-1.jsonl"
        self.bind("filip", "1")
        self.channel_row(channel, "agent-channel-1-first", "open")
        self.speech_now("agent-channel-1-first", "quiet")
        release = self.context.Event()
        child, (_, _, playing) = self.start_say("filip", release)
        self.assertTrue(playing.wait(5))
        marker = json.loads((self.home / "runtime" / "agent-playback-now.json").read_text())
        self.assertEqual(marker["schema"], "codescribe.agent-playback-now.v1")
        self.assertEqual(marker["pid"], child.pid)
        self.channel_row(channel, "agent-channel-1-first", "sealed")
        self.speech_now("agent-channel-1-second", "fresh", age=0.0)
        self.channel_row(channel, "agent-channel-1-second", "open")
        time.sleep(0.2)
        release.set()
        self.join(child)
        self.assertTrue(self.last_outcome()["spoken"], self.last_outcome())
        self.assert_playback_ended()

    def test_pause_in_a_take_for_another_agent_holds_the_reply(self):
        # 2026-10-23 06:57: Roman (channel 3) started speaking one second into
        # a pause of Monika's open take for hania (channel 4).
        self.bind("filip", "3")
        channel = self.home / "buses" / "channel-4.jsonl"
        self.channel_row(channel, "agent-channel-4-first", "open")
        self.speech_now("agent-channel-4-first", "quiet")
        child, (_, waiting, playing) = self.start_say("filip")
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set(), "a pause in a message to hania is not room for filip")
        self.assertEqual(self.last_outcome()["reason"], "take_live")
        # The take seals and the channel reopens with nothing said yet.
        self.channel_row(channel, "agent-channel-4-first", "sealed")
        self.channel_row(channel, "agent-channel-4-second", "open")
        self.speech_now("agent-channel-4-second", "fresh")
        child, (_, _, playing) = self.start_say("filip")
        self.join(child)
        self.assertTrue(playing.is_set())
        self.assertTrue(self.last_outcome()["spoken"])

    def test_ineligible_foreign_reply_does_not_hold_the_player_lock(self):
        self.bind("filip", "3")
        self.bind("hania", "4")
        channel = self.home / "buses" / "channel-4.jsonl"
        self.channel_row(channel, "agent-channel-4-first", "open")
        self.speech_now("agent-channel-4-first", "quiet")
        foreign, (_, waiting, foreign_playing) = self.start_say(
            "filip", take_wait_seconds=1.5)
        self.assertTrue(waiting.wait(5))
        own, (_, _, own_playing) = self.start_say("hania")
        self.assertTrue(own_playing.wait(1), "an ineligible reply must not occupy playback.lock")
        self.assertFalse(foreign_playing.is_set())
        self.join(own)
        self.join(foreign)

    def test_rebound_quiet_channel_revokes_the_waiting_agents_ownership(self):
        self.bind("filip", "3")
        channel = self.home / "buses" / "channel-3.jsonl"
        self.channel_row(channel, "agent-channel-3-first", "open")
        self.speech_now("agent-channel-3-first", "speaking")
        child, (_, waiting, playing) = self.start_say("filip", take_wait_seconds=0.8)
        self.assertTrue(waiting.wait(5))
        self.bind("hania", "3")
        self.channel_row(channel, "agent-channel-3-first", "sealed")
        self.channel_row(channel, "agent-channel-3-second", "open")
        self.speech_now("agent-channel-3-second", "quiet")
        self.join(child)
        self.assertFalse(playing.is_set(), "ownership must be read again at admission")
        self.assertEqual(self.last_outcome()["reason"], "take_live")

    def assert_playback_ended(self):
        marker = json.loads((self.home / "runtime" / "agent-playback-now.json").read_text())
        self.assertTrue(marker["ended"])
        self.assertLessEqual(marker["until_ms"], time.time() * 1000)

    def test_quiet_restarts_when_the_previous_reply_ends(self):
        # Back-to-back replies: the channel heard nothing under the first one.
        channel = self.home / "buses" / "channel-1.jsonl"
        self.bind("filip", "1")
        self.channel_row(channel, "agent-channel-1-live", "open")
        self.speech_now("agent-channel-1-live", "quiet", age=30.0)
        marker = self.home / "runtime" / "agent-playback-now.json"
        marker.write_text(json.dumps({
            "schema": "codescribe.agent-playback-now.v1", "pid": os.getpid(),
            "until_ms": int(time.time() * 1000), "ended": True,
        }))
        child, (_, waiting, playing) = self.start_say("filip")
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set(), "a reply that just ended is not quiet")
        self.assertEqual(self.last_outcome()["reason"], "take_live")

    def test_a_person_stops_a_reply_by_key_or_by_measured_speech(self):
        channel = self.home / "buses" / "channel-1.jsonl"
        self.bind("filip", "1")
        self.channel_row(channel, "agent-channel-1-live", "open")
        for how in ("key take", "speech"):
            with self.subTest(how):
                self.speech_now("agent-channel-1-live", "quiet")
                release = self.context.Event()
                child, (_, _, playing) = self.start_say("filip", release)
                self.assertTrue(playing.wait(5))
                if how == "key take":
                    with self.bus.open("a") as stream:
                        stream.write(json.dumps(
                            {"status": "session_started", "session_id": "key-take"}) + "\n")
                else:
                    self.speech_now("agent-channel-1-live", "speaking", age=0.0)
                self.join(child)
                self.assertEqual(self.last_outcome()["reason"], "take_started")
                self.assert_playback_ended()
                (self.home / "runtime" / "agent-playback-now.json").unlink()
                with self.bus.open("a") as stream:
                    stream.write(json.dumps(
                        {"status": "session_ended", "session_id": "key-take"}) + "\n")

    def test_assert_install_idle_refuses_while_a_channel_is_open(self):
        channel = self.home / "buses" / "channel-3.jsonl"
        self.channel_row(channel, "agent-channel-3-live", "open")
        argv = [
            "bus-demux.py",
            "--assert-install-idle",
            "--bridge-home",
            str(self.home),
            "--bus",
            str(self.bus),
        ]
        with (
            patch.object(sys, "argv", argv),
            patch.object(DEMUX, "bridge_home", return_value=self.home),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertNotEqual(DEMUX.main(), 0)
        self.channel_row(channel, "agent-channel-3-live", "sealed")
        with (
            patch.object(sys, "argv", argv),
            patch.object(DEMUX, "bridge_home", return_value=self.home),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            self.assertEqual(DEMUX.main(), 0)

    def test_player_failure_releases_lock_and_removes_wav(self):
        paths = []

        def fail(command, **_kwargs):
            paths.append(Path(command[1]))
            raise FileNotFoundError("test player unavailable")

        with patch("subprocess.Popen", side_effect=fail):
            played, error, reason = DEMUX._play_pcm_24k(
                b"\x00\x00", playback_root=self.home, bus=self.bus
            )
        self.assertFalse(played)
        self.assertEqual(reason, "playback_failed")
        self.assertIn("FileNotFoundError", error)
        self.assertFalse(paths[0].exists())
        lock = self.hold_lock()
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


# Root-owned independent lifecycle controls, frozen before source reception.
def forensic_cold_wait_respects_deadline():
    with tempfile.TemporaryDirectory(prefix="cs-L30-budget-",dir="/private/tmp") as d:
        root=Path(d);bus=root/"events.jsonl"
        rows=[dict(schema="forensic.synthetic.diagnostic.v1",session_id="closed-history",status="diagnostic") for _ in range(200)]
        bus.write_text("".join(json.dumps(row)+"\n" for row in rows))
        clock=[0.0];parsed=[0];loads=DEMUX.json.loads
        def slow_loads(value,*a,**kw):
            out=loads(value,*a,**kw);clock[0]+=1.0;parsed[0]+=1;return out
        with patch.object(DEMUX.time,"monotonic",side_effect=lambda:clock[0]),patch.object(DEMUX.json,"loads",side_effect=slow_loads):
            allowed=DEMUX._wait_for_take_end(bus,{},bridge_root=root)
        assert not allowed, f"expired take wait authorized playback: elapsed={clock[0]}, limit={DEMUX.TAKE_WAIT_SECONDS}, rows={parsed[0]}"
        assert clock[0] <= DEMUX.TAKE_WAIT_SECONDS+1.0, f"deadline did not interrupt reader: {clock[0]}"
        return dict(elapsed=clock[0],rows=parsed[0])
def forensic_cold_and_incremental_current_take_remains_busy():
    with tempfile.TemporaryDirectory(prefix="cs-L30-busy-",dir="/private/tmp") as d:
        root=Path(d);bus=root/"events.jsonl"
        rows=[dict(status="session_started",session_id="real-live-app")]
        rows += [dict(status="diagnostic",session_id="closed-history") for _ in range(200)]
        rows += [dict(status="session_ended",session_id="unrelated-session")]
        bus.write_text("".join(json.dumps(row)+"\n" for row in rows))
        cursor={};assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        with bus.open("a") as f:f.write(json.dumps(dict(status="session_ended",session_id="real-live-app"))+"\n")
        assert DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        return dict(early_open_retained=True,unrelated_terminal_cannot_clear=True,exact_terminal_releases=True)
def forensic_short_idle_wait_is_eligible():
    with tempfile.TemporaryDirectory(prefix="cs-L30-idle-",dir="/private/tmp") as d:
        root=Path(d);bus=root/"events.jsonl"
        bus.write_text(json.dumps(dict(status="session_started",session_id="finished"))+"\n"+json.dumps(dict(status="session_ended",session_id="finished"))+"\n")
        assert DEMUX._wait_for_take_end(bus,{},bridge_root=root)
        return dict(valid_idle=True)
def forensic_successive_fresh_playbacks_reuse_canonical_history():
    with tempfile.TemporaryDirectory(prefix="cs-L32-replies-",dir="/private/tmp") as d:
        root=Path(d);bus=root/"events.jsonl"
        bus.write_text("".join(json.dumps(dict(status="diagnostic",session_id="history"))+"\n" for _ in range(1000)))
        loads=DEMUX.json.loads;parsed=[0]
        def counted(value,*a,**kw):parsed[0]+=1;return loads(value,*a,**kw)
        with patch.object(DEMUX.json,"loads",side_effect=counted):
            assert DEMUX._wait_for_take_end(bus,{},bridge_root=root)
            first=parsed[0];parsed[0]=0
            assert DEMUX._wait_for_take_end(bus,{},bridge_root=root)
            second=parsed[0]
        assert second <= 10, f"fresh next reply reparsed complete historical bus: first={first}, second={second}"
        return dict(initial_parsed=first,next_reply_parsed=second)
def forensic_lifecycle_rotation_truncation_and_partial_tail_stay_safe():
    with tempfile.TemporaryDirectory(prefix="cs-L32-lifecycle-",dir="/private/tmp") as d:
        root=Path(d);bus=root/"events.jsonl";cursor={}
        def row(status,sid):return json.dumps(dict(status=status,session_id=sid))+"\n"
        bus.write_text(row("session_started","old")+row("session_ended","old"))
        assert DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        replacement=root/"replacement";replacement.write_text(row("session_started","new"));replacement.replace(bus)
        assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        with bus.open("a") as f:f.write(row("session_ended","new"))
        assert DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        bus.write_text(row("session_started","short"))
        assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        with bus.open("a") as f:f.write('{"status":"session_ended","session_id":"short"}')
        assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        with bus.open("a") as f:f.write("\n")
        assert DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        return dict(rotation=True,truncation=True,partial_tail=True)
def forensic_unended_take_survives_bus_truncation_until_exact_terminal():
    with tempfile.TemporaryDirectory(prefix="cs-L34-unended-",dir="/private/tmp") as d:
        root=Path(d);bus=root/"events.jsonl";cursor={}
        bus.write_text(json.dumps(dict(status="session_started",session_id="unended"))+"\n")
        assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root)
        bus.write_text("")
        assert not DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root), "truncation is not a terminal receipt for a previously observed active take"
        bus.write_text(json.dumps(dict(status="session_ended",session_id="unended"))+"\n")
        assert DEMUX.installation_idle(bus,cursor=cursor,bridge_root=root), "exact terminal receipt must still release it"
        return dict(unended_preserved=True,exact_terminal_releases=True)

class LifecycleForensicsTests(unittest.TestCase):
    def test_cold_wait_respects_deadline(self):
        forensic_cold_wait_respects_deadline()

    def test_cold_and_incremental_current_take_remains_busy(self):
        forensic_cold_and_incremental_current_take_remains_busy()

    def test_short_idle_wait_is_eligible(self):
        forensic_short_idle_wait_is_eligible()

    def test_successive_fresh_playbacks_reuse_canonical_history(self):
        forensic_successive_fresh_playbacks_reuse_canonical_history()

    def test_lifecycle_rotation_truncation_and_partial_tail_stay_safe(self):
        forensic_lifecycle_rotation_truncation_and_partial_tail_stay_safe()

    def test_unended_take_survives_bus_truncation_until_exact_terminal(self):
        forensic_unended_take_survives_bus_truncation_until_exact_terminal()



class GenerationReceiptTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.bus = Path(temporary.name).resolve() / "events.jsonl"
        self.bus.write_bytes(b"retained transcript\n")
        metadata = self.bus.stat()
        self.manifest = {
            "schema": "codescribe.bus-generations.v1", "root": str(self.bus),
            "stream_id": "stream", "stream_inode": metadata.st_ino,
            "stream_dev": metadata.st_dev, "stream_birthtime": None,
            "segments": [], "pending": None,
            "active": {
                "id": "active", "path": str(self.bus), "start": 0,
                "length": metadata.st_size, "dev": metadata.st_dev, "ino": metadata.st_ino,
                "day": None, "compressed": False, "sha256": None, "superseded": None,
            },
        }

    def write_manifest(self):
        Path(str(self.bus) + ".generations.json").write_text(json.dumps(self.manifest))

    def test_volume_uuid_is_optional_and_preserves_stream_identity_and_bytes(self):
        identity = self.bus.stat()
        for value in [None, "41b7ff38-c1d7-48cd-bf70-373435ecf99a"]:
            self.manifest["volume_uuid"] = value
            self.write_manifest()
            segments, dev, ino, _ = DEMUX.generation_sources(self.bus)
            self.assertEqual((dev, ino), (identity.st_dev, identity.st_ino))
            self.assertEqual(segments[-1]["length"], identity.st_size)
            with DEMUX.GenerationFile(self.bus) as reader:
                self.assertEqual(reader.read(256), b"retained transcript\n")
        del self.manifest["volume_uuid"]
        self.write_manifest()
        self.assertEqual(DEMUX.generation_sources(self.bus)[1:3], (identity.st_dev, identity.st_ino))

    def test_malformed_uuid_and_unknown_receipt_fields_are_refused(self):
        for value in [False, 1, {}, "not-a-uuid", "41b7ff38c1d748cdbf70373435ecf99a"]:
            with self.subTest(value=value):
                self.manifest["volume_uuid"] = value
                self.write_manifest()
                with self.assertRaises(ValueError):
                    DEMUX.generation_sources(self.bus)
        self.manifest["volume_uuid"] = "41b7ff38-c1d7-48cd-bf70-373435ecf99a"
        self.manifest["unrecognized"] = True
        self.write_manifest()
        with self.assertRaises(ValueError):
            DEMUX.generation_sources(self.bus)

    def test_volume_uuid_does_not_bypass_physical_linkage_validation(self):
        self.manifest["volume_uuid"] = "41b7ff38-c1d7-48cd-bf70-373435ecf99a"
        self.manifest["active"]["dev"] += 1
        self.write_manifest()
        before = self.bus.read_bytes()
        with self.assertRaises(ValueError):
            DEMUX.generation_sources(self.bus)
        self.assertEqual(self.bus.read_bytes(), before)


class InstallCheckpointTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        fixture = Path(temporary.name).resolve()
        self.root = fixture / "agent-bridge"
        self.root.mkdir()
        self.bus = fixture / "events.jsonl"
        self.bus.touch()

    def append(self, status, session="app", **fields):
        with self.bus.open("a") as handle:
            handle.write(json.dumps({"status": status, "session_id": session, **fields}) + "\n")

    def checkpoint(self):
        cursor = {}
        DEMUX.installation_idle(
            self.bus, sealed_is_idle=False, cursor=cursor, bridge_root=self.root
        )
        self.assertTrue(cursor["caught_up"])
        DEMUX._save_lifecycle_cursor(self.bus, self.root, cursor)
        return cursor

    def idle(self):
        return DEMUX.installation_idle_with_checkpoint(self.bus, bridge_root=self.root)

    def test_managed_rollover_reuses_verified_prefix_and_reads_new_lifecycle(self):
        for _ in range(100):
            self.append("revision", text="retained words " * 40)
        cursor = self.checkpoint()
        original = self.bus.read_bytes()
        metadata = self.bus.stat()
        closed = self.bus.parent / "events" / "undated" / "closed.jsonl"
        closed.parent.mkdir(parents=True)
        os.link(self.bus, closed)
        self.bus.unlink()
        self.bus.touch()
        active = self.bus.stat()
        segment = {
            "id": "closed", "path": str(closed), "start": 0,
            "length": len(original), "dev": metadata.st_dev, "ino": metadata.st_ino,
            "day": None, "compressed": False, "sha256": None, "superseded": None,
        }
        hot = {
            **segment, "id": "active", "path": str(self.bus), "start": len(original),
            "length": 0, "dev": active.st_dev, "ino": active.st_ino, "day": "2026_1004",
        }
        Path(str(self.bus) + ".generations.json").write_text(json.dumps({
            "schema": "codescribe.bus-generations.v1", "root": str(self.bus),
            "stream_id": "stream", "stream_inode": metadata.st_ino,
            "stream_dev": metadata.st_dev,
            "stream_birthtime": getattr(metadata, "st_birthtime", None),
            "segments": [segment], "active": hot, "pending": None,
        }))
        self.append("session_started")
        self.append("session_ended")
        read = DEMUX.GenerationFile.read

        def forbid_prefix_replay(reader, width):
            if width > 256 and reader.tell() < cursor["offset"]:
                raise AssertionError("installer reread the verified historical payload")
            return read(reader, width)

        with patch.object(DEMUX.GenerationFile, "read", forbid_prefix_replay):
            self.assertTrue(self.idle())
        self.assertEqual(closed.read_bytes(), original)

    def test_current_capture_stays_busy_through_seal_until_its_own_end(self):
        self.checkpoint()
        self.append("session_started")
        self.append(DEMUX.SEALED)
        self.assertFalse(self.idle())
        self.append("session_ended", session="other")
        self.assertFalse(self.idle())
        self.append("session_ended")
        self.assertTrue(self.idle())

    def test_independent_cli_without_timestamp_is_not_erased_by_app_end(self):
        self.append("session_started", session="cli", source=DEMUX.CLI_FILE_VERDICT_SOURCE)
        self.checkpoint()
        self.append("session_started")
        self.append("session_ended")
        self.assertFalse(self.idle())
        self.append("session_ended", session="cli", source=DEMUX.CLI_FILE_VERDICT_SOURCE)
        self.assertTrue(self.idle())

    def test_missing_known_channel_does_not_clear_its_open_identity(self):
        channel = self.root / "buses" / "channel-1.jsonl"
        channel.parent.mkdir()
        channel.write_text(json.dumps({
            "schema": DEMUX.CHANNEL_SESSION_SCHEMA, "session_id": "channel-take",
            "channel": "1", "state": "open",
        }) + "\n")
        self.checkpoint()
        channel.unlink()
        self.assertFalse(self.idle())

    def test_replaced_journal_cannot_reuse_old_idle_for_a_new_capture(self):
        self.append("revision")
        self.checkpoint()
        self.bus.rename(self.bus.with_suffix(".previous"))
        self.append("session_started", session="new-capture")
        self.assertFalse(self.idle())

    def test_invalid_checkpoint_still_requires_the_canonical_current_take(self):
        self.checkpoint()
        checkpoint = DEMUX._lifecycle_checkpoint(self.bus, self.root)
        checkpoint.write_text('{"schema":"unknown"}')
        self.append("session_started")
        self.assertFalse(self.idle())

    def test_uncached_probe_preserves_existing_sealed_terminal_rule(self):
        self.append("session_started")
        self.append(DEMUX.SEALED)
        self.assertTrue(self.idle())



class StreamingSpeechTests(unittest.TestCase):
    def test_sentence_chunks_preserve_polish_abbreviations_numbers_and_code(self):
        first = 'Sprawdziliśmy np. lek. wet. wynik 1.25 oraz 23.10.2026 w opisie.'
        second = 'Następne zdanie zawiera `x = 1. A. B.` i kończy się tutaj.'
        third = 'Ostatnie zdanie jest wystarczająco długie do osobnej porcji.'
        self.assertEqual(DEMUX.speech_sentence_chunks(' '.join((first, second, third))),
                         [first, second, third])
        self.assertEqual(DEMUX.speech_sentence_chunks('Krótko.'), ['Krótko.'])
        self.assertEqual(DEMUX.speech_sentence_chunks('To tj. objaśnienie np. liczby.'),
                         ['To tj. objaśnienie np. liczby.'])
        fenced = 'Przykład kodu:\n```python\nx = "Pierwsze. Drugie."\n```\nTo jest jego wyjaśnienie.'
        self.assertEqual(DEMUX.speech_sentence_chunks(fenced), [fenced])
        self.assertTrue(all(len(chunk) >= 40 for chunk in DEMUX.speech_sentence_chunks(
            'Tak. ' + first + ' Koniec.')))

    def test_http_pcm_is_delivered_before_eof_and_handles_split_samples(self):
        import urllib.request
        response = MagicMock(status=200)
        response.read1.side_effect = [b'\x01', b'\x02\x03', b'\x04', b'']
        seen = []
        def receive(pcm):
            seen.append((pcm, response.read1.call_count))
        with patch('urllib.request.OpenerDirector') as factory:
            factory.return_value.open.return_value.__enter__.return_value = response
            pcm, error, reason = DEMUX._tts_exchange(
                urllib.request.Request('https://api.x.ai/v1/tts'), receive)
        self.assertEqual((pcm, error, reason), (b'\x01\x02\x03\x04', None, None))
        self.assertEqual(seen, [(b'\x01\x02', 2), (b'\x03\x04', 3)])
        self.assertTrue(all(call.args == (16384,) for call in response.read1.call_args_list))

    def test_stream_failure_does_not_write_partial_cache(self):
        import urllib.request
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            emitted = []
            fields = {'text': 'incomplete'}
            def synthesize():
                emitted.append(b'\x00\x00')
                return None, 'connection lost', 'network'
            result = DEMUX._cached_speech(root, fields, synthesize)
            self.assertEqual(result, (None, 'connection lost', 'network'))
            self.assertEqual(list((root / 'speech').glob('*.wav')), [])

    def exercise_pipeline(self, *, stop_after_first=False, fail_second=False, take_after_first=False):
        import threading
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        bus = root / 'bus.jsonl'
        bus.touch()
        first = 'Pierwsze zdanie ma wystarczająco wiele znaków, by zacząć mówić od razu.'
        second = 'Drugie zdanie będzie generowane jeszcze podczas odtwarzania pierwszego.'
        text = first + ' ' + second
        started = threading.Event()
        finished = threading.Event()
        stopped = threading.Event()
        take_observed = threading.Event()
        player_disposed = threading.Event()
        order, audio, states, requests = [], [], [], []
        class Player:
            PREFILL_SECONDS = 0.3
            def __init__(self):
                order.append('player')
                self._operations = threading.RLock()
                self.returncode = None
                self.eof = False
            def begin_prefill(self):
                return time.monotonic()
            def start_if_ready(self, *, force=False):
                pass  # Native duration/deadline semantics have their own fixture.
            def enqueue(self, pcm):
                audio.append(pcm)
                started.set()
                order.append('audio')
                if stop_after_first:
                    stopped.set()
                # Keep first playback unfinished while synthesis may prefetch.
                # Cancellation, rather than scheduler timing, releases the feed.
                if take_after_first and not player_disposed.wait(3):
                    raise AssertionError('take guard did not dispose the active player')
            def can_enqueue(self):
                return True
            def finish(self):
                self.eof = True
            def poll(self):
                if take_after_first and self.returncode is None:
                    return None
                return 0 if self.eof else self.returncode
            def terminate(self):
                self.returncode = -15
                player_disposed.set()
            def kill(self):
                self.returncode = -9
                player_disposed.set()
            def wait(self, timeout=None):
                return self.returncode
        def exchange(request, on_pcm=None):
            chunk = json.loads(request.data)['text']
            requests.append(chunk)
            order.append('synthesize')
            if fail_second and len(requests) == 2:
                return None, 'fixture failure', 'network'
            if on_pcm:
                on_pcm(b'\x01\x00')
                # The first PCM must play while the first HTTP response is open.
                if len(requests) == 1:
                    if not started.wait(3):
                        raise AssertionError('player waited for response EOF')
            finished.set()
            order.append('synthesized')
            return b'\x01\x00', None, None
        reply = {'reply_id': 'a' * 24, 'provider': 'codex', 'provider_session_id': 'fixture',
                 'lease_id': DEMUX.lease_identifier('codex', 'fixture'), 'tts_vendor': 'xai',
                 'text': text, 'voice': 'carina', 'speed': 1.0}
        original_stopped = DEMUX.ReplyPlaybackControl.stopped
        def should_stop(control):
            return stopped.is_set() or original_stopped(control)
        def installation_idle(*args, **kwargs):
            if take_after_first and started.is_set():
                take_observed.set()
                return False
            return True
        with (patch.object(DEMUX, 'PCMStreamPlayer', Player),
              patch.object(DEMUX, '_xai_speech_key', return_value='test-only-key'),
              patch.object(DEMUX, '_tts_exchange', side_effect=exchange),
              patch.object(DEMUX, 'bus_path', return_value=bus),
              patch.object(DEMUX, '_wait_for_take_end', return_value=True),
              patch.object(DEMUX, 'installation_idle', side_effect=installation_idle),
              patch.object(DEMUX, '_cursor_holds_take', return_value=True),
              patch.object(DEMUX, '_acquire_playback_lock', return_value=True) as lock,
              patch.object(DEMUX.ReplyPlaybackControl, 'stopped', should_stop),
              patch.object(DEMUX, 'publish_reply_event', side_effect=fixture_publish),
              patch.object(DEMUX, 'emit', side_effect=lambda event: states.append(event)),
              patch.object(DEMUX, 'PLAYBACK_POLL_SECONDS', 0.005)):
            code = DEMUX.speak_published_reply(DEMUX.argparse.Namespace(bridge_home=root),
                                              bus, reply, 'b' * 24)
        self.assertEqual(lock.call_count, 1)
        self.assertEqual(order.count('player'), 1)
        self.assertLess(order.index('audio'), order.index('synthesized'))
        self.assertEqual([event['state'] for event in states[:2]], ['waiting', 'playing'])
        self.assertEqual(sum(event['state'] not in ('waiting', 'playing') for event in states), 1)
        self.assertEqual(len({event['playback_ticket'] for event in states}), 1)
        self.assertTrue(finished.is_set())
        if take_after_first:
            self.assertTrue(take_observed.is_set(), 'the lifecycle owner must observe the new take')
            self.assertTrue(player_disposed.is_set(), 'the new take must cancel active playback')
        return code, audio, states[-1], requests, root

    def test_pipeline_streams_and_prefetches_under_one_ticket_and_lock(self):
        code, audio, terminal, requests, root = self.exercise_pipeline()
        self.assertEqual((code, terminal['state']), (0, 'spoken'))
        self.assertEqual(len(requests), 2)
        self.assertEqual(len(audio), 2)
        self.assertEqual(len(list((root / 'speech').glob('*.wav'))), 2)

    def test_sentence_cache_replay_needs_no_network_or_credential(self):
        _, _, _, chunks, root = self.exercise_pipeline()
        received = []
        def consume(stream, **kwargs):
            stream.start()
            try:
                while True:
                    event = stream.queue.get(timeout=2)
                    if event[0] == 'done':
                        return event[1]
                    received.append(event[1])
            finally:
                stream.close()
        with (patch.object(DEMUX, '_play_pcm_24k', side_effect=consume),
              patch.object(DEMUX, '_xai_speech_key', side_effect=AssertionError('credential on hit')),
              patch.object(DEMUX, '_tts_exchange', side_effect=AssertionError('network on hit'))):
            self.assertEqual(DEMUX._speak_xai(' '.join(chunks), 'carina', 1.0,
                                            playback_root=root), (True, None, None))
        self.assertEqual(received, [b'\x01\x00', b'\x01\x00'])

    def test_stream_cancel_interrupts_active_https_read_and_cache_lock_wait(self):
        import socket
        stream = DEMUX.SpeechPCMStream([], lambda *args: None)
        response = MagicMock()
        stream._response_opened(response)
        stream.close()
        response.fp.raw._sock.shutdown.assert_called_once_with(socket.SHUT_RDWR)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = DEMUX._cached_speech(root, {'text': 'cancelled'},
                                          lambda: self.fail('cancelled synthesis started'),
                                          cancelled=lambda: True)
            self.assertEqual(result, (None, 'playback stopped', 'stopped'))

    def test_stop_between_chunks_has_one_stopped_terminal(self):
        code, audio, terminal, _, _ = self.exercise_pipeline(stop_after_first=True)
        self.assertEqual((code, terminal['state'], terminal['reason']), (5, 'stopped', 'stopped'))
        self.assertEqual(len(audio), 1)

    def test_take_started_between_chunks_refuses_without_more_audio(self):
        code, audio, terminal, _, _ = self.exercise_pipeline(take_after_first=True)
        self.assertEqual((code, terminal['state'], terminal['reason']), (5, 'refused', 'take_started'))
        self.assertEqual(len(audio), 1)

    def test_second_chunk_failure_never_claims_spoken(self):
        code, audio, terminal, _, _ = self.exercise_pipeline(fail_second=True)
        self.assertEqual((code, terminal['state']), (5, 'failed'))
        self.assertFalse(terminal['spoken'])
        self.assertEqual(len(audio), 1)



class TtsLanguageTests(unittest.TestCase):
    """xAI speech is sent in a bare language code, never `auto`, and keyed by it."""

    setUp = SpeechTransportTests.setUp
    cached = SpeechCacheTests.cached

    def environment(self, value=None):
        changes = {} if value is None else {"CS_SAY_LANGUAGE": value}
        scoped = patch.dict(os.environ, changes)
        scoped.start()
        self.addCleanup(scoped.stop)
        if value is None:
            os.environ.pop("CS_SAY_LANGUAGE", None)

    def sent_language(self):
        request = self.opener.open.call_args.args[0]
        return json.loads(request.data)["language"]

    def test_the_code_is_cut_to_its_first_part_and_only_pl_or_en_pass(self):
        for raw, code, warned in (
            (None, "pl", False), ("", "pl", False), ("pl", "pl", False), ("PL", "pl", False),
            ("pl-PL", "pl", False), ("pl_PL", "pl", False), ("en", "en", False), ("en-GB", "en", False),
            ("auto", "pl", True), ("polish", "pl", True), ("xx", "pl", True),
        ):
            with self.subTest(raw=raw):
                environ = {} if raw is None else {"CS_SAY_LANGUAGE": raw}
                error = io.StringIO()
                with contextlib.redirect_stderr(error):
                    self.assertEqual(DEMUX.tts_language(environ), code)
                self.assertEqual("CS_SAY_LANGUAGE" in error.getvalue(), warned)
        self.assertEqual(DEMUX.DEFAULT_TTS_LANGUAGE, "pl")

    def test_the_request_carries_the_language_and_never_auto(self):
        for raw, code in ((None, "pl"), ("pl-PL", "pl"), ("en", "en")):
            with self.subTest(raw=raw):
                self.environment(raw)
                self.opener.open.reset_mock()
                self.assertEqual(DEMUX._speak_xai(f"W nocy było cicho ({raw}).", "eve", 1.25),
                                 (True, None, None))
                self.assertEqual(self.sent_language(), code)
                key = DEMUX.speech_cache_key({"provider": "xai", "voice": "eve", "speed": 1.25,
                                              "language": code, "text": f"W nocy było cicho ({raw})."})
                self.assertIn(f"{key}.wav", self.cached())

    def test_a_recording_made_with_auto_is_not_served_after_the_change(self):
        self.environment()
        text = "W nocy było cicho."
        previous = {"provider": "xai", "voice": "eve", "speed": 1.25, "language": "auto", "text": text}
        DEMUX._cached_speech(None, previous, lambda: (b"\x01\x00", None, None))
        self.assertIn(f"{DEMUX.speech_cache_key(previous)}.wav", self.cached())
        self.assertEqual(DEMUX._speak_xai(text, "eve", 1.25), (True, None, None))
        self.assertEqual(self.opener.open.call_count, 1, "the auto recording is a different key")
        self.assertEqual(self.sent_language(), "pl")
        self.assertEqual(DEMUX._speak_xai(text, "eve", 1.25), (True, None, None))
        self.assertEqual(self.opener.open.call_count, 1, "the pl recording is reused")

    def test_long_text_sends_every_sentence_in_the_language(self):
        # Bounded on purpose: the streaming path loops on `read1()`, and the
        # transport mock of `setUp` never returns an empty read, so this test
        # replaces the exchange with a finite one and drains the stream itself.
        self.environment()
        text = ("W nocy było cicho i nikt nie pisał na kanale zero. "
                "U nas jest cicho, bramka na Sztudio skończyła się dobrze. "
                "A co dalej, to powie nam Monika po odsłuchu.")
        self.assertEqual(len(DEMUX.speech_sentence_chunks(text)), 3)
        languages = []

        def exchange(request, on_pcm=None):
            if len(languages) >= 10:
                raise AssertionError("more requests than sentences")
            languages.append(json.loads(request.data)["language"])
            return b"\x01\x00", None, None

        def play(stream, **_):
            stream.start()
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                kind, value = stream.queue.get(timeout=5)
                if kind == "done":
                    return value
            stream.cancelled.set()
            raise AssertionError("the sentence stream did not finish")

        with patch.object(DEMUX, "_tts_exchange", side_effect=exchange), \
                patch.object(DEMUX, "_play_pcm_24k", side_effect=play):
            self.assertEqual(DEMUX._speak_xai(text, "eve", 1.25), (True, None, None))
        self.assertEqual(len(languages), 3, "one request per sentence")
        self.assertEqual(set(languages), {"pl"})
        self.opener.open.assert_not_called()


class SpokenTextTests(unittest.TestCase):
    """Words for listening; examples are sentences the bus voice really spoke."""

    NOW = datetime.datetime(2026, 10, 26, 12, 0, tzinfo=datetime.timezone.utc)
    ZONE = datetime.timezone(datetime.timedelta(hours=1))

    def spoken(self, text, language="pl"):
        return DEMUX.spoken_text(text, language, now=self.NOW, zone=self.ZONE)

    def test_each_rule_on_a_real_sentence(self):
        cases = [
            ("hania → as: Zgoda na dopisanie Readera do Modify.",
             "Hania: Zgoda na dopisanie Readera do Modify."),
            ("as (kanał 1) → Monika, Basia: W2-03 przyjęte.", "As: W2-03 przyjęte."),
            ("Helper przeszedł z 0.17.0 → 0.18.0 bez przerwy.",
             "Helper przeszedł z 0.17.0 na 0.18.0 bez przerwy."),
            ("Na Silverze zwykłe tbflow cf44b428 daje aktualną paczkę.",
             "Na Silverze zwykłe tbflow daje aktualną paczkę."),
            ("Mają wspólne message_id z odpowiedziami (668ee033…, 20c567f4…), które leżą w kanale.",
             "Mają wspólne message id z odpowiedziami, które leżą w kanale."),
            ("basia: przepięta na 0.18.0+gb2e9b3a7, ten sam lease.",
             "basia: przepięta na 0.18.0, ten sam lease."),
            ("Paczka ~/Downloads/codescribe-precedent-20261025.md czeka na odczyt.",
             "Paczka codescribe precedent czeka na odczyt."),
            ("Zmiana jest w core/llm/speech.rs i w testach.", "Zmiana jest w speech i w testach."),
            ("Nagrania są w katalogu ~/AI_artifacts/w2-03-samples/.",
             "Nagrania są w katalogu w2 03 samples."),
            ("Sprawdziłam to w bus-demux.py:5350–5352 i w scripts/bus-demux.py.",
             "Sprawdziłam to w bus demux i w bus demux."),
            ("Kopia ma sent_to = [as/1, basia/2], czyli 2/4 odbiorców.",
             "Kopia ma sent to = [as, kanał 1, basia, kanał 2], czyli 2 z 4 odbiorców."),
            ("Pokwitowania trafiają do runtime/receipts od razu.",
             "Pokwitowania trafiają do runtime receipts od razu."),
            ("Nowa sesja przejmie kanał przez --takeover.", "Nowa sesja przejmie kanał przez takeover."),
            ("Monika dała zgodę o 11:53:17Z, a bramka szła 12:51–12:53.",
             "Monika dała zgodę o 12:53, a bramka szła 12:51 do 12:53."),
            ("Odpowiedzi z 23:35:10Z i 23:37:07Z.", "Odpowiedzi z 0:35 i 0:37."),
            ("Kanał 1 skończył o 05:14:56.", "Kanał 1 skończył o 05:14."),
            ("Wynik:\n- cargo 18/18\n- `make check` zielone", "Wynik: cargo 18 z 18. make check zielone"),
            ("Komenda:\n```\nmake check\n```\nGotowe.", "Komenda: make check. Gotowe."),
            ("Y = sent_to ∪ odbiorcy, zob. §5 i <lease>.",
             "Y = sent to i odbiorcy, zob. paragraf 5 i lease."),
        ]
        for written, expected in cases:
            with self.subTest(written=written):
                self.assertEqual(self.spoken(written), expected)

    def test_english_uses_english_words(self):
        self.assertEqual(self.spoken("as → nela: moved 0.17.0 → 0.18.0, 2/4 done.", "en"),
                         "As: moved 0.17.0 to 0.18.0, 2 of 4 done.")

    def test_plain_text_is_returned_exactly_as_written(self):
        # Unchanged text keeps its speech cache key, so nothing is synthesized again.
        for written in ("W nocy było cicho, a rano wszystko działało jak trzeba.", "Zrobione",
                        "Wiadomość agenta.\n", "Tak.  Dwie spacje zostają."):
            with self.subTest(written=written):
                self.assertEqual(self.spoken(written), written)

    def test_text_with_nothing_left_to_say_is_spoken_as_written(self):
        for written in ("0786cc92", "   ", ""):
            with self.subTest(written=written):
                self.assertEqual(self.spoken(written), written)

    def test_long_unbroken_tokens_stay_linear(self):
        started = time.monotonic()
        result = self.spoken(("a." * 50_000) + " " + ("ab1" * 50_000) + " " + ("x_" * 50_000))
        self.assertLess(time.monotonic() - started, 5.0)
        self.assertTrue(result)


if __name__ == "__main__":
    unittest.main()
