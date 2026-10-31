#!/usr/bin/env python3
"""Behavioral mailbox regressions against separately compiled real sidecars.

The integrator supplies the normal executable and a second build of the same
Swift source with -D NATIVE_AX_PROBE_CLEANUP_TEST. No source is copied or parsed.
All requests are refused before AX application lookup (pid=-1, press=False).
"""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
import uuid


def wait_for(predicate, process, description, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(
                f"sidecar exited {process.returncode} while waiting for {description}"
            )
        if predicate():
            return
        time.sleep(0.005)
    raise AssertionError(f"timed out waiting for {description}")


@contextmanager
def mailbox(seed=None):
    with tempfile.TemporaryDirectory(prefix="codescribe-ax-cleanup-") as root:
        directory = Path(root)
        if seed is not None:
            seed(directory)
        with tempfile.TemporaryFile() as diagnostics:
            process = subprocess.Popen(
                [str(PROBE), str(directory)],
                stdout=diagnostics,
                stderr=diagnostics,
            )
            try:
                wait_for(lambda: (directory / "ready").is_file(), process, "ready")
                yield directory, process
            except Exception as error:
                diagnostics.seek(0)
                details = diagnostics.read().decode(errors="replace")[-2000:]
                raise AssertionError(f"{error}\nsidecar diagnostics:\n{details}") from error
            finally:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def refused_request(directory, process):
    request_id = str(uuid.uuid4())
    path = directory / f"{request_id}.request.json"
    # Atomic admission avoids accidentally testing an incomplete client write.
    staged = directory / "staged"
    staged.write_text(json.dumps({
        "requestID": request_id,
        "pid": -1,
        "identifier": "not-an-admitted-control",
        "press": False,
    }))
    staged.replace(path)
    assert_refusal(directory, process, request_id)
    wait_for(lambda: not path.exists(), process, "valid request cleanup")


def assert_refusal(directory, process, request_id):
    response = directory / f"{request_id}.response.json"
    wait_for(response.is_file, process, "refused request response")
    value = json.loads(response.read_text())
    if value.get("requestID") != request_id or value.get("errorCode") != "request-refused":
        raise AssertionError(f"unexpected response: {value}")
    if value.get("elements") != [] or value.get("pressStatus") is not None:
        raise AssertionError(f"refused request gained an AX result: {value}")


class NativeAXCleanupTests(unittest.TestCase):
    def cleanup(self, path):
        return subprocess.run(
            [str(CLEANUP_PROBE), str(path)], capture_output=True, timeout=5
        )

    def test_cleanup_accepts_absent_valid_and_invalid_request_names(self):
        with tempfile.TemporaryDirectory() as root:
            for name in [f"{uuid.uuid4()}.request.json", "invalid.request.json"]:
                with self.subTest(name=name):
                    result = self.cleanup(Path(root) / name)
                    self.assertEqual(result.returncode, 0, result.stderr[-2000:])

    def test_cleanup_removes_an_existing_request(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "invalid.request.json"
            path.write_bytes(b"invalid")
            result = self.cleanup(path)
            self.assertEqual(result.returncode, 0, result.stderr[-2000:])
            self.assertFalse(path.exists())

    def test_cleanup_preserves_permission_errors(self):
        self.assertNotEqual(os.geteuid(), 0, "EACCES contract requires a non-root test user")
        with tempfile.TemporaryDirectory() as root:
            parent = Path(root) / "read-only"
            parent.mkdir()
            path = parent / "invalid.request.json"
            path.write_bytes(b"must survive failed removal")
            parent.chmod(0o500)
            try:
                result = self.cleanup(path)
                self.assertNotEqual(result.returncode, 0, "EACCES was silently swallowed")
                self.assertEqual(path.read_bytes(), b"must survive failed removal")
            finally:
                parent.chmod(0o700)

    def test_cleanup_preserves_not_a_directory_errors(self):
        with tempfile.TemporaryDirectory() as root:
            parent = Path(root) / "regular-file"
            parent.write_bytes(b"not a directory")
            result = self.cleanup(parent / "invalid.request.json")
            self.assertNotEqual(result.returncode, 0, "ENOTDIR was treated as ENOENT")
            self.assertEqual(parent.read_bytes(), b"not a directory")

    def test_invalid_filename_cleanup_keeps_mailbox_alive(self):
        with mailbox() as (directory, process):
            path = directory / "invalid.request.json"
            path.write_bytes(b"invalid")
            wait_for(lambda: not path.exists(), process, "invalid request cleanup")
            self.assertFalse((directory / "invalid.response.json").exists())
            refused_request(directory, process)

    def test_valid_filename_cleanup_keeps_mailbox_alive(self):
        with mailbox() as (directory, process):
            refused_request(directory, process)
            refused_request(directory, process)

    def test_requester_unlinks_enumerated_requests_before_sidecar_cleanup(self):
        request_ids = [str(uuid.uuid4()) for _ in range(512)]

        def seed(directory):
            # Everything exists before launch. The first response witnesses
            # that contentsOfDirectory returned its array of all these URLs.
            for request_id in request_ids:
                (directory / f"{request_id}.request.json").write_text(json.dumps({
                    "requestID": request_id,
                    "pid": -1,
                    "identifier": "not-an-admitted-control",
                    "press": False,
                }))

        with mailbox(seed) as (directory, process):
            wait_for(
                lambda: any(directory.glob("*.response.json")),
                process,
                "first response from the pre-enumerated batch",
            )
            removed = []
            for request_id in request_ids:
                if (directory / f"{request_id}.response.json").exists():
                    continue
                try:
                    (directory / f"{request_id}.request.json").unlink()
                    removed.append(request_id)
                except FileNotFoundError:
                    # The sidecar already completed this request; it is not
                    # evidence of a requester-side removal and is not counted.
                    continue
            self.assertTrue(
                removed,
                "INSTRUMENT: sidecar exhausted the batch before requester removal; "
                "the cleanup race was not exercised",
            )
            print(f"enumerated_batch={len(request_ids)} requester_removed={len(removed)}")
            wait_for(
                lambda: len(list(directory.glob("*.response.json"))) == len(request_ids),
                process,
                "responses for the enumerated batch including removed requests",
            )
            assert_refusal(directory, process, removed[0])
            # A response precedes cleanup. A later request proves the sidecar
            # survived cleanup and returned to its outer mailbox loop.
            refused_request(directory, process)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", required=True, type=Path)
    parser.add_argument("--cleanup-probe", required=True, type=Path)
    options, test_arguments = parser.parse_known_args()
    PROBE = options.probe.resolve(strict=True)
    CLEANUP_PROBE = options.cleanup_probe.resolve(strict=True)
    unittest.main(argv=[__file__, *test_arguments])
