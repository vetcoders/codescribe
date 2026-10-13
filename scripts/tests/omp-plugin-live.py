#!/usr/bin/env python3
"""Live acceptance of the Codescribe plugin in a real OMP (oh-my-pi) client.

Operator-class instrument: it runs `omp --mode rpc` (account, network, model
cost) against an isolated bridge home and bus, never the user's channels. It
proves the path the hermetic suite cannot: marketplace plugin load, extension
identity, MCP connect, a turn that ENDS, then the extension's `cs-bus --omp-wake`
waiter waking the conversation for a fresh message, read and ACK without a typed
nudge. Exit 0 only when that whole chain is observed.

It registers a temporary marketplace and plugin in the user's OMP plugin
registry under the name `codescribe-live` and removes them again; an enabled
user-scope install of `codescribe@codescribe` would load a second copy of the
plugin, so disable it first (`omp plugin disable codescribe@codescribe`).

  python3 scripts/tests/omp-plugin-live.py [--model ID] [--keep DIR]
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

REPO = Path(__file__).resolve().parents[2]
HELPER = REPO / "scripts" / "cs-bus"
CHANNEL, NAME, PROVIDER = "4", "probe", "omp"
MARKETPLACE = "codescribe-live"


def stage_plugin(target: Path, bus: Path) -> Path:
    """Copy the marketplace; point the plugin's launchers at this checkout's helper."""
    shutil.copytree(REPO / "plugins", target, ignore=shutil.ignore_patterns(".DS_Store", "__pycache__"))
    # A distinct marketplace and runtime package name: OMP refuses a second
    # package called "codescribe" next to an installed codescribe@codescribe.
    catalog = target / ".claude-plugin" / "marketplace.json"
    manifest = json.loads(catalog.read_text())
    manifest["name"] = MARKETPLACE
    manifest["plugins"][0]["name"] = MARKETPLACE
    catalog.write_text(json.dumps(manifest, indent=2) + "\n")
    plugin = target / "codescribe"
    descriptor = plugin / ".claude-plugin" / "plugin.json"
    package = json.loads(descriptor.read_text())
    package["name"] = MARKETPLACE
    descriptor.write_text(json.dumps(package, indent=2) + "\n")
    mcp = plugin / ".mcp.json"
    mcp.write_text(mcp.read_text().replace('\\"$HOME/.local/bin/cs-bus\\"', f'\\"{HELPER}\\"')
                   .replace(" --mcp", f" --mcp --bus {bus}"))
    extension = plugin / "hooks" / "pre" / "codescribe-omp.ts"
    extension.write_text(extension.read_text().replace('"$HOME/.local/bin/cs-bus"', f'"{HELPER}"')
                         .replace(" --omp-wake", f" --omp-wake --bus {bus}"))
    return target


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


def omp(*arguments, check=True):
    return subprocess.run(["omp", *arguments], stdin=subprocess.DEVNULL, capture_output=True, text=True,
                          timeout=120, check=check)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--model", default=os.environ.get("VIBECRAFTED_MODEL_REQUESTED", "kimi-k3:cloud"))
    parser.add_argument("--keep", type=Path, help="evidence directory to keep (default: temporary)")
    parser.add_argument("--wake-timeout", type=float, default=180.0)
    parser.add_argument("--silence", type=float, default=30.0, help="seconds of silence before each message")
    parser.add_argument("--messages", type=int, default=2, help="fresh messages, each needing its own wake")
    options = parser.parse_args()

    work = Path(tempfile.mkdtemp(prefix="cs-omp-live-"))
    evidence = options.keep or work
    evidence.mkdir(parents=True, exist_ok=True)
    home, bus = work / "bridge", work / "bus.jsonl"
    bus.touch()
    marketplace = stage_plugin(work / "plugins", bus)
    env = {key: value for key, value in os.environ.items() if not key.startswith(("CLAUDE_CODE_", "VIBECRAFTED_"))}
    env["CODESCRIBE_AGENT_BRIDGE_HOME"] = str(home)
    receipt = {"model": options.model, "bridge_home": str(home), "bus": str(bus),
               "omp_version": omp("--version").stdout.strip()}
    omp("plugin", "marketplace", "add", str(marketplace))
    try:
        omp("plugin", "install", f"{MARKETPLACE}@{MARKETPLACE}")
        command = ["omp", "--mode", "rpc", "--no-ui", "--auto-approve", "--no-title", "--model", options.model,
                   "--session-dir", str(work / "sessions"), "--cwd", str(work)]
        transcript = (evidence / "omp-rpc.jsonl").open("w")
        client = subprocess.Popen(command, cwd=work, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=(evidence / "omp-stderr.log").open("w"), text=True, bufsize=1)
        events: queue.Queue = queue.Queue()

        def pump():
            for line in client.stdout:
                transcript.write(line)
                transcript.flush()
                try:
                    events.put(json.loads(line))
                except json.JSONDecodeError as error:
                    transcript.write(f"[plugin-live] non-json stdout line skipped: {error}\n")
                    transcript.flush()
            events.put(None)

        threading.Thread(target=pump, daemon=True).start()

        def say(identity, text):
            client.stdin.write(json.dumps({"id": identity, "type": "prompt", "message": text}) + "\n")
            client.stdin.flush()

        def until_turn_end(deadline):
            tools, text = [], []
            while time.monotonic() < deadline:
                try:
                    event = events.get(timeout=max(0.1, deadline - time.monotonic()))
                except queue.Empty:
                    break
                if event is None:
                    raise SystemExit("omp exited early; see omp-stderr.log")
                if event.get("type") == "tool_execution_start":
                    # OMP exposes MCP tools as `xd://mcp__<plugin>_<server>_<tool>` files: `read`
                    # fetches the schema, `write` invokes. Record the invoked Codescribe tool.
                    name = str(event.get("toolName") or event.get("name") or "")
                    path = str((event.get("args") or {}).get("path") or "")
                    if name == "write" and path.startswith("xd://") and "codescribe_" in path:
                        name = "codescribe_" + path.rsplit("codescribe_", 1)[1]
                    tools.append(name)
                if event.get("type") == "message_end":
                    message = event.get("message") or {}
                    if message.get("role") == "assistant":
                        for block in message.get("content") or []:
                            if isinstance(block, dict) and block.get("type") == "text":
                                text.append(str(block.get("text") or ""))
                if event.get("type") == "agent_end":
                    return tools, "\n".join(text), event
            return tools, "\n".join(text), None

        try:
            say("req_connect",
                "Integration probe for the Codescribe plugin. Call codescribe_connect with the provider and "
                f"session_id from the Codescribe identity context, name {NAME!r}, channel {CHANNEL!r}. "
                "Do not call any other tool and do not wait: end your turn right after the connect result. "
                "If you are later woken by a Codescribe notice, call codescribe_read, then codescribe_ack with "
                "exactly its read_delivery_ids, then answer with the message text verbatim and end the turn.")
            tools, _text, first = until_turn_end(time.monotonic() + 180)
            receipt["connect_turn_tools"] = tools
            if first is None or not any(name.endswith("codescribe_connect") for name in tools):
                raise SystemExit("first turn did not connect")
            bindings = json.loads((home / "vc.agent-audience-binding.v1.json").read_text())["bindings"]
            binding = bindings.get(CHANNEL) or {}
            session = binding.get("provider_session_id")
            if binding.get("provider") != PROVIDER or not session:
                raise SystemExit(f"channel {CHANNEL} is not bound to an omp session: {binding}")
            receipt["session_id"] = session
            lease_id = json.loads(subprocess.run(
                [sys.executable, str(HELPER), "--status", "--bridge-home", str(home),
                 "--provider", PROVIDER, "--session", session], capture_output=True, text=True, check=True).stdout)["lease_id"]
            receipt["lease_id"] = lease_id
            lock = home / "runtime" / "native-wake" / f"{lease_id}.lock"
            receipt["wakes"] = []
            for index in range(options.messages):
                # Silence after an ENDED turn: any agent_end here would be a polling turn.
                _tools, _text, idle = until_turn_end(time.monotonic() + options.silence)
                if idle is not None:
                    raise SystemExit("a model turn ran during silence")
                wake = {"silence_seconds": options.silence, "armed_before_message": armed(lock)}
                text = f"Iwo {index + 1} — świeża wiadomość bez nudge'a."
                sent_at = time.monotonic()
                published = subprocess.run(
                    [sys.executable, str(HELPER), "--send-text", "--channel", CHANNEL, "--bridge-home", str(home),
                     "--provider", PROVIDER, "--session", session, "--lease", lease_id, "--bus", str(bus)],
                    input=text, capture_output=True, text=True)
                if published.returncode:
                    raise SystemExit("send-text refused: " + published.stderr)
                tools, answer, result = until_turn_end(time.monotonic() + options.wake_timeout)
                markers = sorted(path.stem for path in (home / "acknowledgments" / lease_id).glob("*.json"))
                wake.update({"text": text, "turn_tools": tools, "latency_seconds": round(time.monotonic() - sent_at, 1),
                             "answer": answer, "acknowledged_total": len(markers),
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
            if receipt.get("session_id"):
                subprocess.run([sys.executable, str(HELPER), "--detach", "--bridge-home", str(home),
                                "--provider", PROVIDER, "--session", receipt["session_id"]], capture_output=True)
    finally:
        omp("plugin", "uninstall", f"{MARKETPLACE}@{MARKETPLACE}", check=False)
        omp("plugin", "marketplace", "remove", MARKETPLACE, check=False)
        (evidence / "receipt.json").write_text(json.dumps(receipt, indent=2, ensure_ascii=False) + "\n")
        print(json.dumps(receipt, indent=2, ensure_ascii=False))
    return 0 if receipt.get("listening_verified") else 1


if __name__ == "__main__":
    sys.exit(main())
