"""A spoken channel-zero message to All needs at least three words.

Maciej, 2026-11-08: an open microphone on channel zero turned a stray
"Dziękuję!" or "Dzień dobry" into a message that woke every agent. The rule
is a length rule on the closing snapshot of a voice take to All; it is not an
acoustic judgement, and nothing else is gated by it: typed text, peer replies,
the direct channels 1-9, ordinary dictation and messages carrying attachments
still deliver as before.

The first class pins the counting rule and the `consider` admission owner.
The second runs the real follower over channel-zero takes shaped like the
rows the app writes, with the production flags (`--drafts --coalesce`).
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

SPEC = importlib.util.spec_from_file_location(
    "bus_short_voice_broadcast", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

PROVIDER, CONVERSATION, NAME = "claude-code", "conversation-short-voice", "jozek"


def sealed_voice(text, *, session="agent-channel-0-take", audience="*", mode="agent", **extra):
    row = {"schema": DEMUX.CLEAN_SCHEMA, "status": DEMUX.SEALED, "session_id": session,
           "audience": audience, "mode": mode, "channel": "0", "text": text}
    row.update(extra)
    return row


def admitted(event, *, drafts=False, name=NAME):
    return DEMUX.consider(event, name=name, hear_all=False, drafts=drafts, debug=False)


class CountingRuleTests(unittest.TestCase):
    def test_fewer_than_three_words_is_short(self):
        for text in ("Dziękuję!", "Dzień dobry", "Kto tam?", "Dobra.", "  ...  ", "", "!!! — ?"):
            with self.subTest(text=text):
                self.assertTrue(DEMUX.short_spoken_broadcast(sealed_voice(text)))

    def test_three_words_and_more_are_a_message(self):
        for text in ("Dzień dobry wszystkim", "No i jak ta!", "Kto tam? Jak się nazywasz?",
                     "żółć gęś jaźń", "3 koty śpią"):
            with self.subTest(text=text):
                self.assertFalse(DEMUX.short_spoken_broadcast(sealed_voice(text)))

    def test_punctuation_never_inflates_the_count(self):
        for text in ("Dzień , dobry", "Dzień — dobry", "Dzień . . . dobry", "Dzień _ dobry",
                     "Dzień - dobry", "Dzień ' dobry"):
            with self.subTest(text=text):
                self.assertTrue(DEMUX.short_spoken_broadcast(sealed_voice(text)))

    def test_uncertainty_markup_is_not_a_word(self):
        for text in ("<unclear>dobra</unclear>", "<unclear></unclear> Dobra <UNCLEAR>tak</UNCLEAR>",
                     "Dobra <unclear>"):
            with self.subTest(text=text):
                self.assertTrue(DEMUX.short_spoken_broadcast(sealed_voice(text)))
        self.assertFalse(DEMUX.short_spoken_broadcast(
            sealed_voice("<unclear>Dobra</unclear> to idziemy")),
            "words inside the tags are still words; only the tags are not")

    def test_internal_hyphens_and_apostrophes_keep_one_word(self):
        for text in ("biało-czerwony dom", "rock'n'roll gra", "O’Neil śpi", "ping-pong-ping tak"):
            with self.subTest(text=text):
                self.assertTrue(DEMUX.short_spoken_broadcast(sealed_voice(text)))
        self.assertFalse(DEMUX.short_spoken_broadcast(sealed_voice("biało czerwony dom")))


class AdmissionTests(unittest.TestCase):
    """`consider` is the one owner: what it refuses never reaches a mailbox."""

    def test_short_spoken_all_seal_is_refused_and_three_words_pass(self):
        self.assertIsNone(admitted(sealed_voice("Dzień dobry")))
        payload = admitted(sealed_voice("Dzień dobry wszystkim"))
        self.assertIsNotNone(payload)
        self.assertEqual(payload["text"], "Dzień dobry wszystkim")

    def test_the_evidence_schema_is_gated_like_the_clean_one(self):
        self.assertIsNone(admitted(sealed_voice("Dziękuję!", schema=DEMUX.EVIDENCE_SCHEMA)))

    def test_typed_one_word_to_an_agent_still_delivers(self):
        typed = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
                 "source": "typed", "text": "Dzięki", "message_id": "a" * 24,
                 "source_event_id": "a" * 24, "audience": NAME, "channel": "0",
                 "session_id": "agent-channel-0-typed"}
        payload = admitted(typed)
        self.assertIsNotNone(payload)
        self.assertEqual(payload["text"], "Dzięki")

    def test_a_one_word_peer_reply_still_delivers(self):
        reply = {"schema": DEMUX.AGENT_REPLY_SCHEMA, "kind": "agent_reply", "peer_to": NAME,
                 "text": "OK", "reply_id": "b" * 24, "message_id": "b" * 24,
                 "source_event_id": "b" * 24, "channel": "0",
                 "sender": {"name": "lena", "provider": "codex", "provider_session_id": "s",
                            "lease_id": "c" * 32}}
        payload = admitted(reply)
        self.assertIsNotNone(payload)
        self.assertEqual(payload["text"], "OK")

    def test_direct_channels_are_not_gated(self):
        for channel in range(1, 10):
            for audience in (NAME, "*"):
                with self.subTest(channel=channel, audience=audience):
                    event = sealed_voice("Dzięki", session=f"agent-channel-{channel}-take",
                                         audience=audience, channel=str(channel))
                    self.assertFalse(DEMUX.short_spoken_broadcast(event))
                    self.assertIsNotNone(admitted(event))

    def test_channel_zero_voice_to_one_named_agent_is_outside_the_rule(self):
        """The documented scope is a message to All (`audience: "*"`)."""
        event = sealed_voice("Dzięki", audience=NAME)
        self.assertFalse(DEMUX.short_spoken_broadcast(event))
        self.assertIsNotNone(admitted(event))

    def test_ordinary_dictation_is_not_gated(self):
        self.assertFalse(DEMUX.short_spoken_broadcast(sealed_voice("Dzięki", mode="dictation")))
        self.assertFalse(DEMUX.short_spoken_broadcast(
            sealed_voice("Dzięki", session="dictation-session-1")))

    def test_a_short_caption_with_an_attachment_still_delivers(self):
        pointer = {"path": "/tmp/image.png", "name": "image.png", "media_type": "image/png",
                   "bytes": 13, "sha256": "a" * 64}
        payload = admitted(sealed_voice("Zobacz", attachments=[pointer]))
        self.assertIsNotNone(payload)

    def test_a_live_draft_is_not_settled_early(self):
        draft = sealed_voice("Dzień", status=DEMUX.LIVE_STATUSES[0])
        self.assertIsNotNone(admitted(draft, drafts=True),
                             "a draft may still grow into a message of three words")


class ChannelZeroFollowerTests(unittest.TestCase):
    """The real follower over channel-zero takes, with the production flags."""

    WINDOW = 32768

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "agent-bridge"
        self.bus = self.home / "channel-1.jsonl"
        self.bus.touch()
        self.lease_id = DEMUX.lease_identifier(PROVIDER, CONVERSATION)
        self.lease_path = self.root / "leases" / f"{self.lease_id}.json"
        self.recipients = [{"audience": NAME, "bus": str(self.bus.resolve()), "channel": "1",
                            "lease_id": self.lease_id, "name": NAME, "provider": PROVIDER,
                            "provider_session_id": CONVERSATION}]
        self.sequence = 0
        self.hook_marks = self.home / "seal-hook"

    def coordinates(self, session, index):
        self.sequence += 1
        start = 4096 + index * self.WINDOW
        return {"sequence": self.sequence, "capture_epoch": 1, "sample_start": start,
                "sample_end": start + self.WINDOW - 2048, "document_index": index,
                "emitted_at": f"2026-11-08T10:00:{self.sequence % 60:02d}.000000Z",
                "occurrence_session_id": session, "label": "Maciej",
                "acoustic_receipts": [{"receipt_id": f"pcm-{session}-{index}"}]}

    def publication(self, session, revision, action, text, primary=0, carried=(), **extra):
        row = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session, "audience": "*",
               "channel": "0", "mode": "agent", "recipients": self.recipients,
               "reducer_action": action, "reducer_revision": revision, "rendered_text": text,
               "persistence_encoding": "shared-revision.v1", **self.coordinates(session, primary)}
        row["occurrence_rows"] = [self.coordinates(session, index) for index in carried]
        row.update(extra)
        return row

    def channel_row(self, session, state):
        return {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "kind": "channel_session", "channel": "0",
                "session_id": session, "state": state, "reason": "opened" if state == "open" else "hangup",
                "opened_at": "2026-11-08T10:00:00Z", "emitted_at": "2026-11-08T10:00:00Z",
                "agent": NAME, "provider": PROVIDER, "provider_session_id": CONVERSATION,
                "recipients": self.recipients}

    def write(self, rows):
        with self.bus.open("a", encoding="utf-8") as stream:
            for row in rows:
                stream.write(json.dumps(row, ensure_ascii=False) + "\n")

    def speak(self, session, drafts, final, *, seal=True, **seal_extra):
        """One channel-zero take: open, the drafts as spoken, the seal, the hangup."""
        rows = [self.channel_row(session, "open")]
        for revision, text in enumerate(drafts, start=1):
            rows.append(self.publication(session, revision, "apply_ledger_decision", text))
        if seal:
            rows.append(self.publication(session, len(drafts) + 1, "seal_coverage", final, **seal_extra))
        rows.append(self.channel_row(session, "sealed"))
        self.write(rows)

    def run_demux(self, *extra, check=True):
        return subprocess.run([sys.executable, str(SPEC.origin), "--bus", str(self.bus), "--bridge-home",
                               str(self.root), "--provider", PROVIDER, "--session", CONVERSATION, *extra],
                              capture_output=True, text=True, timeout=30, check=check,
                              env={**os.environ, "CODESCRIBE_DATA_DIR": str(self.home)})

    def follow(self):
        hook = f"echo \"$CODESCRIBE_SEAL_TEXT\" >> '{self.hook_marks}'"
        result = self.run_demux("--name", NAME, "--from-start", "--drafts", "--coalesce", "--on-seal", hook)
        return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]

    def seals(self, emitted):
        return [item for item in emitted if item.get("kind") in DEMUX.TERMINAL_KINDS]

    def state(self):
        return DEMUX.read_json(self.lease_path)

    def hook_runs(self, expected):
        """The hook is detached; wait for the expected count, then for strays."""
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            lines = self.hook_marks.read_text().splitlines() if self.hook_marks.exists() else []
            if len(lines) >= expected:
                break
            time.sleep(0.05)
        time.sleep(0.3)
        return self.hook_marks.read_text().splitlines() if self.hook_marks.exists() else []

    def assert_nothing_reached_the_agent(self, emitted):
        self.assertEqual(self.seals(emitted), [], "no follower message")
        state = self.state()
        self.assertEqual(state["pending"], [], "nothing waits in the mailbox, drafts included")
        self.assertEqual(state.get("held", []), [], "nothing is held for later release")
        self.assertEqual(self.hook_runs(0), [], "the seal hook never fired")

    def test_one_word_take_reaches_nobody(self):
        self.speak("agent-channel-0-thanks", ["Dziękuję!"], "Dziękuję!")
        self.assert_nothing_reached_the_agent(self.follow())

    def test_two_word_take_reaches_nobody(self):
        self.speak("agent-channel-0-hello", ["Dzień", "Dzień dobry"], "Dzień dobry")
        self.assert_nothing_reached_the_agent(self.follow())

    def test_a_held_short_take_is_never_held(self):
        until = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 600))
        self.speak("agent-channel-0-held", ["Dobra"], "Dobra", hold_until=until)
        self.assert_nothing_reached_the_agent(self.follow())

    def test_a_take_closed_without_a_seal_row_is_gated_too(self):
        self.speak("agent-channel-0-hangup", ["Kto tam?"], None, seal=False)
        self.assert_nothing_reached_the_agent(self.follow())

    def test_three_words_deliver_once_and_fire_the_hook(self):
        self.speak("agent-channel-0-three", ["Dzień", "Dzień dobry", "Dzień dobry wszystkim"],
                   "Dzień dobry wszystkim")
        seals = self.seals(self.follow())
        self.assertEqual([item["text"] for item in seals], ["Dzień dobry wszystkim"])
        self.assertEqual([item["kind"] for item in self.state()["pending"] if item["kind"] in DEMUX.TERMINAL_KINDS],
                         ["seal"])
        self.assertEqual(self.hook_runs(1), ["Dzień dobry wszystkim"])

    def test_a_short_take_leaves_nothing_beside_the_next_message(self):
        self.speak("agent-channel-0-stray", ["Dzięki"], "Dzięki")
        self.speak("agent-channel-0-real", ["No", "No i", "No i jak ta"], "No i jak ta")
        seals = self.seals(self.follow())
        self.assertEqual([item["text"] for item in seals], ["No i jak ta"])
        self.assertEqual([item for item in self.state()["pending"]
                          if item["session_id"] == "agent-channel-0-stray"], [])

    def test_refused_takes_do_not_pile_up_or_come_back_on_resume(self):
        """Nothing acknowledges a draft: only its message's terminal envelope
        settles it. A refused take has no terminal envelope, so its previews
        must leave with the refusal, or every interjection stays in the lease
        toward the 256 cap and is published again by every resume."""
        for number in range(3):
            self.speak(f"agent-channel-0-stray-{number}", ["Dzięki"], "Dzięki")
        self.follow()
        self.assertEqual(self.state()["pending"], [])
        resumed = self.follow()
        self.assertEqual([item for item in resumed if item.get("kind") in DEMUX.DRAFT_KINDS], [])

    def test_once_finds_nothing_to_deliver_for_a_short_take(self):
        self.speak("agent-channel-0-once", ["Dzięki"], "Dzięki")
        for flags in ((), ("--drafts",)):
            with self.subTest(flags=flags):
                result = self.run_demux("--name", NAME, "--once", *flags, check=False)
                self.assertEqual(result.returncode, 1, result.stderr)
                emitted = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
                self.assertEqual([item for item in emitted
                                  if item.get("kind") in (*DEMUX.DRAFT_KINDS, *DEMUX.TERMINAL_KINDS)], [])

    def test_once_delivers_three_words(self):
        self.speak("agent-channel-0-once-three", ["Dzień dobry wszystkim"], "Dzień dobry wszystkim")
        result = self.run_demux("--name", NAME, "--once", "--drafts", check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        emitted = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
        self.assertEqual([item["text"] for item in self.seals(emitted)], ["Dzień dobry wszystkim"])


if __name__ == "__main__":
    unittest.main()
