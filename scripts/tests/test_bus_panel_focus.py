"""A click on an agent row brings that agent's own vc-frame pane to focus."""
import contextlib
import importlib.util
import io
import json
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
         "host_bundle_id": "io.vetcoders.vibecrafted", "host_pid": 43854,
         "frame_bin": "/opt/vc-frame"}


class FrameContextTests(unittest.TestCase):
    def test_pane_session_and_terminal_host_come_from_the_inherited_environment(self):
        environ = {"VC_FRAME_PANE_ID": "9", "VC_FRAME_SESSION_NAME": "vibecrafted",
                   "VC_FRAME_SOCKET_DIR": "/tmp/vc-frame-501",
                   "__CFBundleIdentifier": "io.vetcoders.vibecrafted",
                   "ALACRITTY_SOCKET": "/var/folders/T/Alacritty-43854.sock",
                   "VIBECRAFTED_VC_FRAME_BIN": sys.executable}
        self.assertEqual(DEMUX.frame_context(environ), {
            "session": "vibecrafted", "pane_id": 9, "socket_dir": "/tmp/vc-frame-501",
            "host_bundle_id": "io.vetcoders.vibecrafted", "host_pid": 43854,
            "frame_bin": sys.executable})

    def test_zellij_names_are_accepted_and_the_log_names_the_host(self):
        frame = DEMUX.frame_context({"ZELLIJ_PANE_ID": "3", "ZELLIJ_SESSION_NAME": "work",
                                     "ZELLIJ_SOCKET_DIR": "/tmp/z",
                                     "ALACRITTY_LOG": "/tmp/Alacritty-77.log", "PATH": ""})
        self.assertEqual((frame["pane_id"], frame["session"], frame["socket_dir"],
                          frame["host_pid"], frame["frame_bin"]), (3, "work", "/tmp/z", 77, None))

    def test_an_agent_outside_vc_frame_has_no_frame(self):
        self.assertIsNone(DEMUX.frame_context({"VC_FRAME_SESSION_NAME": "vibecrafted"}))
        self.assertIsNone(DEMUX.frame_context({"VC_FRAME_PANE_ID": "9"}))
        self.assertIsNone(DEMUX.frame_context({"VC_FRAME_PANE_ID": "plugin_2",
                                               "VC_FRAME_SESSION_NAME": "vibecrafted"}))


class RunPanelFocusTests(unittest.TestCase):
    def run_with(self, returncode=0, stderr="", error=None):
        completed = subprocess.CompletedProcess([], returncode, stdout="", stderr=stderr)
        with mock.patch("subprocess.run", side_effect=error, return_value=completed) as run:
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
        self.assertEqual(DEMUX.run_panel_focus({**FRAME, "frame_bin": None}), ("error", None))


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


if __name__ == "__main__":
    unittest.main()
