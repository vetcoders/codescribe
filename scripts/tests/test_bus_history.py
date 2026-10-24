"""`cs-bus --history`: the retained channel conversation as Markdown, read only."""
import argparse
import contextlib
import datetime
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_history", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

START = datetime.datetime(2026, 10, 18, 21, 0, tzinfo=datetime.timezone.utc)


def stamp(seconds):
    return (START + datetime.timedelta(seconds=seconds)).isoformat(timespec="microseconds")


def typed(identity, audience, channel, seconds, text, origin="0"):
    return {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message", "source": "typed",
            "message_id": identity, "source_event_id": identity, "audience": audience, "channel": channel,
            "origin_channel": origin, "emitted_at": stamp(seconds), "text": text}


def reply(identity, sender, seconds, text, *, peer_to=None, target=None):
    row = {"schema": DEMUX.AGENT_REPLY_SCHEMA, "kind": "agent_reply", "message_id": identity,
           "reply_id": identity, "source_event_id": identity, "emitted_at": stamp(seconds), "text": text,
           "sender": {"name": sender, "provider": "claude-code", "provider_session_id": sender,
                      "lease_id": "0" * 32}, "target": target}
    if peer_to is not None:
        row["peer_to"] = peer_to
    return row


def control(kind, identity, seconds, **extra):
    return {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": kind, "identity": identity,
            "message_id": identity, "control_id": f"{seconds:024x}", "emitted_at": stamp(seconds), **extra}


def channel_take(session, seconds, text):
    """One channel-0 take: opened, one reducer snapshot, closed."""
    return [
        {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "kind": "channel_session", "channel": "0",
         "session_id": session, "state": "open", "opened_at": stamp(seconds - 5), "emitted_at": stamp(seconds - 5)},
        {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session, "audience": "*", "channel": "0",
         "rendered_text": text, "reducer_revision": 1, "reducer_action": "apply_ledger_decision",
         "sequence": 1, "document_index": 0, "emitted_at": stamp(seconds - 1)},
        {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "kind": "channel_session", "channel": "0",
         "session_id": session, "state": "closed", "emitted_at": stamp(seconds)},
    ]


class BusHistoryTests(unittest.TestCase):
    def setUp(self):
        scratch = Path(__file__).resolve().parents[2] / "build" / "test-scratch"
        scratch.mkdir(parents=True, exist_ok=True)
        temp = tempfile.TemporaryDirectory(prefix="bus-history-", dir=scratch)
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve() / "bridge"
        (self.root / "buses").mkdir(parents=True)
        take = channel_take("agent-channel-0-take", 10, "Dobra, słuchajcie.\n## not a heading")
        broadcast = {"channel": "0", "mode": "broadcast", "recipients": [
            {"channel": "1", "name": "ada"}, {"channel": "2", "name": "stasiu"}]}
        self.write("1", [
            *take,
            typed("a" * 24, "ada", "1", 20, "zaciągnij"),
            reply("c" * 24, "explorek", 40, "lista gotowa", peer_to="ada", target=broadcast),
            reply("d" * 24, "stasiu", 50, "biorę sample", peer_to="ada",
                  target={"mode": "direct", "recipients": [{"channel": "1", "name": "ada"}]}),
            {"schema": "codescribe.agent-ack.v1", "kind": "agent_ack", "delivery_id": "f" * 24,
             "emitted_at": stamp(55)},
        ])
        self.write("2", [
            *take,
            typed("a" * 24, "stasiu", "2", 20, "zaciągnij"),
            typed("b" * 24, "stasiu", "2", 25, "nie - to nie jest albo albo"),
            control("withdraw", "b" * 24, 27),
            typed("e" * 24, "stasiu", "2", 30, "pierwsza wersja", origin="2"),
            control("revise", "e" * 24, 32, text="poprawiona wersja"),
            control("release", "e" * 24, 33),
            reply("c" * 24, "explorek", 40, "lista gotowa", peer_to="stasiu", target=broadcast),
            reply("9" * 24, "stasiu", 51, "biorę sample", peer_to="explorek",
                  target={"mode": "direct", "recipients": [{"channel": "3", "name": "explorek"}]}),
            reply("8" * 24, "ada", 60, "zainstalowane"),
        ])

    def write(self, channel, rows):
        path = self.root / "buses" / f"channel-{channel}.jsonl"
        path.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")

    def history(self, **overrides):
        values = dict(bridge_home=self.root, since=None, until=None, channel=None)
        values.update(overrides)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(DEMUX.history_command(argparse.Namespace(**values)), 0)
        return out.getvalue()

    def headings(self, markdown):
        return [line.split(" · ", 1)[1] for line in markdown.splitlines() if line.startswith("### ")]

    def test_each_message_reads_once_in_order_with_its_lane(self):
        markdown = self.history()
        self.assertEqual(self.headings(markdown), [
            "Founder (voice) → all",
            "Founder (typed) → all",
            "Founder (typed) → stasiu (revised before delivery)",
            "explorek → all",
            "stasiu → ada, explorek",
            "ada → Founder",
        ])
        self.assertIn("- Messages: 6 (Founder voice 1, Founder typed 2, agents 3)", markdown)
        self.assertIn("- Withdrawn before delivery, not shown: 1 (whole history)", markdown)

    def test_withdrawn_text_is_gone_and_revision_replaces_the_held_words(self):
        markdown = self.history()
        self.assertNotIn("albo albo", markdown)
        self.assertNotIn("pierwsza wersja", markdown)
        self.assertIn("> poprawiona wersja", markdown)

    def test_message_text_is_quoted_so_it_cannot_open_a_section(self):
        markdown = self.history()
        self.assertIn("> Dobra, słuchajcie.\n> ## not a heading", markdown)
        self.assertNotIn("\n## not a heading", markdown)

    def test_window_and_channel_filters(self):
        since = (START + datetime.timedelta(seconds=35)).isoformat()
        self.assertEqual(self.headings(self.history(since=since)),
                         ["explorek → all", "stasiu → ada, explorek", "ada → Founder"])
        self.assertEqual(self.headings(self.history(channel="0")),
                         ["Founder (voice) → all", "Founder (typed) → all", "explorek → all"])
        self.assertEqual(self.headings(self.history(channel="1")), [
            "Founder (voice) → all", "Founder (typed) → all", "explorek → all", "stasiu → ada, explorek"])

    def test_relative_and_local_moments(self):
        now = datetime.datetime(2026, 10, 19, 6, 0, tzinfo=datetime.timezone.utc)
        self.assertEqual(DEMUX.history_moment("90m", now), now - datetime.timedelta(minutes=90))
        self.assertEqual(DEMUX.history_moment("2d", now), now - datetime.timedelta(days=2))
        local = DEMUX.history_moment("2026-10-18", now)
        self.assertIsNotNone(local.tzinfo)
        self.assertEqual((local.year, local.month, local.day, local.hour), (2026, 10, 18, 0))
        self.assertEqual(DEMUX.history_time("2026-10-19T03:30:14.123456789Z"),
                         datetime.datetime(2026, 10, 19, 3, 30, 14, 123456, tzinfo=datetime.timezone.utc))

    def test_history_reads_only(self):
        before = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        self.history()
        after = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main()
