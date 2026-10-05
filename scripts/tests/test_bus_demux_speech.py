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
from pathlib import Path
import sys
import tempfile
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
        # Erasing history is not a terminal receipt for the observed live take.
        self.assertFalse(
            DEMUX.installation_idle(self.bus, sealed_is_idle=False, cursor=cursor)
        )
        self.bus.write_text(
            json.dumps({"status": "session_ended", "session_id": "second"}) + "\n"
        )
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

    def test_sealed_channel_lets_speech_play(self):
        channel = self.home / "buses" / "channel-2.jsonl"
        self.channel_row(channel, "agent-channel-2-done", "open")
        self.channel_row(channel, "agent-channel-2-done", "sealed")
        child, (_, _, playing) = self.start_say("filip")
        self.join(child)
        self.assertTrue(playing.is_set())
        (reply,) = self.outcomes()
        self.assertTrue(reply["spoken"])

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


if __name__ == "__main__":
    unittest.main()
