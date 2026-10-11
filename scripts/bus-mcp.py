#!/usr/bin/env python3
"""Native MCP transport over the existing Codescribe mailbox, not an agent runner."""
from concurrent.futures import ThreadPoolExecutor
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from types import SimpleNamespace


def install_extension(demux, root, directory):
    source = root / "runtime" / "plugins"
    if not (source / ".claude-plugin/marketplace.json").is_file():
        raise ValueError("native extension payload missing; run make install-bus first")
    target = directory / "extensions.json"
    if target.is_symlink():
        raise ValueError("symlinked extensions.json refused; existing configuration was retained")
    if not target.is_file():
        raise ValueError(f"register this native marketplace in Junie first: /extensions marketplace add {source}")
    descriptor = os.open(directory / "codescribe-install.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        config = json.loads(target.read_text())
        if not isinstance(config, dict):
            raise ValueError("extensions.json must be an object; existing configuration was retained")
        matches = [entry for entry in config.values() if isinstance(entry, dict)
                   and entry.get("source") == {"type": "local", "path": str(source)}]
        if len(matches) != 1:
            raise ValueError(f"native marketplace absent or source conflicts; register it with /extensions marketplace add {source}")
        entry = matches[0]
        installed = entry.get("extensions")
        if not isinstance(installed, list) or not all(isinstance(item, str) for item in installed):
            raise ValueError("invalid extension list; existing configuration was retained")
        if "codescribe" not in installed:
            installed.append("codescribe")
        demux.atomic_json(target, config)
    return {"kind": "extension_registered", "source": str(source), "configuration": str(target),
            "activation": "next Junie start, or /extensions install codescribe in the running session"}


def schema(properties=None, required=None):
    return {"type": "object", "properties": properties or {}, "required": required or [],
            "additionalProperties": False}


TOOLS = [
    {"name": "codescribe_connect", "description": "Bind THIS Junie session to a named Codescribe channel. Use the real session_id supplied by the native SessionStart hook. Never take over another conversation.",
     "inputSchema": schema({"session_id": {"type": "string"}, "name": {"type": "string"},
                            "channel": {"type": "string", "pattern": "^[1-9]$"},
                            "voice": {"type": "string"}, "tts_vendor": {"type": "string", "enum": ["xai", "openai"]}},
                           ["session_id", "name", "channel"])},
    {"name": "codescribe_wait", "description": "Wait for one complete owned message, without invoking a model or ACKing. Remains pending during silence; cancel the tool to stop waiting. After read, ACK before acting. After replying, call this again for continuous listening.", "inputSchema": schema()},
    {"name": "codescribe_read", "description": "Read one complete pending message without ACKing. Drain before waiting; preserve sender and attachment pointers.", "inputSchema": schema()},
    {"name": "codescribe_ack", "description": "Acknowledge ONLY exact delivery IDs completely read through this MCP connection, immediately before work. ACK means read, not completed.",
     "inputSchema": schema({"delivery_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}}}, ["delivery_ids"])},
    {"name": "codescribe_reply", "description": "Save and speak one requested voice reply associated with an exact read and acknowledged delivery. Retains the existing voice, credentials and microphone/playback interlocks. Report spoken/reason truthfully.",
     "inputSchema": schema({"delivery_id": {"type": "string"}, "text": {"type": "string"}}, ["delivery_id", "text"])},
    {"name": "codescribe_status", "description": "Inspect this connection without claiming listening_verified. Only a fresh delivery and answer prove listening.", "inputSchema": schema()},
    {"name": "codescribe_detach", "description": "Release this conversation's channel on explicit stop. Unread messages can prevent release; never delete them.", "inputSchema": schema()},
]


class Mailbox:
    def __init__(self, demux, root, bus=None):
        self.demux, self.root, self.bus = demux, root, bus
        self.owner = None
        self.read_ids, self.acked_ids, self.replied_ids = set(), set(), set()
        self.lock = threading.RLock()
        self.wait_lock = threading.Lock()
        self.command = [sys.executable, str(Path(demux.__file__).with_name("cs-bus"))]

    def run(self, flags, timeout=30):
        command = [*self.command, "--bridge-home", str(self.root)]
        if self.bus is not None:
            command += ["--bus", str(self.bus)]
        if self.owner is not None:
            command += ["--provider", "junie", "--session", self.owner]
        result = subprocess.run([*command, *flags], capture_output=True, text=True, timeout=timeout)
        if result.returncode:
            # Voice failure still carries its durable reply receipt.
            if "--say" in flags and result.stdout.strip():
                return {"receipts": [json.loads(line) for line in result.stdout.splitlines()], "exit_code": result.returncode}
            raise ValueError(result.stderr.strip() or "Codescribe operation refused")
        rows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        return rows[0] if len(rows) == 1 else {"receipts": rows}

    def connect(self, values):
        session = values["session_id"]
        if not isinstance(session, str) or not session.strip() or any(ord(c) < 32 for c in session):
            raise ValueError("real native session_id required")
        if not isinstance(values["name"], str) or not values["name"].strip():
            raise ValueError("nonempty agent name required")
        if values["channel"] not in tuple(str(n) for n in range(1, 10)):
            raise ValueError("channel must be a single digit 1-9")
        if self.owner is not None and self.owner != session:
            raise ValueError("this MCP connection already belongs to another session")
        flags = ["--attach", "--name", values["name"], "--channel", values["channel"]]
        for key, flag in (("voice", "--voice"), ("tts_vendor", "--tts-vendor")):
            if key in values:
                flags += [flag, values[key]]
        previous, self.owner = self.owner, session
        try:
            return self.run(flags)
        except Exception:
            self.owner = previous
            raise

    def read(self):
        result = self.run(["--read-pending", "--read-limit", "1"])
        self.read_ids.update(result["read_delivery_ids"])
        return result

    def wait(self, cancel):
        if not self.wait_lock.acquire(blocking=False):
            raise ValueError("one Codescribe wait is already active")
        try:
            args = SimpleNamespace(bridge_home=self.root, provider="junie", session=self.owner,
                                   bus=self.bus, bus_overridden=self.bus is not None)
            source = self.root / "leases" / f"{self.demux.lease_identifier('junie', self.owner)}.json"
            trigger = self.demux.BusEventTrigger(source, 0.1)
            try:
                while not cancel.is_set():
                    _state, rows = self.demux.unread_pending(args)
                    if rows:
                        with self.lock:
                            result = self.read()
                        if result["deliveries"]:
                            return result
                    trigger.wait(timeout=0.5)
                raise ValueError("Codescribe wait cancelled; nothing was acknowledged")
            finally:
                trigger.close()
        finally:
            self.wait_lock.release()

    def call(self, name, values, cancel):
        definition = next((tool for tool in TOOLS if tool["name"] == name), None)
        if definition is None:
            raise ValueError("unknown Codescribe tool")
        contract = definition["inputSchema"]
        if not isinstance(values, dict) or set(values) - set(contract["properties"]) or any(key not in values for key in contract["required"]):
            raise ValueError("invalid tool arguments")
        for key, value in values.items():
            kind = contract["properties"][key]["type"]
            if kind == "string" and not isinstance(value, str) or kind == "array" and not isinstance(value, list):
                raise ValueError("invalid tool argument type")
        with self.lock:
            if name == "codescribe_connect":
                return self.connect(values)
            if self.owner is None:
                raise ValueError("connect using THIS session's native session_id first")
            if name == "codescribe_read":
                return self.read()
            if name == "codescribe_ack":
                ids = values["delivery_ids"]
                if not isinstance(ids, list) or not ids or not all(isinstance(item, str) and item in self.read_ids for item in ids):
                    raise ValueError("ACK requires exact IDs completely read through this connection")
                result = self.run(["--ack", *ids])
                self.acked_ids.update(ids)
                return result
            if name == "codescribe_reply":
                identity, text = values["delivery_id"], values["text"]
                if not isinstance(identity, str) or identity not in self.acked_ids:
                    raise ValueError("reply requires a read and acknowledged owned delivery")
                if not isinstance(text, str) or not text.strip():
                    raise ValueError("reply text must not be empty")
                if identity in self.replied_ids:
                    raise ValueError("reply already attempted; inspect its durable receipt instead of replaying")
                self.replied_ids.add(identity)
                return self.run(["--say", text, "--reply-to", identity], timeout=300)
            if name == "codescribe_status":
                return self.run(["--status"])
            if name == "codescribe_detach":
                if self.wait_lock.locked():
                    raise ValueError("cancel the active wait before detaching")
                result = self.run(["--detach"])
                self.owner = None
                return result
        return self.wait(cancel)


def serve(demux, root, bus):
    mailbox = Mailbox(demux, root, bus)
    output_lock, pending_lock = threading.Lock(), threading.Lock()
    pending = {}

    def send(payload):
        with output_lock:
            print(json.dumps({"jsonrpc": "2.0", **payload}, ensure_ascii=False), flush=True)

    def execute(identity, params, cancel):
        try:
            result = mailbox.call(params.get("name"), params.get("arguments", {}), cancel)
            response = {"content": [{"type": "text", "text": json.dumps(result, ensure_ascii=False)}]}
        except (ValueError, OSError, subprocess.SubprocessError) as error:
            response = {"isError": True, "content": [{"type": "text", "text": str(error)}]}
        try:
            if not cancel.is_set():
                send({"id": identity, "result": response})
        finally:
            with pending_lock:
                pending.pop(identity, None)

    with ThreadPoolExecutor(max_workers=4) as workers:
        try:
            for line in sys.stdin:
                try:
                    request = json.loads(line)
                    if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
                        raise ValueError("JSON-RPC 2.0 object required")
                except (ValueError, json.JSONDecodeError):
                    send({"id": None, "error": {"code": -32700, "message": "Invalid JSON-RPC message"}})
                    continue
                identity, method = request.get("id"), request.get("method")
                if identity is not None and (not isinstance(identity, (str, int)) or isinstance(identity, bool)):
                    send({"id": None, "error": {"code": -32600, "message": "String or integer id required"}})
                    continue
                params = request.get("params") or {}
                if not isinstance(params, dict):
                    send({"id": identity, "error": {"code": -32602, "message": "Object params required"}})
                    continue
                if method == "notifications/cancelled":
                    if not isinstance(params.get("requestId"), (str, int)):
                        continue
                    with pending_lock:
                        event = pending.get(params.get("requestId"))
                        if event:
                            event.set()
                    continue
                if identity is None:
                    continue
                if method == "initialize":
                    send({"id": identity, "result": {"protocolVersion": params.get("protocolVersion", "2024-11-05"),
                          "capabilities": {"tools": {}}, "serverInfo": {"name": "codescribe", "version": "1.0.0"}}})
                elif method == "ping":
                    send({"id": identity, "result": {}})
                elif method == "tools/list":
                    send({"id": identity, "result": {"tools": TOOLS}})
                elif method == "tools/call":
                    with pending_lock:
                        if identity in pending or len(pending) >= 4:
                            send({"id": identity, "error": {"code": -32600, "message": "Duplicate or excess request"}})
                            continue
                        cancel = pending[identity] = threading.Event()
                    workers.submit(execute, identity, params, cancel)
                else:
                    send({"id": identity, "error": {"code": -32601, "message": "Method not found"}})
        finally:
            with pending_lock:
                for cancel in pending.values():
                    cancel.set()
