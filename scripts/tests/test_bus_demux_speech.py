"""Fixed-origin speech transport; no credentials, network or speaker access."""

import contextlib
import http.client
import importlib.util
import io
import json
import multiprocessing
import os
import fcntl
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
        with (
            patch.object(DEMUX, "_speak_xai", return_value=speaker) as spoken,
            patch.object(sys, "argv", argv),
            contextlib.redirect_stdout(out),
        ):
            code = DEMUX.main()
        return code, json.loads(out.getvalue()), spoken

    def test_name_comes_from_the_lease_when_not_given(self):
        self.attach_lease("session-a", "filip")
        code, reply, spoken = self.say("--session", "session-a")
        self.assertEqual(code, 0)
        self.assertEqual(reply["name"], "filip")
        # The resolved name selects the profile, exactly as an explicit one.
        spoken.assert_called_once_with(
            "Zrobione.", "rex", 1.1, playback_root=self.home, bus=self.bus
        )

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
            bus=self.bus,
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
        self.assertEqual(reply["reason"], "quota_exhausted")
        row = json.loads(self.bus.read_text())
        self.assertEqual(row["reason"], "quota_exhausted")
        self.assertEqual(row["tts_error"], "tts request failed (403)")


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
):
    """Real --say and flock in a process; only synthesis and afplay are replaced."""
    DEMUX.PLAYBACK_WAIT_SECONDS = wait_seconds
    DEMUX.TAKE_WAIT_SECONDS = take_wait_seconds
    DEMUX.PLAYBACK_POLL_SECONDS = 0.01
    acquire = DEMUX._acquire_playback_lock
    playback_log = Path(home) / "playback.jsonl"
    sentinel = Path(home) / "playing"

    def synthesize(_request):
        synthesized.set()
        return b"\x00\x00", None, None

    def lock(descriptor):
        waiting.set()
        return acquire(descriptor)

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
            if release is None or release.is_set():
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
        patch("subprocess.Popen", side_effect=Player),
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

    def replies(self):
        return [
            row
            for line in self.bus.read_text().splitlines()
            if (row := json.loads(line)).get("kind") == "agent_reply"
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
        self.assertEqual(len(self.replies()), 2)
        self.assertTrue(all(row["spoken"] for row in self.replies()))

    def test_timeout_writes_playback_busy_and_never_plays(self):
        lock = self.hold_lock()
        child, (_, waiting, playing) = self.start_say("astra", wait_seconds=0.1)
        self.assertTrue(waiting.wait(5))
        self.join(child)
        self.assertFalse(playing.is_set())
        (reply,) = self.replies()
        self.assertFalse(reply["spoken"])
        self.assertEqual(reply["reason"], "playback_busy")
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
        # The timed-out waiter must not remove the shared lock inode.
        self.assertTrue((self.home / "runtime" / "playback.lock").exists())

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
        (reply,) = self.replies()
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
        (reply,) = self.replies()
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
        (reply,) = self.replies()
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
        (reply,) = self.replies()
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
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        initial_offset = cursor["offset"]
        with self.bus.open("a") as stream:
            stream.write(
                json.dumps({"status": "session_ended", "session_id": "first"}) + "\n"
            )
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        self.assertGreater(cursor["offset"], initial_offset)
        self.bus.rename(self.bus.with_suffix(".old"))
        self.bus.write_text(
            json.dumps({"status": "session_started", "session_id": "second"}) + "\n"
        )
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        self.bus.write_text("")
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )

    def test_idle_cursor_refuses_partial_tail_until_the_row_completes(self):
        cursor = {}
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        self.bus.write_text('{"status": "session_started", "session_id":')
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        with self.bus.open("a") as stream:
            stream.write('"take"}\n')
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        with self.bus.open("a") as stream:
            stream.write(
                json.dumps({"status": "session_ended", "session_id": "take"}) + "\n"
            )
        self.assertTrue(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )

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


if __name__ == "__main__":
    unittest.main()
