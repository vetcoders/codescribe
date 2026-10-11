#!/usr/bin/env python3
"""One disposable, network-denied local MCP ping. No production path arguments."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile


SOURCE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("synthetic_ping_probe", SOURCE / "mcp-mailbox-probe.py")
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)
D = PROBE.DEMUX
SANDBOX = Path("/usr/bin/sandbox-exec")
PYTHON = Path(sys.executable).resolve()


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


def exchange(root, session, identity, bus, publisher, timeout):
    if sys.platform != "darwin" or not SANDBOX.is_file():
        raise RuntimeError("macOS sandbox-exec is required; no unsandboxed mode")
    profile = root / "sandbox.sb"
    profile.write_text(sandbox_profile(root, publisher))
    profile.chmod(0o600)
    argv = [str(SANDBOX), "-f", str(profile), str(PYTHON), "-B",
            str(SOURCE / "mcp-mailbox-probe.py"), "--provider", "fixture", "--session", session,
            "--bridge-home", str(root), "--bus", str(bus), "--fixture-root", str(root)]
    calls = [{"jsonrpc": "2.0", "id": 1, "method": "initialize"},
             {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}]
    for n, (name, args) in enumerate([
        ("read_pending", {}), ("ack", {"request_id": identity}), ("read_pending", {}),
        ("reply", {"request_id": identity, "text": "pong"})], start=3):
        calls.append({"jsonrpc": "2.0", "id": n, "method": "tools/call",
                      "params": {"name": name, "arguments": args}})
    # No inherited credentials, real HOME or user-supplied command/paths.
    env = {"PATH": f"{publisher.parent}:/usr/bin:/bin", "HOME": str(root),
           "TMPDIR": str(root), "PYTHONUTF8": "1"}
    child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True, env=env, cwd=root, start_new_session=True)
    try:
        output, errors = child.communicate("".join(json.dumps(row) + "\n" for row in calls), timeout=timeout)
    finally:
        # Stop the owned group, including a publisher left by interruption.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=2)
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


def run(timeout=15):
    if not 0.05 <= timeout <= 30:
        raise ValueError("timeout must be between 0.05 and 30 seconds")
    with tempfile.TemporaryDirectory(prefix="codescribe-synthetic-ping-") as temporary:
        root = Path(temporary).resolve()
        root.chmod(0o700)
        session, identity, bus = fixture(root)
        publisher = Path(D.reply_publisher_command(root)).resolve(strict=True)
        result = exchange(root, session, identity, bus, publisher, timeout)
    if root.exists():
        raise RuntimeError("fixture cleanup failed")
    return {**result, "fixture_removed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=15)
    args = parser.parse_args()
    try:
        result = run(args.timeout)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
        sys.stderr.write("Synthetic ping refused, failed or timed out; no broader retry.\n")
        return 1
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
