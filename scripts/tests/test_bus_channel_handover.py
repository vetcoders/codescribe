"""A channel digit moves between sessions of one agent name without hand edits.

A follower outlives the session that started it, so the next session of the
same name used to find its digit occupied and the Founder's takes landing in a
mailbox nobody read (live on 2026-10-06, channel 3). These tests drive the
real helper: ``--detach`` releases a session's own channel, ``--attach
--takeover`` claims a digit held by the same name, and a refusal leaves the
binding file byte-for-byte unchanged while the receipt names the reader that
was being stopped.
"""
import importlib.util
import contextlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_channel_handover", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

NAME, CHANNEL = "james", "2"
OLD = ("codex", "session-old")
NEW = ("claude-code", "session-new")


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


class ChannelHandoverTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "agent-bridge"
        self.bus = self.home / "channel-bus.jsonl"
        self.bus.touch()
        self.binding = self.root / DEMUX.AUDIENCE_BINDING_FILENAME
        self.environment = {
            key: value for key, value in os.environ.items() if key != "CLAUDE_CODE_SESSION_ID"
        }
        self.environment["CODESCRIBE_AGENT_BRIDGE_HOME"] = str(self.root)
        self.sequence = 0
        self.helper_path = HELPER
        self.processes = []
        self.addCleanup(self.reap)

    def reap(self):
        for pid in self.processes:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    # -- drivers ---------------------------------------------------------

    def helper(self, *arguments):
        return subprocess.run(
            [sys.executable, str(self.helper_path), "--bridge-home", str(self.root), *arguments],
            capture_output=True, text=True, timeout=60, env=self.environment,
        )

    def owner(self, identity):
        return ["--provider", identity[0], "--session", identity[1]]

    def attach(self, identity, name=NAME, channel=CHANNEL, takeover=False):
        result = self.helper(
            "--bus", str(self.bus), "--attach", "--channel", channel, "--name", name,
            *self.owner(identity), "--wakeup", "off", *(["--takeover"] if takeover else []),
        )
        for line in result.stdout.splitlines():
            pid = json.loads(line).get("follower_pid")
            if isinstance(pid, int):
                self.processes.append(pid)
        return result

    def attached(self, identity, **options):
        result = self.attach(identity, **options)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def detach(self, identity):
        return self.helper("--detach", *self.owner(identity))

    def lease_id(self, identity):
        return DEMUX.lease_identifier(*identity)

    def lease(self, identity):
        return json.loads(
            (self.root / "leases" / f"{self.lease_id(identity)}.json").read_text(encoding="utf-8")
        )

    def markers(self, identity):
        folder = self.root / "acknowledgments" / self.lease_id(identity)
        return sorted(path.name for path in folder.glob("*.json")) if folder.exists() else []

    def bindings(self):
        return json.loads(self.binding.read_text(encoding="utf-8"))["bindings"]

    def speak(self, text, name=NAME):
        """One sealed take addressed to ``name`` on the shared channel bus."""
        self.sequence += 1
        row = {
            "schema": DEMUX.CLEAN_SCHEMA, "sequence": self.sequence,
            "session_id": "handover-take", "utterance_id": f"utterance-{self.sequence}",
            "emitted_at": f"2026-10-06T09:00:{self.sequence:02d}Z",
            "status": "transcript_sealed", "audience": name, "text": text,
        }
        with self.bus.open("a", encoding="utf-8") as output:
            output.write(json.dumps(row, ensure_ascii=False) + "\n")

    def delivered(self, identity, text):
        """Delivery id of the sealed take once it sits in the session's mailbox."""
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                pending = self.lease(identity).get("pending", [])
            except (OSError, ValueError):
                pending = []
            for item in pending:
                if item.get("kind") == "seal" and item.get("text") == text:
                    return item["delivery_id"]
            time.sleep(0.05)
        self.fail(f"take did not reach the mailbox: {text}")

    def gone(self, pid):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and alive(pid):
            time.sleep(0.05)
        return not alive(pid)

    def orphan(self, *command):
        """A process that is nobody's child, so a stopped one is really gone."""
        started = subprocess.run(
            ["/bin/sh", "-c", 'nohup "$@" >/dev/null 2>&1 & echo $!', "sh", *command],
            capture_output=True, text=True, check=True, cwd=self.home,
        )
        pid = int(started.stdout.strip())
        self.processes.append(pid)
        return pid

    def stubborn_follower(self, identity):
        """The actual helper and lease owner, with SIGTERM refusal injected."""
        self.instrument_follower("import signal\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)")
        return self.attached(identity)["follower_pid"]

    def instrument_follower(self, body):
        script = self.home / "bus-demux.py"
        original = HELPER.read_text(encoding="utf-8")
        marker = 'if __name__ == "__main__":'
        self.assertEqual(original.count(marker), 1)
        instrument = 'if "--follow" in sys.argv:\n' + "\n".join(
            "    " + line for line in body.splitlines()) + "\n\n"
        script.write_text(original.replace(marker, instrument + marker), encoding="utf-8")
        self.helper_path = script

    def in_process_attach(self):
        arguments = [str(HELPER), "--bridge-home", str(self.root), "--bus", str(self.bus),
                     "--attach", "--channel", CHANNEL, "--name", NAME,
                     *self.owner(NEW), "--takeover", "--wakeup", "off"]
        output, error = io.StringIO(), io.StringIO()
        with mock.patch.object(sys, "argv", arguments), contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            result = DEMUX.main()
        return result, json.loads(output.getvalue()), error.getvalue()

    def stored_session(self, identity, name=NAME, channel=CHANNEL, pid=None, pending=()):
        """An earlier session's durable state, written as its follower left it."""
        lease_id = self.lease_id(identity)
        for folder in ("leases", "runtime/followers"):
            (self.root / folder).mkdir(parents=True, exist_ok=True)
        DEMUX.atomic_json(self.root / "leases" / f"{lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": lease_id, "provider": identity[0],
            "provider_session_id": identity[1], "name": name, "bus": str(self.bus),
            "cursor": 0, "last_sequence": 0, "active": pid is not None, "pid": pid,
            "heartbeat_unix": time.time(), "pending": list(pending),
        })
        if pid is not None:
            DEMUX.atomic_json(DEMUX.follower_pidfile(self.root, lease_id),
                              {"lease_id": lease_id, "pid": pid})
        state = json.loads(self.binding.read_text(encoding="utf-8")) if self.binding.exists() else {
            "schema": DEMUX.AUDIENCE_BINDING_SCHEMA, "bindings": {}}
        state["bindings"][channel] = {
            "audience": name, "provider": identity[0],
            "provider_session_id": identity[1], "bus": str(self.bus),
        }
        DEMUX.atomic_json(self.binding, state)
        return lease_id

    def envelope(self, identity, kind, delivery_id, text):
        return {
            "schema": "codescribe.agent-bridge.event.v1", "kind": kind,
            "delivery_id": delivery_id, "lease_id": self.lease_id(identity),
            "provider": identity[0], "provider_session_id": identity[1],
            "audience": NAME, "text": text, "emitted_at": "2026-10-06T08:59:00Z",
        }

    # -- detach ----------------------------------------------------------

    def test_detach_releases_own_channel_and_keeps_the_mailbox(self):
        receipt = self.attached(OLD)
        other = self.attached(("codex", "session-lena"), name="lena", channel="5")
        self.speak("James, pierwsza.")
        first = self.delivered(OLD, "James, pierwsza.")
        self.speak("James, druga.")
        second = self.delivered(OLD, "James, druga.")
        acknowledged = self.helper("--bus", str(self.bus), "--ack", first, *self.owner(OLD))
        self.assertEqual(acknowledged.returncode, 0, acknowledged.stderr)

        status = json.loads(self.helper("--status", *self.owner(OLD)).stdout)
        self.assertEqual(
            (status["pending_file"], status["acknowledged_markers"], status["backlog"],
             status["unacked_seals"], status["channel"]),
            (2, 1, 1, 1, CHANNEL), status)

        neighbour = self.bindings()["5"]
        result = self.detach(OLD)
        self.assertEqual(result.returncode, 0, result.stderr)
        released = json.loads(result.stdout)
        self.assertEqual(released["schema"], DEMUX.DETACH_RECEIPT_SCHEMA)
        self.assertEqual(released["kind"], "detach_receipt")
        self.assertEqual(released["lease_id"], self.lease_id(OLD))
        self.assertEqual(released["released_channels"], [CHANNEL])
        self.assertEqual(released["follower_pid"], receipt["follower_pid"])
        self.assertEqual(released["follower_state"], "stopped")
        self.assertIs(released["binding_changed"], True)
        self.assertIs(released["was_attached"], True)
        self.assertNotIn("attached", released)
        self.assertEqual(released["unacked_deliveries"], 1)
        self.assertEqual(released["unacked_delivery_ids"], [second])

        self.assertEqual(self.bindings(), {"5": neighbour})
        self.assertTrue(self.gone(receipt["follower_pid"]))
        self.assertTrue(alive(other["follower_pid"]))
        self.assertEqual(
            [item["delivery_id"] for item in self.lease(OLD)["pending"]], [first, second])
        self.assertEqual(self.markers(OLD), [f"{first}.json"])
        status = json.loads(self.helper("--status", *self.owner(OLD)).stdout)
        self.assertIsNone(status["channel"])
        self.assertIs(status["follower_alive"], False)
        self.assertEqual(status["backlog"], 1)

        before = self.binding.read_bytes()
        again = self.detach(OLD)
        self.assertEqual(again.returncode, 0, again.stderr)
        repeated = json.loads(again.stdout)
        self.assertEqual(repeated["released_channels"], [])
        self.assertEqual(repeated["follower_state"], "not_running")
        self.assertIsNone(repeated["follower_pid"])
        self.assertIs(repeated["binding_changed"], False)
        self.assertIs(repeated["was_attached"], False)
        self.assertEqual(self.binding.read_bytes(), before)

        # The freed digit is a plain attach for the next session of the name.
        fresh = self.attached(NEW)
        self.assertIs(fresh["binding_changed"], False)
        self.assertIsNone(fresh["previous"])
        self.assertEqual(self.bindings()[CHANNEL]["provider_session_id"], NEW[1])

    def test_detach_with_unreadable_bindings_stops_nothing(self):
        receipt = self.attached(OLD)
        self.binding.write_text("{broken", encoding="utf-8")
        result = self.detach(OLD)
        self.assertEqual(result.returncode, 3, result.stdout)
        self.assertIn("unreadable or invalid; nothing changed", result.stderr)
        self.assertEqual(self.binding.read_text(encoding="utf-8"), "{broken")
        self.assertTrue(alive(receipt["follower_pid"]), "refusal must leave the follower running")

    # -- takeover --------------------------------------------------------

    def test_takeover_moves_the_digit_to_the_next_session_of_the_name(self):
        old = self.attached(OLD)
        self.speak("James, przeczytana.")
        read = self.delivered(OLD, "James, przeczytana.")
        self.speak("James, czeka na ciebie.")
        waiting = self.delivered(OLD, "James, czeka na ciebie.")
        self.assertEqual(
            self.helper("--bus", str(self.bus), "--ack", read, *self.owner(OLD)).returncode, 0)

        before = self.binding.read_bytes()
        plain = self.attach(NEW)
        self.assertEqual(plain.returncode, 3, plain.stdout)
        self.assertEqual(plain.stdout, "")
        self.assertIn(f"channel {CHANNEL} is occupied by {NAME}; free channels: 1, 3, 4, 5, 6, 7, 8, 9; nothing changed", plain.stderr)
        self.assertIn("--takeover", plain.stderr)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertTrue(alive(old["follower_pid"]))
        self.assertFalse((self.root / "leases" / f"{self.lease_id(NEW)}.json").exists())

        mailbox, cursor = self.lease(OLD)["pending"], self.lease(OLD)["cursor"]
        receipt = self.attached(NEW, takeover=True)
        self.assertEqual(receipt["kind"], "attach_receipt")
        self.assertIs(receipt["binding_changed"], True)
        self.assertIs(receipt["follower_spawned"], True)
        self.assertEqual(receipt["previous"], {
            "provider": OLD[0], "provider_session_id": OLD[1],
            "lease_id": self.lease_id(OLD), "audience": NAME,
            "follower_pid": old["follower_pid"], "follower_state": "stopped",
            "unacked_deliveries": 1, "unacked_delivery_ids": [waiting],
        })
        entry = self.bindings()[CHANNEL]
        self.assertEqual(
            (entry["audience"], entry["provider"], entry["provider_session_id"], entry["bus"]),
            (NAME, NEW[0], NEW[1], str(self.bus)))
        self.assertTrue(self.gone(old["follower_pid"]))

        # The retired mailbox is exactly as its own session left it.
        self.assertEqual(self.lease(OLD)["pending"], mailbox)
        self.assertEqual(self.lease(OLD)["cursor"], cursor)
        self.assertEqual(self.markers(OLD), [f"{read}.json"])

        self.speak("James, nowa sesja.")
        self.delivered(NEW, "James, nowa sesja.")
        self.assertEqual(self.lease(OLD)["pending"], mailbox)

        # Inherited envelopes are read on demand and never consumed.
        retired = self.root / "leases" / f"{self.lease_id(OLD)}.json"
        untouched = retired.read_bytes()
        inherited = self.helper(
            "--read-delivery", waiting, "--lease", self.lease_id(OLD), *self.owner(NEW))
        self.assertEqual(inherited.returncode, 0, inherited.stderr)
        envelope = json.loads(inherited.stdout)
        self.assertEqual(envelope["text"], "James, czeka na ciebie.")
        self.assertEqual(envelope["delivery_id"], waiting)
        self.assertEqual(envelope["inherited_from"], {
            "provider": OLD[0], "provider_session_id": OLD[1],
            "lease_id": self.lease_id(OLD), "name": NAME, "acknowledgment": "not_available",
        })
        self.assertEqual(retired.read_bytes(), untouched)
        self.assertEqual(self.markers(OLD), [f"{read}.json"])

        for refused in (
            self.helper("--read-delivery", waiting, "--lease", self.lease_id(OLD),
                        "--provider", "codex", "--session", "stranger"),
            self.helper("--read-delivery", "0" * 24, "--lease", self.lease_id(OLD),
                        *self.owner(NEW)),
            self.helper("--bus", str(self.bus), "--ack", waiting, *self.owner(NEW)),
        ):
            self.assertEqual(refused.returncode, 3, refused.stdout)
            self.assertEqual(refused.stdout, "")
        self.assertEqual(retired.read_bytes(), untouched)
        self.assertEqual(self.markers(OLD), [f"{read}.json"])

        # Repeating the claim from the owning session changes nothing.
        owned = self.binding.read_bytes()
        repeated = self.attached(NEW, takeover=True)
        self.assertIs(repeated["binding_changed"], False)
        self.assertIsNone(repeated["previous"])
        self.assertIs(repeated["follower_spawned"], False)
        self.assertEqual(repeated["follower_pid"], receipt["follower_pid"])
        self.assertEqual(self.binding.read_bytes(), owned)

        # Once the digit is released the inherited mailbox is closed again.
        self.assertEqual(self.detach(NEW).returncode, 0)
        closed = self.helper(
            "--read-delivery", waiting, "--lease", self.lease_id(OLD), *self.owner(NEW))
        self.assertEqual(closed.returncode, 3, closed.stdout)
        self.assertIn("inherited read refused", closed.stderr)

    def test_rename_on_the_same_session_takes_effect_on_resume(self):
        """Detach + attach under a new name re-points direct routing.

        Live on 2026-10-07, channel 1: a session renamed jozek -> vagabond
        kept the old lease name on resume, so takes addressed to the new
        name never reached the mailbox while broadcasts still arrived.
        """
        self.attached(NEW, name="jozek", channel="1")
        self.assertEqual(self.detach(NEW).returncode, 0)
        self.attached(NEW, name="vagabond", channel="1")
        # The delivery proves the routing; the lease file may lag behind the
        # attach receipt until the follower's first persist.
        self.speak("Vagabond, melduj się.", name="vagabond")
        self.delivered(NEW, "Vagabond, melduj się.")
        self.assertEqual(self.lease(NEW)["name"], "vagabond")

    def test_takeover_never_claims_another_name(self):
        old = self.attached(OLD)
        before = self.binding.read_bytes()
        result = self.attach(NEW, name="eve", takeover=True)
        self.assertEqual(result.returncode, 3, result.stdout)
        self.assertEqual(result.stdout, "")
        self.assertIn(f"channel {CHANNEL} is occupied by {NAME}; free channels: 1, 3, 4, 5, 6, 7, 8, 9; nothing changed", result.stderr)
        self.assertNotIn("--takeover", result.stderr)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertTrue(alive(old["follower_pid"]))
        self.assertFalse((self.root / "leases" / f"{self.lease_id(NEW)}.json").exists())

    def test_takeover_of_a_channel_whose_reader_already_died(self):
        seal = self.envelope(OLD, "seal", "a" * 24, "James, bez czytnika.")
        typed = self.envelope(OLD, "message", "b" * 24, "James, wpisane.")
        draft = self.envelope(OLD, "revised", "c" * 24, "James, bez czytn")
        self.stored_session(OLD, pending=[draft, seal, typed])
        receipt = self.attached(NEW, takeover=True)
        self.assertIs(receipt["binding_changed"], True)
        previous = receipt["previous"]
        self.assertEqual(previous["follower_state"], "not_running")
        self.assertIsNone(previous["follower_pid"])
        # A draft revision is not a message waiting for anyone: the receipt
        # counts what --status calls unacknowledged seals.
        self.assertEqual(previous["unacked_delivery_ids"], ["a" * 24, "b" * 24])
        self.assertEqual(previous["unacked_deliveries"], 2)
        self.assertEqual(self.bindings()[CHANNEL]["provider_session_id"], NEW[1])

    def test_takeover_refuses_a_reader_it_cannot_identify(self):
        impostor = self.orphan("/bin/sleep", "120")
        self.stored_session(OLD, pid=impostor)
        before = self.binding.read_bytes()
        result = self.attach(NEW, takeover=True)
        self.assertEqual(result.returncode, 3, result.stdout)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["schema"], DEMUX.TAKEOVER_RECEIPT_SCHEMA)
        self.assertEqual(receipt["kind"], "takeover_receipt")
        self.assertIs(receipt["binding_changed"], False)
        self.assertEqual(receipt["channel"], CHANNEL)
        self.assertEqual(receipt["previous"]["follower_state"], "unverified_retained")
        self.assertEqual(receipt["previous"]["follower_pid"], impostor)
        self.assertEqual(receipt["previous"]["provider_session_id"], OLD[1])
        self.assertIn(
            f"previous follower unverified_retained; owner retained; channel {CHANNEL} unchanged", result.stderr)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertTrue(alive(impostor))
        self.assertFalse(DEMUX.follower_pidfile(self.root, self.lease_id(NEW)).exists())

        own = self.detach(OLD)
        self.assertEqual(own.returncode, 3, own.stdout)
        released = json.loads(own.stdout)
        self.assertEqual(released["follower_state"], "unverified_retained")
        self.assertEqual(released["released_channels"], [])
        self.assertIs(released["binding_changed"], False)
        self.assertIn("bus-demux: detach failed:", own.stderr)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertTrue(alive(impostor))

    def test_takeover_refuses_a_reader_that_does_not_exit(self):
        stubborn = self.stubborn_follower(OLD)
        before = self.binding.read_bytes()
        result = self.attach(NEW, takeover=True)
        self.assertEqual(result.returncode, 3, result.stdout)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["kind"], "takeover_receipt")
        self.assertIs(receipt["binding_changed"], False)
        self.assertEqual(receipt["previous"]["follower_state"], "did_not_exit")
        self.assertEqual(receipt["previous"]["follower_pid"], stubborn)
        self.assertIn(
            "bus-demux: attach failed: previous follower did_not_exit; owner retained; "
            f"channel {CHANNEL} unchanged", result.stderr)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertTrue(alive(stubborn))
        self.assertFalse(DEMUX.follower_pidfile(self.root, self.lease_id(NEW)).exists())

        own = self.detach(OLD)
        self.assertEqual(own.returncode, 3, own.stdout)
        self.assertEqual(json.loads(own.stdout)["follower_state"], "did_not_exit")
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertTrue(alive(stubborn))

    def test_takeover_that_cannot_start_its_reader_leaves_the_entry(self):
        old = self.attached(OLD)
        before = self.binding.read_bytes()
        damaged = self.root / "leases" / f"{self.lease_id(NEW)}.json"
        damaged.write_text("{broken", encoding="utf-8")
        result = self.attach(NEW, takeover=True)
        self.assertEqual(result.returncode, 3, result.stdout)
        # The old reader is already down; the entry still names its session,
        # so the same command can simply be repeated.
        self.assertEqual(self.binding.read_bytes(), before)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["kind"], "takeover_receipt")
        self.assertIs(receipt["binding_changed"], False)
        self.assertEqual(receipt["previous"]["provider_session_id"], OLD[1])
        self.assertEqual(receipt["previous"]["follower_pid"], old["follower_pid"])
        self.assertEqual(receipt["previous"]["follower_state"], "stopped")
        self.assertIn("follower exited during startup", result.stderr)
        self.assertIn(f"channel {CHANNEL} unchanged", result.stderr)
        self.assertEqual(damaged.read_text(encoding="utf-8"), "{broken")

    def test_attach_reports_a_reader_that_died_at_startup(self):
        damaged = self.root / "leases" / f"{self.lease_id(NEW)}.json"
        damaged.parent.mkdir(parents=True)
        damaged.write_text("{broken", encoding="utf-8")
        result = self.attach(NEW)
        self.assertEqual(result.returncode, 3, result.stdout)
        self.assertNotIn("attach_receipt", result.stdout)
        self.assertIn("bus-demux: attach failed: follower exited during startup", result.stderr)
        self.assertEqual(damaged.read_text(encoding="utf-8"), "{broken")

    # -- command surface -------------------------------------------------

    def test_handover_flags_refuse_ambiguous_commands(self):
        for arguments, reason in (
            (["--takeover", *self.owner(NEW)], "--takeover requires --attach"),
            (["--takeover", "--status", *self.owner(NEW)], "--takeover requires --attach"),
            (["--detach"], "--detach requires --provider/--session"),
            (["--detach", "--status", *self.owner(OLD)], "--detach combines with no other command"),
            (["--detach", "--ack", "a" * 24, *self.owner(OLD)], "--detach combines with no other command"),
            (["--detach", "--watch", *self.owner(OLD)], "--detach combines with no other command"),
            (["--detach", "--from-file", str(self.bus), *self.owner(OLD)],
             "--detach combines with no other command"),
            (["--detach", "--attach", "--channel", CHANNEL, "--name", NAME, *self.owner(OLD)],
             "--detach combines with no other command"),
        ):
            result = self.helper(*arguments)
            self.assertEqual(result.returncode, 2, (arguments, result.stderr))
            self.assertIn(reason, result.stderr, arguments)
            self.assertEqual(result.stdout, "")
        self.assertFalse(self.binding.exists())

    def test_mixed_detach_has_no_publication_or_playback_effect(self):
        old = self.attached(OLD)
        binding, bus = self.binding.read_bytes(), self.bus.read_bytes()
        for operation in (
            ["--send-text", "--channel", CHANNEL, "--lease", self.lease_id(OLD)],
            ["--say", "This must never be spoken"],
            ["--play-reply", "a" * 24, "--playback-ticket", "b" * 24],
            ["--stop-reply", "a" * 24, "--playback-ticket", "b" * 24],
        ):
            result = self.helper("--bus", str(self.bus), "--detach", *self.owner(OLD), *operation)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertEqual(result.stdout, "")
            self.assertIn("--detach combines with no other command", result.stderr)
            self.assertEqual(self.binding.read_bytes(), binding)
            self.assertEqual(self.bus.read_bytes(), bus)
            self.assertTrue(alive(old["follower_pid"]))
        self.assertFalse((self.root / "runtime" / "reply-sources").exists())

    def test_stale_manual_reader_remains_owned_without_a_pidfile(self):
        self.instrument_follower("time.time = lambda: 1.0")
        old = self.attached(OLD)
        DEMUX.follower_pidfile(self.root, self.lease_id(OLD)).unlink()
        self.assertEqual(self.lease(OLD)["heartbeat_unix"], 1.0)
        receipt = self.attached(NEW, takeover=True)
        self.assertEqual(receipt["previous"]["follower_pid"], old["follower_pid"])
        self.assertEqual(receipt["previous"]["follower_state"], "stopped")
        self.assertTrue(self.gone(old["follower_pid"]))

    def test_accepted_typed_row_ahead_of_cursor_prevents_takeover(self):
        target = Path(os.environ.get("CARGO_TARGET_DIR", HELPER.parents[1] / "target"))
        publisher = target / "debug" / "codescribe"
        self.assertTrue(publisher.is_file(), "build the canonical publisher before this round trip")
        self.environment["PATH"] = str(publisher.parent) + os.pathsep + self.environment.get("PATH", "")
        old = self.attached(OLD)
        pid = old["follower_pid"]
        os.kill(pid, signal.SIGSTOP)
        binding = self.binding.read_bytes()
        sent = subprocess.run(
            [sys.executable, str(self.helper_path), "--bridge-home", str(self.root),
             "--bus", str(self.bus), "--send-text", "--channel", CHANNEL,
             "--lease", self.lease_id(OLD), *self.owner(OLD)],
            input="James, accepted before handover", capture_output=True, text=True,
            timeout=10, env=self.environment)
        self.assertEqual(sent.returncode, 0, sent.stderr)
        published = json.loads(sent.stdout)
        self.assertEqual(published["kind"], "message_published")
        self.assertLess(self.lease(OLD)["cursor"], self.bus.stat().st_size)
        os.kill(pid, signal.SIGKILL)
        self.assertTrue(self.gone(pid))
        lease, source = self.lease(OLD), self.bus.read_bytes()
        refused = self.attach(NEW, takeover=True)
        self.assertEqual(refused.returncode, 3, refused.stderr)
        self.assertEqual(json.loads(refused.stdout)["previous"]["follower_state"], "undrained_retained")
        self.assertEqual(self.binding.read_bytes(), binding)
        self.assertEqual(self.lease(OLD), lease)
        self.assertEqual(self.bus.read_bytes(), source)
        self.assertFalse((self.root / "leases" / f"{self.lease_id(NEW)}.json").exists())
        recovered = self.attached(OLD)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            pending = self.lease(OLD)["pending"]
            if any(item.get("message_id") == published["message_id"] for item in pending):
                break
            time.sleep(0.05)
        else:
            self.fail("accepted old-owner message was not recoverable")
        self.assertTrue(alive(recovered["follower_pid"]))
        self.assertEqual(self.bindings()[CHANNEL]["provider_session_id"], OLD[1])

    def test_bound_owner_without_recovery_state_is_retained(self):
        self.stored_session(OLD)
        (self.root / "leases" / f"{self.lease_id(OLD)}.json").unlink()
        binding = self.binding.read_bytes()
        result = self.attach(NEW, takeover=True)
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertEqual(self.binding.read_bytes(), binding)
        self.assertEqual(json.loads(result.stdout)["previous"]["follower_state"], "undrained_retained")
        self.assertFalse((self.root / "leases" / f"{self.lease_id(NEW)}.json").exists())

    def test_log_open_failure_retains_exact_old_binding(self):
        self.attached(OLD)
        binding = self.binding.read_bytes()
        log, _ = DEMUX.follower_paths(self.root, self.lease_id(NEW))
        log.mkdir()
        result = self.attach(NEW, takeover=True)
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertEqual(self.binding.read_bytes(), binding)
        self.assertEqual(json.loads(result.stdout)["new_follower_state"], "not_started")

    def test_pidfile_write_failure_stops_its_new_reader_and_keeps_owner(self):
        self.attached(OLD)
        binding = self.binding.read_bytes()
        DEMUX.follower_pidfile(self.root, self.lease_id(NEW)).mkdir()
        result = self.attach(NEW, takeover=True)
        self.assertEqual(result.returncode, 3, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["new_follower_state"], "stopped")
        self.assertIs(receipt["binding_changed"], False)
        self.assertEqual(self.binding.read_bytes(), binding)

    def test_spawn_failure_keeps_exact_old_binding(self):
        self.attached(OLD)
        binding = self.binding.read_bytes()
        original = subprocess.Popen
        def fail_follower(command, *arguments, **options):
            if "--follow" in command:
                raise OSError("injected follower spawn failure")
            return original(command, *arguments, **options)
        with mock.patch.object(subprocess, "Popen", side_effect=fail_follower):
            result, receipt, error = self.in_process_attach()
        self.assertEqual(result, 3, error)
        self.assertIn("injected follower spawn failure", error)
        self.assertIs(receipt["binding_changed"], False)
        self.assertEqual(receipt["new_follower_state"], "not_started")
        self.assertEqual(self.binding.read_bytes(), binding)

    def test_post_replace_failure_restores_original_raw_bytes(self):
        self.attached(OLD)
        binding = self.binding.read_bytes()
        original = DEMUX.atomic_json
        def fail_after_replace(path, value):
            original(path, value)
            if path == self.binding:
                raise OSError("injected binding durability failure")
        with mock.patch.object(DEMUX, "atomic_json", side_effect=fail_after_replace):
            result, receipt, error = self.in_process_attach()
        self.assertEqual(result, 3, error)
        self.assertIs(receipt["binding_changed"], False)
        self.assertEqual(receipt["binding_state"], "unchanged")
        self.assertEqual(self.binding.read_bytes(), binding)

    def test_failed_restoration_reports_uncertainty(self):
        self.attached(OLD)
        binding = self.binding.read_bytes()
        original = DEMUX.atomic_json
        original_bytes = DEMUX.atomic_bytes
        def fail_after_replace(path, value):
            original(path, value)
            if path == self.binding:
                raise OSError("injected binding durability failure")
        def fail_restore(path, encoded):
            if path == self.binding and encoded == binding:
                raise OSError("injected restore failure")
            return original_bytes(path, encoded)
        with mock.patch.object(DEMUX, "atomic_json", side_effect=fail_after_replace), mock.patch.object(DEMUX, "atomic_bytes", side_effect=fail_restore):
            result, receipt, error = self.in_process_attach()
        self.assertEqual(result, 3, error)
        self.assertIsNone(receipt["binding_changed"])
        self.assertEqual(receipt["binding_state"], "uncertain")
        self.assertIn("channel 2 state uncertain", error)
        self.assertEqual(self.bindings()[CHANNEL]["provider_session_id"], NEW[1])


if __name__ == "__main__":
    unittest.main()
