"""Independent controls for remote identity and wake shutdown races."""

import contextlib
import importlib.util
import io
from pathlib import Path
import sys
import tempfile
import threading
from types import SimpleNamespace
import unittest
import unittest.mock as mock


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parents[1] / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


demux = load("remote_admission_demux", "bus-demux.py")
mcp = load("remote_admission_mcp", "bus-mcp.py")


class RemoteBusAdmissionTests(unittest.TestCase):
    def test_remote_registration_uses_the_session_the_remote_parser_will_select(self):
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch("subprocess.run", return_value=SimpleNamespace(returncode=0)):
                with mock.patch.object(demux, "remember_remote_host") as remember:
                    code = demux.run_remote(
                        ["--remote", "host-a", "--attach", "--channel", "2", "--name", "agent",
                         "--provider", "claude-code", "--session", "first", "--session", "second"],
                        environ={}, root=Path(directory),
                    )
        self.assertEqual(code, 0)
        self.assertEqual(remember.call_args.args[1], demux.lease_identifier("claude-code", "second"))

    def test_a_spawn_finishing_after_stop_releases_its_remote_waiter(self):
        stopped = threading.Event()
        reads = []

        class ErrorPipe:
            def read(self):
                reads.append(True)
                return ""

        proc = SimpleNamespace(stderr=ErrorPipe(), wait=lambda: 0)

        def spawn(*args, **kwargs):
            stopped.set()
            return proc

        with mock.patch.object(mcp.subprocess, "Popen", side_effect=spawn):
            with mock.patch.object(mcp, "release_remote") as release:
                result = mcp.remote_wake(
                    demux, "host-a", "session", {}, [], window=1,
                    alive=lambda: not stopped.is_set(),
                )
        self.assertEqual(result, (0, ""))
        release.assert_called_once_with(proc)
        self.assertEqual(reads, [], "a canceled late spawn must not enter a blocking stderr read")

    def test_remote_notice_completed_during_local_wake_cleanup_is_not_lost(self):
        announced = threading.Event()
        released = threading.Event()
        finished = threading.Event()
        proc = object()

        def local(*args, **kwargs):
            self.assertTrue(announced.wait(2))
            return mcp.WAKE_EXIT

        def remote(demux, host, session, environ, procs, **kwargs):
            procs.append(proc)
            announced.set()
            self.assertTrue(released.wait(2))
            finished.set()
            return mcp.WAKE_EXIT, "Codescribe: new message on channel 2\n"

        err = io.StringIO()
        with mock.patch.object(mcp, "bound_channel", return_value=("2", "lena")):
            with mock.patch.object(mcp, "native_wake", side_effect=local):
                with mock.patch.object(mcp, "remote_wake", side_effect=remote):
                    with mock.patch.object(mcp, "release_remote", side_effect=lambda _: released.set()):
                        with contextlib.redirect_stderr(err):
                            code = mcp.wake_here_and_remote(
                                demux, Path("/unused"), Path("/unused/bus"), "session", {},
                                ["host-a"], window=1, interval=0.01,
                            )
                            self.assertTrue(finished.wait(2))
        self.assertEqual(code, mcp.WAKE_EXIT)
        self.assertIn("Mailbox on host-a", err.getvalue())


if __name__ == "__main__":
    unittest.main()
