#!/usr/bin/env python3
"""Disposable sandboxed MCP fixture: one local ping or bounded stdio service."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import pwd
import selectors
import signal
import subprocess
import sys
import tempfile
import time


SOURCE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("synthetic_ping_probe", SOURCE / "mcp-mailbox-probe.py")
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)
D = PROBE.DEMUX
SANDBOX = Path("/usr/bin/sandbox-exec")
PYTHON = Path(sys.executable).resolve()
MAX_TRAFFIC = 2 << 20
MAX_FRAMES = 16


def pinned_publisher():
    """Use the installed user's fixed binary, never PATH or inherited HOME."""
    candidate = Path(pwd.getpwuid(os.getuid()).pw_dir) / ".cargo" / "bin" / "codescribe"
    if candidate.is_symlink() or not candidate.is_file() or not os.access(candidate, os.X_OK):
        raise RuntimeError("fixed canonical publisher is unavailable")
    return candidate.resolve(strict=True)


def sandbox_profile(root, publisher):
    """Default deny, bounded writable fixture and two permitted executables."""
    root = root.resolve(strict=True)
    reads = [root, SOURCE, Path("/System"), Path("/Library"), Path("/usr"), Path("/opt/homebrew")]
    clauses = " ".join(f"(subpath {json.dumps(str(path))})" for path in reads)
    return "\n".join([
        "(version 1)", "(deny default)", "(allow file-read-metadata)",
        f"(allow file-read* {clauses} (literal {json.dumps(str(publisher))}) "
        '(literal "/dev/null") (literal "/dev/random") (literal "/dev/urandom"))',
        f"(allow file-write* (subpath {json.dumps(str(root))}) (literal \"/dev/null\"))",
        "(allow process-fork)",
        f"(allow process-exec (literal {json.dumps(str(PYTHON))}) (literal {json.dumps(str(publisher))}))",
        "(allow signal (target self))", "(allow sysctl-read)",
    ])


def fixture(root):
    """Only synthetic labels, under a launcher-owned TemporaryDirectory."""
    session = "synthetic-" + os.urandom(12).hex()
    lease = D.lease_identifier("fixture", session)
    identity = os.urandom(12).hex()
    bus = root / "bus.jsonl"
    bus.touch(mode=0o600)
    owner = {"provider": "fixture", "provider_session_id": session, "lease_id": lease}
    envelope = dict(owner, delivery_owner=owner.copy(), delivery_id=identity,
                    kind="message", source="typed", text="Miksa, ping", audience="synthetic",
                    bus=str(bus), emitted_at=D.utc_now(), message_id=identity)
    D.atomic_json(root / "fixture.json", {"schema": "codescribe.synthetic-probe.v1",
                  "session": session, "lease_id": lease})
    D.atomic_json(root / "leases" / f"{lease}.json", {
        "schema": D.LEASE_SCHEMA, **owner, "bus": str(bus), "name": "synthetic",
        "pending": [envelope], "cursor": 0})
    return session, identity, bus


def start_child(root, session, bus, publisher, *, text):
    if sys.platform != "darwin" or not SANDBOX.is_file():
        raise RuntimeError("macOS sandbox-exec is required; no unsandboxed mode")
    profile = root / "sandbox.sb"
    profile.write_text(sandbox_profile(root, publisher))
    profile.chmod(0o600)
    argv = [str(SANDBOX), "-f", str(profile), str(PYTHON), "-B",
            str(SOURCE / "mcp-mailbox-probe.py"), "--provider", "fixture", "--session", session,
            "--bridge-home", str(root), "--bus", str(bus), "--fixture-root", str(root)]
    # PATH contains only the pinned publisher's directory, not caller entries.
    env = {"PATH": str(publisher.parent), "HOME": str(root),
           "TMPDIR": str(root), "PYTHONUTF8": "1"}
    return subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=text, env=env, cwd=root,
                            bufsize=-1 if text else 0, start_new_session=True)


def stop_child(child):
    """Stop only our group, including the canonical publisher, and reap it."""
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=2)
    finally:
        for stream in (child.stdin, child.stdout, child.stderr):
            stream.close()


def exchange(root, session, identity, bus, publisher, timeout):
    calls = [{"jsonrpc": "2.0", "id": 1, "method": "initialize"},
             {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}]
    for n, (name, args) in enumerate([
        ("read_pending", {}), ("ack", {"request_id": identity}), ("read_pending", {}),
        ("reply", {"request_id": identity, "text": "pong"})], start=3):
        calls.append({"jsonrpc": "2.0", "id": n, "method": "tools/call",
                      "params": {"name": name, "arguments": args}})
    child = start_child(root, session, bus, publisher, text=True)
    try:
        output, errors = child.communicate("".join(json.dumps(row) + "\n" for row in calls), timeout=timeout)
    finally:
        stop_child(child)
    if child.returncode or errors:
        raise RuntimeError("sandboxed probe failed; no unsandboxed retry")
    responses = [json.loads(line) for line in output.splitlines()]
    if len(responses) != 6 or any(row.get("error") or row.get("result", {}).get("isError") for row in responses):
        raise RuntimeError("MCP text exchange refused")
    read = json.loads(responses[2]["result"]["content"][0]["text"])
    extra = json.loads(responses[4]["result"]["content"][0]["text"])
    if (read["events"][0]["request_id"] != identity
            or read["events"][0]["delivery"]["text"] != "Miksa, ping"
            or len(read["events"]) != 1 or read["remaining"] != 0 or extra["events"]):
        raise RuntimeError("unexpected synthetic mailbox contents")
    receipt = json.loads(responses[-1]["result"]["content"][0]["text"])
    mailbox = PROBE.Mailbox("fixture", session, root, bus, root)
    _, reply = D.load_published_reply(mailbox.args, receipt["reply_id"])
    if reply.get("delivery_id") != identity or reply.get("text") != "pong" or reply.get("spoken") is not False:
        raise RuntimeError("canonical reply readback failed")
    if (root / "wakeups").exists():
        raise RuntimeError("native queue state is forbidden")
    return {"result": "passed", "synthetic_input": "Miksa, ping", "reply": "pong",
            "network": "denied_by_sandbox", "native_queue": "disabled", "spoken": False}


class FrameBudget:
    """Bound NDJSON before forwarding, including incomplete lines and totals."""
    def __init__(self):
        self.pending = bytearray()
        self.total = 0
        self.frames = 0

    def feed(self, data):
        self.total += len(data)
        if self.total > MAX_TRAFFIC:
            raise ValueError("stdio traffic limit exceeded")
        self.pending.extend(data)
        ready = bytearray()
        while (end := self.pending.find(b"\n")) >= 0:
            if end + 1 > PROBE.LIMIT:
                raise ValueError("stdio frame limit exceeded")
            self.frames += 1
            if self.frames > MAX_FRAMES:
                raise ValueError("stdio frame count exceeded")
            ready.extend(self.pending[:end + 1])
            del self.pending[:end + 1]
        if len(self.pending) >= PROBE.LIMIT:
            raise ValueError("unterminated stdio frame limit exceeded")
        return ready

    def finish(self):
        if self.pending:
            raise ValueError("EOF inside stdio frame")


def relay(child, timeout, incoming, outgoing):
    """One nonblocking stdio relay; slow input/output cannot defeat deadline."""
    input_fd, output_fd = incoming.fileno(), outgoing.fileno()
    child_in, child_out, child_err = child.stdin.fileno(), child.stdout.fileno(), child.stderr.fileno()
    reads = {input_fd: (child_in, FrameBudget()), child_out: (output_fd, FrameBudget()), child_err: None}
    writes = {child_in: bytearray(), output_fd: bytearray()}
    original = {fd: os.get_blocking(fd) for fd in (*reads, *writes)}
    deadline = time.monotonic() + timeout
    try:
        for fd in original:
            os.set_blocking(fd, False)
        with selectors.DefaultSelector() as selector:
            for fd in reads:
                selector.register(fd, selectors.EVENT_READ)
            while True:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise subprocess.TimeoutExpired("synthetic stdio service", timeout)
                if input_fd not in reads and not writes[child_in] and not child.stdin.closed:
                    child.stdin.close()
                if child_out not in reads and child_err not in reads and not writes[output_fd]:
                    if child.wait(timeout=left) != 0:
                        raise RuntimeError("sandboxed service failed")
                    return
                for key, _ in selector.select(left):
                    fd = key.fd
                    # Another event in this batch may have unregistered this fd.
                    if fd not in reads and (fd not in writes or not writes[fd]):
                        continue
                    try:
                        if fd in reads:
                            data = os.read(fd, 65536)
                            route = reads[fd]
                            if not data:
                                selector.unregister(fd)
                                del reads[fd]
                                if route:
                                    route[1].finish()
                                if fd == child_out and input_fd in reads:
                                    reads[input_fd][1].finish()
                                    selector.unregister(input_fd)
                                    del reads[input_fd]
                                    if writes[child_in]:
                                        raise RuntimeError("service closed before input drained")
                            elif route is None:
                                # Never relay raw child diagnostics to the caller.
                                raise RuntimeError("sandboxed service emitted diagnostics")
                            else:
                                destination, budget = route
                                framed = budget.feed(data)
                                if framed:
                                    if not writes[destination]:
                                        selector.register(destination, selectors.EVENT_WRITE)
                                    writes[destination].extend(framed)
                        else:
                            count = os.write(fd, writes[fd][:65536])
                            if not count:
                                raise RuntimeError("stdio write stalled")
                            del writes[fd][:count]
                            if not writes[fd]:
                                selector.unregister(fd)
                    except BlockingIOError:
                        continue
    finally:
        # Restore caller-owned stdio flags; child pipes may already be closed.
        for fd, blocking in original.items():
            if fd in (input_fd, output_fd):
                os.set_blocking(fd, blocking)


def serve(root, session, bus, publisher, timeout):
    child = start_child(root, session, bus, publisher, text=False)
    try:
        relay(child, timeout, sys.stdin.buffer, sys.stdout.buffer)
    finally:
        stop_child(child)


def run(timeout=15, service=False):
    if not 0.05 <= timeout <= 30:
        raise ValueError("timeout must be between 0.05 and 30 seconds")
    with tempfile.TemporaryDirectory(prefix="codescribe-synthetic-ping-") as temporary:
        root = Path(temporary).resolve()
        root.chmod(0o700)
        session, identity, bus = fixture(root)
        publisher = pinned_publisher()
        if service:
            serve(root, session, bus, publisher, timeout)
            result = None
        else:
            result = exchange(root, session, identity, bus, publisher, timeout)
    if root.exists():
        raise RuntimeError("fixture cleanup failed")
    return {**result, "fixture_removed": True} if result is not None else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=15)
    parser.add_argument("--serve", action="store_true", help="bounded fixture-only MCP stdio server")
    args = parser.parse_args()
    try:
        result = run(args.timeout, service=args.serve)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
        sys.stderr.write("Synthetic ping refused, failed or timed out; no broader retry.\n")
        return 1
    if result is not None:
        print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
