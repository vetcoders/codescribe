"""An unexpectedly lost follower comes back on its own lease, at most twice.

Founder 2026-10-10: a broken Bus connection must tell the agent and resume
while the session lasts; after two such breaks with no message between them,
stop reconnecting. These tests drive the watch's supervisor and the owned
recovery path with fake processes, a fake clock and a fake Codex queue: no
follower is spawned or killed and no provider is called.
"""
import argparse
import contextlib
import fcntl
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "bus_listener_recovery_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

SESSION = "thread-roman-a"
FORK = "thread-roman-fork"
CHANNEL = "2"


class FakeChild:
    def __init__(self, pid):
        self.pid = pid
        self.terminated = False

    def poll(self):
        return 1 if self.terminated else None

    def terminate(self):
        self.terminated = True


class ListenerRecoveryTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve() / "agent-bridge"
        self.bus = str(Path(temp.name).resolve() / "channel-2.jsonl")
        Path(self.bus).touch()
        self.lease_id = DEMUX.lease_identifier("codex", SESSION)
        self.lease_path = self.root / "leases" / f"{self.lease_id}.json"
        self.pending = [self.envelope(f"{index:024x}") for index in (1, 2)]
        self.write_lease(pid=4101, started="Sat Oct 10 00:10:00 2026")
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings[CHANNEL] = {"audience": "roman", "provider": "codex",
                                 "provider_session_id": SESSION, "bus": self.bus}
        DEMUX.atomic_json(DEMUX.follower_pidfile(self.root, self.lease_id), {
            "lease_id": self.lease_id, "pid": 4101,
            "configuration": {"wakeup": "codex-queue", "on_seal": None, "helper_sha256": "old"},
        })
        self.alive: set[int] = set()
        self.launches = []
        self.next_pid = 5000
        patches = [
            mock.patch.object(DEMUX, "process_is_alive", side_effect=lambda pid: pid in self.alive),
            mock.patch.object(DEMUX, "launch_follower", side_effect=self.fake_launch),
            mock.patch.object(DEMUX, "confirm_follower", side_effect=self.fake_confirm),
        ]
        for patcher in patches:
            patcher.start()
            self.addCleanup(patcher.stop)

    def envelope(self, identity):
        return {"kind": "seal", "delivery_id": identity, "lease_id": self.lease_id,
                "provider": "codex", "provider_session_id": SESSION,
                "audience": "roman", "text": f"Roman, wiadomość {identity[-1]}"}

    def write_lease(self, *, pid, started, pending=None):
        DEMUX.atomic_json(self.lease_path, {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id, "provider": "codex",
            "provider_session_id": SESSION, "name": "roman", "bus": self.bus,
            "cursor": 777, "pending": self.pending if pending is None else pending,
            "active": True, "pid": pid, "process_identity": {"started": started, "command": "x"},
            "heartbeat_unix": 0,
        })

    def fake_launch(self, root, command):
        self.launches.append(command)
        self.next_pid += 1
        return FakeChild(self.next_pid)

    def fake_confirm(self, root, lease_id, session, bus, child, configuration):
        # The real child resumes the lease: same cursor and pending, new pid.
        state = DEMUX.read_json(self.lease_path)
        state.update(pid=child.pid, process_identity={"started": f"t{child.pid}", "command": "x"})
        DEMUX.atomic_json(self.lease_path, state)
        DEMUX.atomic_json(DEMUX.follower_pidfile(root, lease_id),
                          {"lease_id": lease_id, "pid": child.pid, "configuration": configuration})
        self.alive.add(child.pid)

    def kill_current_follower(self):
        self.alive.discard(DEMUX.read_json(self.lease_path)["pid"])

    def supervisor(self, session=SESSION):
        return DEMUX.ListenerSupervisor(self.root, "codex", session, interval=2.0)

    def drive(self, supervisor, start, seconds, step=1.0):
        notices, now = [], start
        while now < start + seconds:
            notice = supervisor.check(now)
            if notice is not None:
                notices.append(notice)
            now += step
        return notices, now

    def lifecycle(self):
        return DEMUX.read_json(DEMUX.lifecycle_paths(self.root, self.lease_id)[0]) or {}

    def test_unexpected_loss_recovers_same_lease_with_exact_backlog(self):
        self.alive.add(4101)
        watch = self.supervisor()
        self.assertEqual(self.drive(watch, 0, 10)[0], [])
        lease_before = DEMUX.read_json(self.lease_path)
        self.kill_current_follower()
        notices, _ = self.drive(watch, 10, 10)
        self.assertEqual([n["event"] for n in notices], ["follower_recovered"])
        notice = notices[0]
        self.assertEqual(notice["consecutive_losses"], 1)
        self.assertEqual((notice["channel"], notice["name"], notice["lost_follower_pid"]),
                         (CHANNEL, "roman", 4101))
        self.assertNotIn("delivery_id", notice)
        self.assertNotIn("text", notice)
        self.assertEqual(len(self.launches), 1)
        argv = self.launches[0]
        for flag, value in (("--session", SESSION), ("--provider", "codex"), ("--name", "roman"),
                            ("--follower-channel", CHANNEL), ("--bus", self.bus),
                            ("--wakeup", "codex-queue")):
            self.assertEqual(argv[argv.index(flag) + 1], value)
        self.assertIn("--follow", argv)
        lease_after = DEMUX.read_json(self.lease_path)
        self.assertEqual(lease_after["pending"], lease_before["pending"])
        self.assertEqual(lease_after["cursor"], 777)
        self.assertFalse((self.root / "acknowledgments").exists())
        self.assertFalse((self.root / "wakeups").exists())
        # Single reader: further checks see the recovered follower and stay quiet.
        self.assertEqual(self.drive(watch, 20, 30)[0], [])
        self.assertEqual(len(self.launches), 1)

    def test_two_losses_without_message_suspend_visibly_and_stop_reviving(self):
        self.alive.add(4101)
        watch = self.supervisor()
        self.kill_current_follower()
        first, now = self.drive(watch, 0, 6)
        self.assertEqual([n["event"] for n in first], ["follower_recovered"])
        self.kill_current_follower()
        with mock.patch("shutil.which", return_value="/fake/codex"), \
                mock.patch("subprocess.run",
                           return_value=subprocess.CompletedProcess([], 0, "queued", "")) as run:
            second, now = self.drive(watch, now, 6)
            later, _ = self.drive(watch, now, 60)
        self.assertEqual([n["event"] for n in second], ["recovery_suspended"])
        self.assertEqual(second[0]["consecutive_losses"], 2)
        self.assertEqual(second[0]["queue_notice"], "provider_accepted")
        self.assertEqual(later, [])
        self.assertEqual(len(self.launches), 1)
        self.assertEqual(run.call_count, 1)
        argv = run.call_args.args[0]
        self.assertEqual(argv[:5], ["/fake/codex", "queue", "--thread", SESSION, "--message"])
        self.assertNotIn("Roman, wiadomość", argv[5])
        for envelope in self.pending:
            self.assertNotIn(envelope["delivery_id"], argv[5])
        self.assertTrue(self.lifecycle()["suspended"])
        status = self.status()
        self.assertTrue(status["listener"]["recovery_suspended"])
        self.assertEqual(status["listener"]["consecutive_losses"], 2)
        self.assertEqual(status["backlog"], 2)

    def test_received_message_resets_the_streak(self):
        self.alive.add(4101)
        watch = self.supervisor()
        self.kill_current_follower()
        _, now = self.drive(watch, 0, 6)
        self.assertEqual(self.lifecycle()["consecutive_losses"], 1)
        # The recovered follower's lease receives one new message.
        self.write_lease(pid=0, started="gone")
        lease = DEMUX.SessionLease(
            root=self.root, provider="codex", provider_session_id=SESSION,
            name="roman", bus=Path(self.bus), requested_id=None, ttl_seconds=120,
            follow_from_end=True)
        self.assertEqual(sorted(lease.pending), [e["delivery_id"] for e in self.pending])
        self.assertTrue(lease.queue_delivery(self.envelope("f" * 24)))
        lease.close()
        self.assertEqual(self.lifecycle()["consecutive_losses"], 0)
        self.assertEqual(self.lifecycle()["reset_by"], "message_received")
        self.alive.clear()  # the follower that received it is lost too
        notices, _ = self.drive(watch, now, 6)
        self.assertEqual([n["event"] for n in notices], ["follower_recovered"])
        self.assertEqual(notices[0]["consecutive_losses"], 1)
        self.assertEqual(len(self.launches), 2)

    def test_quiet_idle_never_trips_a_loss(self):
        self.alive.add(4101)
        watch = self.supervisor()
        notices, _ = self.drive(watch, 0, 3600, step=7.0)
        self.assertEqual(notices, [])
        self.assertEqual(self.launches, [])
        self.assertFalse(DEMUX.lifecycle_paths(self.root, self.lease_id)[0].exists())

    def test_one_missing_observation_is_not_a_loss(self):
        self.alive.add(4101)
        watch = self.supervisor()
        watch.check(0)
        self.kill_current_follower()
        self.assertIsNone(watch.check(2))  # first absence only
        self.alive.add(4101)  # a reattach restarted it meanwhile
        self.assertEqual(self.drive(watch, 4, 20)[0], [])
        self.assertEqual(self.launches, [])

    def test_explicit_detach_prevents_revival(self):
        self.alive.add(4101)
        watch = self.supervisor()
        watch.check(0)
        self.kill_current_follower()
        with DEMUX.channel_bindings(self.root) as bindings:
            del bindings[CHANNEL]  # what --detach commits after stopping it
        notices, _ = self.drive(watch, 2, 10)
        self.assertEqual([n["event"] for n in notices], ["listener_ended"])
        self.assertEqual(self.launches, [])
        self.assertFalse(DEMUX.lifecycle_paths(self.root, self.lease_id)[0].exists())

    def test_fork_with_another_session_cannot_take_the_connection(self):
        lease_bytes = self.lease_path.read_bytes()
        fork = self.supervisor(FORK)
        self.assertEqual(self.drive(fork, 0, 20)[0], [])
        self.assertEqual(DEMUX.recover_follower(self.root, "codex", FORK)["event"], "listener_ended")
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings[CHANNEL] = {"audience": "roman", "provider": "codex",
                                 "provider_session_id": FORK, "bus": self.bus}
        # Takeover by the fork: the original session must not revive either.
        self.assertEqual(DEMUX.recover_follower(self.root, "codex", SESSION)["event"], "listener_ended")
        self.assertEqual(self.launches, [])
        self.assertEqual(self.lease_path.read_bytes(), lease_bytes)

    def test_held_lease_lock_is_a_handover_not_a_loss(self):
        lock_path = self.root / "leases" / f"{self.lease_id}.lock"
        with open(lock_path, "a+b") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            self.assertEqual(DEMUX.recover_follower(self.root, "codex", SESSION)["event"], "busy")
        self.assertEqual(self.launches, [])
        self.assertFalse(DEMUX.lifecycle_paths(self.root, self.lease_id)[0].exists())

    def test_notification_window_renewal_is_not_a_disconnect(self):
        self.alive.add(4101)
        watch = self.supervisor()
        record = DEMUX.watch_record_path(self.root, self.lease_id)
        identity = {"started": "w1", "command": "watch"}
        # The provider's window ends and the agent starts the next watch;
        # the follower keeps running throughout.
        for watch_pid in (7001, 7002, 7003):
            DEMUX.atomic_json(record, {"lease_id": self.lease_id, "pid": watch_pid,
                                       "process_identity": identity})
            self.alive.add(watch_pid)
            self.assertEqual(self.drive(watch, watch_pid, 55)[0], [])
            self.alive.discard(watch_pid)
        self.assertEqual(self.launches, [])
        self.assertFalse(DEMUX.lifecycle_paths(self.root, self.lease_id)[0].exists())
        status = self.status()
        self.assertEqual(status["listener"]["consecutive_losses"], 0)
        self.assertFalse(status["listener"]["watch_alive"])

    def test_failed_recovery_is_reported_once_and_not_retried_for_the_same_loss(self):
        self.alive.add(4101)
        watch = self.supervisor()
        self.kill_current_follower()
        with mock.patch.object(DEMUX, "confirm_follower",
                               side_effect=OSError("follower exited during startup")):
            notices, _ = self.drive(watch, 0, 60)
        self.assertEqual([n["event"] for n in notices], ["recovery_failed"])
        self.assertEqual(len(self.launches), 1)
        self.assertEqual(self.lifecycle()["losses"][-1]["outcome"], "recovery_failed")

    def status(self):
        args = argparse.Namespace(bridge_home=self.root, provider="codex", session=SESSION,
                                  lease_ttl=120.0)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            DEMUX.status_command(args)
        return json.loads(output.getvalue())


if __name__ == "__main__":
    unittest.main()
