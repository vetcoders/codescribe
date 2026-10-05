"""Integrator: packaged Python -> real Rust append owner -> exact reply reader.

Only the speaker is substituted. All bus, manifest and control files are temporary.
The caller must build the canonical CLI; an absent executable fails this test.
"""

import argparse
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]


class PackagedReplyPublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.stage = Path(cls.temporary.name).resolve() / "payload"
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
        executable = target / "debug" / "codescribe"
        subprocess.run([str(ROOT / "scripts/build-app.sh"), "--stage-agent-bridge",
                        str(cls.stage), "0.15.3", str(executable.resolve())],
                       check=True, capture_output=True)
        spec = importlib.util.spec_from_file_location("packaged_reply_publisher",
                                                      cls.stage / "bin/bus-demux.py")
        cls.demux = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = cls.demux
        spec.loader.exec_module(cls.demux)

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.home = self.root / "agent-bridge"
        shutil.copytree(self.stage, self.home / "runtime")
        self.bus = self.root / "bus.jsonl"
        self.args = argparse.Namespace(
            bridge_home=self.home, bus=self.bus, name="lena", provider="codex",
            session="publisher-test", say="Iwo " * 200000, voice="eve",
            speed=1.0, tts_vendor="xai", reply_to=None, bus_overridden=True,
        )
        scoped = patch.object(self.demux, "bus_path", return_value=self.bus)
        scoped.start()
        self.addCleanup(scoped.stop)

    def say(self, speaker):
        with patch.object(self.demux, "_speak_xai", side_effect=speaker), \
                contextlib.redirect_stdout(io.StringIO()):
            return self.demux.say_reply(self.args)

    def published(self):
        source = next((self.home / "runtime/reply-sources").glob("*.json"))
        return self.demux.load_published_reply(self.args, source.stem)

    def test_packaged_publisher_commits_chunked_text_before_speech_and_can_replay(self):
        def speaker(text, *_args, **_kwargs):
            bus, reply = self.published()
            self.assertEqual(bus, self.bus)
            self.assertEqual(reply["text"], text)
            self.assertFalse(reply["spoken"])
            return True, None, None

        # A packaged publisher must work without finding a developer CLI.
        with patch("shutil.which", side_effect=AssertionError("unexpected PATH lookup")):
            self.assertEqual(self.say(speaker), 0)
        self.assertTrue(b'codescribe.bus-chunk.v1' in self.bus.read_bytes(),
                        'long reply must exercise actual chunk framing')
        bus, reply = self.published()
        self.assertEqual(reply["text"], self.args.say)
        self.args.play_reply = reply["reply_id"]
        self.args.playback_ticket = "e" * 24
        with patch.object(self.demux, "_speak_xai", side_effect=speaker), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(self.demux.play_reply_command(self.args), 0)
        self.assertEqual((bus.stat().st_mode & 0o777), 0o600)
        self.assertTrue(Path(str(bus) + ".generations.json").is_file())

    def test_foreign_provider_session_cannot_replay_published_text(self):
        self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
        _, reply = self.published()
        self.args.session = "another-session"
        with self.assertRaises(ValueError):
            self.demux.load_published_reply(self.args, reply["reply_id"])

    def test_tampered_packaged_publisher_is_refused_before_speech(self):
        publisher = self.home / "runtime/bin/codescribe"
        with publisher.open("ab") as handle:
            handle.write(b"tampered")
        with patch.object(self.demux, "_speak_xai") as speaker:
            with self.assertRaises(ValueError):
                self.demux.say_reply(self.args)
            speaker.assert_not_called()
        self.assertFalse(self.bus.exists())


    def test_canonical_publisher_refuses_uppercase_reply_and_playback_ids(self):
        reply = {"schema": "codescribe.agent-reply.v1", "kind": "agent_reply",
                 "reply_id": "a" * 24, "provider": "codex", "provider_session_id": self.args.session,
                 "lease_id": self.demux.lease_identifier("codex", self.args.session),
                 "emitted_at": "2026-10-05T06:00:00Z", "text": "Iwo", "spoken": False,
                 "association": "unsolicited", "delivery_id": None}
        cases = [{**reply, "reply_id": "A" * 24},
                 {**reply, "association": "addressed", "delivery_id": "B" * 24},
                 {**reply, "schema": "codescribe.agent-reply-playback.v1", "kind": "agent_reply_playback",
                  "state": "waiting", "playback_ticket": "C" * 24}]
        for event in cases:
            with self.subTest(event=event["kind"]), self.assertRaises(OSError):
                self.demux.publish_reply_event(self.bus, event, bridge_root=self.home)
        self.assertFalse(self.bus.exists(), "invalid IDs must not enter the journal")


if __name__ == "__main__":
    unittest.main()
