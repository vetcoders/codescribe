#!/usr/bin/env python3
"""Live acceptance of the Codescribe plugin in a real Claude Code client.

Operator-class instrument: it runs `claude -p` (account, network, model cost)
against an isolated bridge home and bus, never the user's channels. It proves
the path the hermetic suite cannot: plugin load, hook identity, MCP connect,
a turn that ENDS, then an asyncRewake wake for a fresh message, read and ACK
without a typed nudge. Exit 0 only when that whole chain is observed.

  python3 scripts/tests/claude-code-plugin-live.py [--model ID] [--keep DIR]
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import uuid

REPO = Path(__file__).resolve().parents[2]
HELPER = REPO / "scripts" / "cs-bus"
CHANNEL, NAME = "7", "probe"


def stage_plugin(target: Path, bus: Path) -> Path:
    """Copy the plugin; point its launchers at this checkout's helper."""
    plugin = target / "codescribe"
    shutil.copytree(REPO / "plugins" / "codescribe", plugin,
                    ignore=shutil.ignore_patterns(".DS_Store", "__pycache__"))
    for relative in (".mcp.json", "hooks/claude-code.json"):
        path = plugin / relative
        text = path.read_text().replace('\\"$HOME/.local/bin/cs-bus\\"', f'\\"{HELPER}\\"')
        if relative == ".mcp.json":
            text = text.replace(" --mcp", f" --mcp --bus {bus}")
        path.write_text(text)
    return plugin


def armed(lock: Path) -> bool:
    """A wake process holds the lease lock; probing never arms it."""
    if not lock.exists():
        return False
    with lock.open() as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(handle, fcntl.LOCK_UN)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--model", default=os.environ.get("VIBECRAFTED_MODEL_REQUESTED", "claude-opus-5-5"))
    parser.add_argument("--keep", type=Path, help="evidence directory to keep (default: temporary)")
    parser.add_argument("--wake-timeout", type=float, default=180.0)
    parser.add_argument("--silence", type=float, default=30.0, help="seconds of silence before each message")
    parser.add_argument("--messages", type=int, default=2, help="fresh messages, each needing its own wake")
    options = parser.parse_args()

    work = Path(tempfile.mkdtemp(prefix="cs-claude-live-"))
    evidence = options.keep or work
    evidence.mkdir(parents=True, exist_ok=True)
    home, bus = work / "bridge", work / "bus.jsonl"
    bus.touch()
    plugin = stage_plugin(work / "plugins", bus)
    session = str(uuid.uuid4())
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("CLAUDE_CODE_", "CLAUDECODE", "VIBECRAFTED_"))}
    env["CODESCRIBE_AGENT_BRIDGE_HOME"] = str(home)
    command = ["claude", "-p", "--input-format", "stream-json", "--output-format", "stream-json",
               "--verbose", "--session-id", session, "--plugin-dir", str(plugin),
               "--model", options.model, "--permission-mode", "bypassPermissions",
               "--setting-sources", "project,local"]
    transcript = (evidence / "claude-stream.jsonl").open("w")
    client = subprocess.Popen(command, cwd=work, env=env, stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, stderr=(evidence / "claude-stderr.log").open("w"),
                              text=True, bufsize=1)
    events: queue.Queue = queue.Queue()

    def pump():
        for line in client.stdout:
            transcript.write(line)
            transcript.flush()
            try:
                events.put(json.loads(line))
            except json.JSONDecodeError:
                pass
        events.put(None)

    threading.Thread(target=pump, daemon=True).start()

    def say(text):
        client.stdin.write(json.dumps({"type": "user", "message": {
            "role": "user", "content": [{"type": "text", "text": text}]}}) + "\n")
        client.stdin.flush()

    def until_result(deadline):
        tools = []
        while time.monotonic() < deadline:
            try:
                event = events.get(timeout=max(0.1, deadline - time.monotonic()))
            except queue.Empty:
                break
            if event is None:
                raise SystemExit("claude exited early; see claude-stderr.log")
            for block in (event.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use":
                    tools.append(block.get("name", ""))
            if event.get("type") == "result":
                return tools, event
        return tools, None

    receipt = {"session_id": session, "model": options.model, "bridge_home": str(home), "bus": str(bus),
               "claude_version": subprocess.run(["claude", "--version"], capture_output=True, text=True).stdout.strip()}
    try:
        say("Integration probe for the Codescribe plugin. Call codescribe_connect with the provider and "
            f"session_id from the Codescribe identity hook context, name {NAME!r}, channel {CHANNEL!r}. "
            "Do not call any other tool and do not wait: end your turn right after the connect result. "
            "If you are later woken by a Codescribe notice, call codescribe_read, then codescribe_ack with "
            "exactly its read_delivery_ids, then answer with the message text verbatim and end the turn.")
        tools, first = until_result(time.monotonic() + 180)
        receipt["connect_turn_tools"] = tools
        if first is None or not any(name.endswith("codescribe_connect") for name in tools):
            raise SystemExit("first turn did not connect")
        lease_id = json.loads(subprocess.run(
            [sys.executable, str(HELPER), "--status", "--bridge-home", str(home),
             "--provider", "claude-code", "--session", session], capture_output=True, text=True, check=True).stdout)["lease_id"]
        receipt["lease_id"] = lease_id
        lock = home / "runtime" / "native-wake" / f"{lease_id}.lock"
        receipt["wakes"] = []
        for index in range(options.messages):
            # Silence after an ENDED turn: any result here would be a polling turn.
            _tools, idle = until_result(time.monotonic() + options.silence)
            if idle is not None:
                raise SystemExit("a model turn ran during silence")
            wake = {"silence_seconds": options.silence, "armed_before_message": armed(lock)}
            text = f"Iwo {index + 1} — świeża wiadomość bez nudge'a."
            sent_at = time.monotonic()
            published = subprocess.run(
                [sys.executable, str(HELPER), "--send-text", "--channel", CHANNEL, "--bridge-home", str(home),
                 "--provider", "claude-code", "--session", session, "--lease", lease_id, "--bus", str(bus)],
                input=text, capture_output=True, text=True)
            if published.returncode:
                raise SystemExit("send-text refused: " + published.stderr)
            tools, result = until_result(time.monotonic() + options.wake_timeout)
            markers = sorted(path.stem for path in (home / "acknowledgments" / lease_id).glob("*.json"))
            wake.update({"text": text, "turn_tools": tools, "latency_seconds": round(time.monotonic() - sent_at, 1),
                         "result": (result or {}).get("result"), "acknowledged_total": len(markers),
                         "verified": bool(result and len(markers) == index + 1
                                          and any(name.endswith("codescribe_read") for name in tools)
                                          and any(name.endswith("codescribe_ack") for name in tools))})
            receipt["wakes"].append(wake)
            if not wake["verified"]:
                break
        receipt["acknowledged_delivery_ids"] = sorted(
            path.stem for path in (home / "acknowledgments" / lease_id).glob("*.json"))
        receipt["listening_verified"] = (len(receipt["wakes"]) == options.messages
                                         and all(wake["verified"] and wake["armed_before_message"]
                                                 for wake in receipt["wakes"]))
    finally:
        client.stdin.close()
        try:
            client.wait(timeout=30)
        except subprocess.TimeoutExpired:
            client.kill()
        subprocess.run([sys.executable, str(HELPER), "--detach", "--bridge-home", str(home),
                        "--provider", "claude-code", "--session", session], capture_output=True)
        (evidence / "receipt.json").write_text(json.dumps(receipt, indent=2, ensure_ascii=False) + "\n")
        print(json.dumps(receipt, indent=2, ensure_ascii=False))
    return 0 if receipt.get("listening_verified") else 1


if __name__ == "__main__":
    sys.exit(main())
