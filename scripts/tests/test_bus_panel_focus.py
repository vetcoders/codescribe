"""A click on an agent row brings that agent's own vc-frame pane to focus."""
import contextlib
import importlib.util
import io
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_panel_focus", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

FRAME = {"session": "vibecrafted", "pane_id": 9, "socket_dir": "/tmp/vc-frame-501",
         "host_bundle_id": "io.vetcoders.vibecrafted", "host_pid": 43854}


class FrameContextTests(unittest.TestCase):
    def test_pane_session_and_terminal_host_come_from_the_inherited_environment(self):
        environ = {"VC_FRAME_PANE_ID": "9", "VC_FRAME_SESSION_NAME": "vibecrafted",
                   "VC_FRAME_SOCKET_DIR": "/tmp/vc-frame-501",
                   "__CFBundleIdentifier": "io.vetcoders.vibecrafted",
                   "ALACRITTY_SOCKET": "/var/folders/T/Alacritty-43854.sock",
                   "VIBECRAFTED_VC_FRAME_BIN": sys.executable}
        # The engine pin is not part of the pane address.
        self.assertEqual(DEMUX.frame_context(environ), FRAME)

    def test_zellij_names_are_accepted_and_the_log_names_the_host(self):
        frame = DEMUX.frame_context({"ZELLIJ_PANE_ID": "3", "ZELLIJ_SESSION_NAME": "work",
                                     "ZELLIJ_SOCKET_DIR": "/tmp/z",
                                     "ALACRITTY_LOG": "/tmp/Alacritty-77.log"})
        self.assertEqual((frame["pane_id"], frame["session"], frame["socket_dir"],
                          frame["host_pid"]), (3, "work", "/tmp/z", 77))

    def test_an_agent_outside_vc_frame_has_no_frame(self):
        self.assertIsNone(DEMUX.frame_context({"VC_FRAME_SESSION_NAME": "vibecrafted"}))
        self.assertIsNone(DEMUX.frame_context({"VC_FRAME_PANE_ID": "9"}))
        self.assertIsNone(DEMUX.frame_context({"VC_FRAME_PANE_ID": "plugin_2",
                                               "VC_FRAME_SESSION_NAME": "vibecrafted"}))


class RunPanelFocusTests(unittest.TestCase):
    def run_with(self, returncode=0, stderr="", error=None):
        completed = subprocess.CompletedProcess([], returncode, stdout="", stderr=stderr)
        with mock.patch.object(DEMUX, "frame_cli", return_value="/opt/vc-frame"), \
                mock.patch("subprocess.run", side_effect=error, return_value=completed) as run:
            outcome = DEMUX.run_panel_focus(FRAME)
        return outcome, run

    def test_success_targets_the_terminal_pane_inside_its_own_session(self):
        outcome, run = self.run_with()
        self.assertEqual(outcome, ("focused", 0))
        command = run.call_args.args[0]
        self.assertEqual(command, ["/opt/vc-frame", "action", "focus-pane-id", "terminal_9"])
        environ = run.call_args.kwargs["env"]
        self.assertEqual((environ["ZELLIJ_SESSION_NAME"], environ["ZELLIJ_SOCKET_DIR"]),
                         ("vibecrafted", "/tmp/vc-frame-501"))

    def test_server_messages_are_answers_not_transport_failures(self):
        self.assertEqual(self.run_with(2, "Pane Terminal(9) is already focused")[0],
                         ("already_focused", 2))
        self.assertEqual(self.run_with(2, "Pane with id Terminal(9) not found")[0],
                         ("pane_missing", 2))
        self.assertEqual(self.run_with(1, "connection refused")[0], ("error", 1))

    def test_a_missing_binary_or_a_hung_server_is_an_error(self):
        self.assertEqual(self.run_with(error=FileNotFoundError("vc-frame"))[0], ("error", None))
        self.assertEqual(self.run_with(error=subprocess.TimeoutExpired("vc-frame", 5))[0],
                         ("error", None))
        with mock.patch.object(DEMUX, "frame_cli", return_value=None):
            self.assertEqual(DEMUX.run_panel_focus(FRAME), ("error", None))


class PanelFocusLeaseTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "channel-5.jsonl"
        self.bus.touch()
        self.provider, self.session = "claude-code", "panel-session"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        for target, value in (("NativeProviderModelReader", None), ("frame_context", FRAME)):
            patcher = mock.patch.object(DEMUX, target, return_value=value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def lease(self):
        lease = DEMUX.SessionLease(
            root=self.root, provider=self.provider, provider_session_id=self.session,
            name="stanisław", bus=self.bus, requested_id=None, ttl_seconds=120,
            follow_from_end=False)
        self.addCleanup(lease.close)
        return lease

    def request(self, request_id="3" * 24, age=0.0, **overrides):
        emitted = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - age))
        row = {"schema": DEMUX.AGENT_PANEL_FOCUS_SCHEMA, "kind": "focus_agent_panel",
               "request_id": request_id, "emitted_at": emitted,
               "provider": self.provider, "provider_session_id": self.session,
               "lease_id": self.lease_id, "bus": str(self.bus), "channel": "5",
               "source": "overlay_click"}
        row.update(overrides)
        return row

    def test_the_lease_publishes_its_pane_coordinates(self):
        self.lease()
        state = json.loads((self.root / "leases" / f"{self.lease_id}.json").read_text())
        self.assertEqual(state["frame"], FRAME)

    def test_a_fresh_click_focuses_once_and_answers_with_the_host(self):
        lease = self.lease()
        with mock.patch.object(DEMUX, "run_panel_focus", return_value=("focused", 0)) as run:
            result = lease.focus_panel(self.request())
            repeat = lease.focus_panel(self.request())
        run.assert_called_once_with(FRAME)
        self.assertIsNone(repeat)
        self.assertEqual({key: result[key] for key in (
            "schema", "kind", "request_id", "lease_id", "status", "exit_code")}, {
            "schema": DEMUX.AGENT_PANEL_FOCUS_RESULT_SCHEMA, "kind": "agent_panel_focus_result",
            "request_id": "3" * 24, "lease_id": self.lease_id, "status": "focused", "exit_code": 0})
        self.assertEqual(result["frame"], {"session": "vibecrafted", "pane_id": 9,
                                           "host_bundle_id": "io.vetcoders.vibecrafted",
                                           "host_pid": 43854})
        self.assertNotIn("text", result)

    def test_a_click_for_another_lease_or_bus_is_not_ours(self):
        lease = self.lease()
        with mock.patch.object(DEMUX, "run_panel_focus") as run:
            self.assertIsNone(lease.focus_panel(self.request(lease_id="f" * 32)))
            self.assertIsNone(lease.focus_panel(self.request(provider_session_id="foreign")))
            self.assertIsNone(lease.focus_panel(self.request(bus="/another/bus.jsonl")))
        run.assert_not_called()

    def test_a_replayed_backlog_does_not_steal_the_focus(self):
        lease = self.lease()
        with mock.patch.object(DEMUX, "run_panel_focus") as run:
            stale = lease.focus_panel(self.request(age=DEMUX.PANEL_FOCUS_FRESH_SECONDS + 5))
            undated = lease.focus_panel(self.request(request_id="4" * 24, emitted_at="soon"))
        run.assert_not_called()
        self.assertEqual((stale["status"], undated["status"]), ("stale", "stale"))

    def test_an_agent_outside_vc_frame_answers_no_frame(self):
        lease = self.lease()
        lease.frame = None
        with mock.patch.object(DEMUX, "run_panel_focus") as run:
            result = lease.focus_panel(self.request())
        run.assert_not_called()
        self.assertEqual((result["status"], result["frame"]), ("no_frame", None))

    def test_only_a_request_row_decodes(self):
        self.assertEqual(DEMUX.panel_focus_request(json.dumps(self.request()))["request_id"], "3" * 24)
        for row in (self.request(kind="agent_reply"), self.request(request_id="bad"),
                    {**self.request(), "schema": DEMUX.AGENT_PANEL_FOCUS_RESULT_SCHEMA}):
            self.assertIsNone(DEMUX.panel_focus_request(json.dumps(row)))
        self.assertIsNone(DEMUX.panel_focus_request("not json " + DEMUX.AGENT_PANEL_FOCUS_SCHEMA))

    def test_the_follower_answers_on_the_bus_without_waking_the_model(self):
        with self.bus.open("a") as stream:
            stream.write(json.dumps(self.request()) + "\n")

        class StopAfterOnePass:
            mode = "fixture"

            def __init__(self, *args):
                pass

            def wait(self, timeout):
                raise KeyboardInterrupt

            def close(self):
                pass

        output, error = io.StringIO(), io.StringIO()
        argv = [str(HELPER), "--bridge-home", str(self.root), "--bus", str(self.bus),
                "--provider", self.provider, "--session", self.session,
                "--name", "stanisław", "--follow", "--from-start"]
        with mock.patch.object(sys, "argv", argv), \
                mock.patch.object(DEMUX, "BusEventTrigger", StopAfterOnePass), \
                mock.patch.object(DEMUX, "run_panel_focus", return_value=("focused", 0)), \
                mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            self.assertEqual(DEMUX.main(), 130, error.getvalue())
        self.assertTrue(publish.called, error.getvalue())
        bus, result = publish.call_args.args
        self.assertEqual((bus, result["status"], result["request_id"]),
                         (self.bus, "focused", "3" * 24))
        state = json.loads((self.root / "leases" / f"{self.lease_id}.json").read_text())
        self.assertEqual(state["pending"], [])
        self.assertEqual(state["cursor"], self.bus.stat().st_size)
        self.assertNotIn(DEMUX.AGENT_PANEL_FOCUS_SCHEMA, output.getvalue())


class VcFrameOnPathTests(unittest.TestCase):
    """Leo, vibecrafted maintainer, 2026-10-19: an external client runs the public
    `vc-frame` found on PATH at each action, never the engine pin; the pane
    address keeps coming from the owner's own native context.

    The candidates are real private executables: each records which one ran,
    with what arguments and against which session and socket.
    """

    ADDRESS = "action focus-pane-id terminal_9 vibecrafted /tmp/vc-frame-501"

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "channel-5.jsonl"
        self.bus.touch()
        self.log = self.home / "dispatch.log"
        self.provider, self.session = "claude-code", "path-session"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        reader = mock.patch.object(DEMUX, "NativeProviderModelReader", return_value=None)
        reader.start()
        self.addCleanup(reader.stop)
        self.pinned = self.executable(self.home / "releases/4.4.0+gold/libexec", "pinned")
        self.first = self.executable(self.home / "first/bin", "first")
        self.second = self.executable(self.home / "second/bin", "second")
        environ = mock.patch.dict(os.environ, {
            "VC_FRAME_PANE_ID": "9", "VC_FRAME_SESSION_NAME": "vibecrafted",
            "VC_FRAME_SOCKET_DIR": "/tmp/vc-frame-501",
            "VIBECRAFTED_VC_FRAME_BIN": str(self.pinned), "PATH": str(self.first.parent)})
        environ.start()
        self.addCleanup(environ.stop)

    def executable(self, directory, label):
        directory.mkdir(parents=True)
        path = directory / "vc-frame"
        # Shell builtins only: the fake runs with PATH reduced to its own dir.
        path.write_text(
            "#!/bin/sh\n"
            f"printf '%s %s %s %s\\n' {label} \"$*\" \"$VC_FRAME_SESSION_NAME\" "
            f"\"$VC_FRAME_SOCKET_DIR\" >> {shlex.quote(str(self.log))}\n")
        path.chmod(0o755)
        return path

    def dispatched(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def lease(self):
        lease = DEMUX.SessionLease(
            root=self.root, provider=self.provider, provider_session_id=self.session,
            name="stanisław", bus=self.bus, requested_id=None, ttl_seconds=120,
            follow_from_end=False)
        self.addCleanup(lease.close)
        return lease

    def click(self, lease, request_id="5" * 24):
        return lease.focus_panel({
            "schema": DEMUX.AGENT_PANEL_FOCUS_SCHEMA, "kind": "focus_agent_panel",
            "request_id": request_id, "emitted_at": DEMUX.utc_now(),
            "provider": self.provider, "provider_session_id": self.session,
            "lease_id": self.lease_id, "bus": str(self.bus), "channel": "5",
            "source": "overlay_click"})

    def test_the_engine_pin_never_wins(self):
        result = self.click(self.lease())
        self.assertEqual((result["status"], result["exit_code"]), ("focused", 0))
        self.assertEqual(self.dispatched(), [f"first {self.ADDRESS}"])

    def test_a_path_change_after_attach_selects_the_new_command(self):
        lease = self.lease()
        os.environ["PATH"] = str(self.second.parent)
        self.click(lease)
        self.assertEqual(self.dispatched(), [f"second {self.ADDRESS}"])

    def test_the_pane_address_does_not_move_with_the_command(self):
        lease = self.lease()
        recorded = json.loads(lease.path.read_text())["frame"]
        self.click(lease, "6" * 24)
        os.environ["PATH"] = str(self.second.parent)
        self.click(lease, "7" * 24)
        self.assertEqual(self.dispatched(),
                         [f"first {self.ADDRESS}", f"second {self.ADDRESS}"])
        self.assertEqual(json.loads(lease.path.read_text())["frame"], recorded)
        self.assertEqual((recorded["session"], recorded["pane_id"], recorded["socket_dir"]),
                         ("vibecrafted", 9, "/tmp/vc-frame-501"))

    def test_without_vc_frame_on_path_there_is_no_fallback(self):
        lease = self.lease()
        os.environ["PATH"] = str(self.home / "empty")
        result = self.click(lease)
        self.assertEqual(self.dispatched(), [])
        self.assertEqual((result["status"], result["exit_code"]), ("error", None))


class ClaudeDaemonFrameTests(unittest.TestCase):
    """Daemon-hosted Claude sessions (`--bg-pty-host`) inherit no pane env in the
    worker process; the vc-frame pane lives on the attached `claude agents` client.
    """

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "channel-4.jsonl"
        self.bus.touch()
        self.claude_dir = self.home / ".claude"
        (self.claude_dir / "sessions").mkdir(parents=True)
        (self.claude_dir / "daemon" / "attach-journal").mkdir(parents=True)
        reader = mock.patch.object(DEMUX, "NativeProviderModelReader", return_value=None)
        reader.start()
        self.addCleanup(reader.stop)
        clean_env = {
            k: v for k, v in os.environ.items()
            if k not in DEMUX._FRAME_ENV_KEYS
        }
        env_patch = mock.patch.dict(os.environ, clean_env, clear=True)
        env_patch.start()
        self.addCleanup(env_patch.stop)

    def write_session(self, repl_pid: int, session_id: str, name: str):
        (self.claude_dir / "sessions" / f"{repl_pid}.json").write_text(
            json.dumps({
                "pid": repl_pid,
                "sessionId": session_id,
                "cwd": "/Users/maciejgad/vc-workspace/vetcoders/codescribe",
                "startedAt": 1792252228000,
                "kind": "bg",
                "name": name,
            })
        )

    def test_pty_coupled_claude_agents_panes_resolve_only_attached_workers(self):
        stasiu_sid = "dbf5de70-4790-4df3-bc51-f953b2e120d6"
        claudit_sid = "243a0eb0-4d6c-497b-8d36-5b58fa319b7f"
        klaudiusz_sid = "43a1778a-1e6d-4878-8326-dba5588c9114"
        explorek_sid = "9842280b-805c-4cf2-8088-963342eedae8"

        self.write_session(29034, stasiu_sid, "stasiu")
        self.write_session(38260, claudit_sid, "claudit")
        self.write_session(5560, klaudiusz_sid, "klaudiusz")
        self.write_session(96531, explorek_sid, "explorek")

        procs = {
            29022: {"pid": 29022, "ppid": 1, "tty": "ttys007", "command": "claude --bg-pty-host /tmp/a.sock"},
            29034: {"pid": 29034, "ppid": 29022, "tty": "ttys007", "command": f"claude --resume {stasiu_sid}"},
            38250: {"pid": 38250, "ppid": 1, "tty": "ttys010", "command": "claude --bg-pty-host /tmp/b.sock"},
            38260: {"pid": 38260, "ppid": 38250, "tty": "ttys010", "command": f"claude --resume {claudit_sid}"},
            5549: {"pid": 5549, "ppid": 1, "tty": "ttys003", "command": "claude --bg-pty-host /tmp/c.sock"},
            5560: {"pid": 5560, "ppid": 5549, "tty": "ttys003", "command": f"claude --resume {klaudiusz_sid}"},
            96428: {"pid": 96428, "ppid": 1, "tty": "ttys001", "command": "claude --bg-pty-host /tmp/d.sock"},
            96531: {"pid": 96531, "ppid": 96428, "tty": "ttys001", "command": f"claude --resume {explorek_sid}"},
            38226: {"pid": 38226, "ppid": 73198, "tty": "ttys008", "command": "claude agents"},
            8278: {"pid": 8278, "ppid": 73198, "tty": "ttys009", "command": "claude agents"},
        }
        envs = {
            38226: {
                "VC_FRAME_PANE_ID": "3",
                "VC_FRAME_SESSION_NAME": "codescribe",
                "ZELLIJ_SOCKET_DIR": "/tmp/vc-frame-501",
                "__CFBundleIdentifier": "io.vetcoders.vc-terminal",
                "ALACRITTY_SOCKET": "/tmp/Alacritty-73155.sock",
            },
            8278: {
                "VC_FRAME_PANE_ID": "4",
                "VC_FRAME_SESSION_NAME": "codescribe",
                "ZELLIJ_SOCKET_DIR": "/tmp/vc-frame-501",
                "__CFBundleIdentifier": "io.vetcoders.vc-terminal",
                "ALACRITTY_SOCKET": "/tmp/Alacritty-73155.sock",
            },
        }
        ttys = {
            "ttys008": {"path": "/dev/ttys008", "atime": 1792412896.878546, "winsz": (35, 112)},
            "ttys010": {"path": "/dev/ttys010", "atime": 1792412896.878549, "winsz": (35, 112)},
            "ttys009": {"path": "/dev/ttys009", "atime": 1792415016.397545, "winsz": (35, 112)},
            "ttys007": {"path": "/dev/ttys007", "atime": 1792415016.397929, "winsz": (35, 112)},
            "ttys003": {"path": "/dev/ttys003", "atime": 1792395276.0, "winsz": (32, 106)},
            "ttys001": {"path": "/dev/ttys001", "atime": 1792387238.0, "winsz": (36, 138)},
        }

        with mock.patch.object(DEMUX, "process_table", return_value=procs), \
                mock.patch.object(DEMUX, "process_environ", side_effect=lambda pid: envs.get(pid, {})), \
                mock.patch.object(DEMUX, "tty_window_and_access", side_effect=lambda tty: ttys.get(tty)):
            claudit_frame = DEMUX.resolve_claude_frame_context(claudit_sid, home=self.home)
            stasiu_frame = DEMUX.resolve_claude_frame_context(stasiu_sid, home=self.home)
            klaudiusz_frame = DEMUX.resolve_claude_frame_context(klaudiusz_sid, home=self.home)
            explorek_frame = DEMUX.resolve_claude_frame_context(explorek_sid, home=self.home)

        self.assertEqual(claudit_frame["pane_id"], 3)
        self.assertEqual(claudit_frame["session"], "codescribe")
        self.assertEqual(stasiu_frame["pane_id"], 4)
        self.assertEqual(stasiu_frame["session"], "codescribe")
        self.assertIsNone(klaudiusz_frame)
        self.assertIsNone(explorek_frame)

    def test_focus_panel_refreshes_frame_when_daemon_worker_attaches_later(self):
        claudit_sid = "243a0eb0-4d6c-497b-8d36-5b58fa319b7f"
        self.write_session(38260, claudit_sid, "claudit")
        (self.claude_dir / "daemon" / "roster.json").write_text(
            json.dumps({
                "version": 1,
                "workers": {
                    "243a0eb0": {
                        "sessionId": claudit_sid,
                        "pid": 38250,
                        "firedInteractiveMarks": [
                            {"kind": "loaded", "msgsLoaded": 874, "msgsInJsonl": 877}
                        ],
                    }
                },
            })
        )

        # At lease creation time, no `claude agents` client is attached yet.
        with mock.patch.object(DEMUX, "process_table", return_value={}):
            lease = DEMUX.SessionLease(
                root=self.root,
                provider="claude-code",
                provider_session_id=claudit_sid,
                name="claudit",
                bus=self.bus,
                requested_id=None,
                ttl_seconds=120,
                follow_from_end=False,
                provider_metadata_home=self.home,
            )
            self.addCleanup(lease.close)
        self.assertIsNone(lease.frame)

        # Later, user opens `claude agents` in pane 3 and attaches to `claudit`.
        (self.claude_dir / "daemon" / "attach-journal" / "gesture-1.json").write_text(
            json.dumps({
                "gestureId": "gesture-1",
                "startedAtEpochMs": 1792389042530,
                "pid": 38226,
                "msgsLoaded": 874,
                "msgsInJsonl": 877,
            })
        )
        procs = {
            38250: {"pid": 38250, "ppid": 1, "tty": "ttys010", "command": "claude --bg-pty-host /tmp/b.sock"},
            38260: {"pid": 38260, "ppid": 38250, "tty": "ttys010", "command": f"claude --resume {claudit_sid}"},
            38226: {"pid": 38226, "ppid": 73198, "tty": "ttys008", "command": "claude agents"},
        }
        envs = {
            38226: {
                "VC_FRAME_PANE_ID": "3",
                "VC_FRAME_SESSION_NAME": "codescribe",
                "ZELLIJ_SOCKET_DIR": "/tmp/vc-frame-501",
            }
        }
        request = {
            "schema": DEMUX.AGENT_PANEL_FOCUS_SCHEMA,
            "kind": "focus_agent_panel",
            "request_id": "8" * 24,
            "emitted_at": DEMUX.utc_now(),
            "provider": "claude-code",
            "provider_session_id": claudit_sid,
            "lease_id": lease.lease_id,
            "bus": str(self.bus),
            "channel": "4",
            "source": "overlay_click",
        }
        with mock.patch.object(DEMUX, "process_table", return_value=procs), \
                mock.patch.object(DEMUX, "process_environ", side_effect=lambda pid: envs.get(pid, {})), \
                mock.patch.object(DEMUX, "tty_window_and_access", return_value=None), \
                mock.patch.object(DEMUX, "run_panel_focus", return_value=("focused", 0)) as run_focus:
            result = lease.focus_panel(request)

        self.assertEqual(result["status"], "focused")
        self.assertEqual(result["frame"]["pane_id"], 3)
        self.assertEqual(result["frame"]["session"], "codescribe")
        run_focus.assert_called_once()
        persisted = json.loads(lease.path.read_text())
        self.assertEqual(persisted["frame"]["pane_id"], 3)


if __name__ == "__main__":
    unittest.main()
