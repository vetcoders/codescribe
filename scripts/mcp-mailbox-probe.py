#!/usr/bin/env python3
"""Private stdio MCP text probe over one existing Codescribe mailbox.

No attachment, model, HTTP listener, subscription, microphone or TTS. The local
launcher fixes the owner; remote tool arguments cannot change it. transcript.ready
is a record returned by read_pending, not a ChatGPT webhook subscription.
"""

import argparse
import contextlib
import fcntl
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import sys


SPEC = importlib.util.spec_from_file_location(
    "codescribe_mailbox_demux", Path(__file__).with_name("bus-demux.py"))
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)
LIMIT = 1 << 20
ID = re.compile(r"[0-9a-f]{24}")


def tool(name, description, properties, required=(), read_only=False):
    return {"name": name, "description": description,
            "inputSchema": {"type": "object", "properties": properties,
                            "required": list(required), "additionalProperties": False},
            "annotations": {"readOnlyHint": read_only}}


REQUEST_ID = {"type": "string", "pattern": "^[0-9a-f]{24}$"}
TOOLS = [
    tool("read_pending", "Read complete owned messages as transcript.ready records. Does not ACK.",
         {}, read_only=True),
    tool("ack", "Acknowledge one request_id fully read in this connection. ACK means read, not done.",
         {"request_id": REQUEST_ID}, ("request_id",)),
    tool("reply", "Publish one text reply to a request read and ACKed here. No speech. Retry with identical text only.",
         {"request_id": REQUEST_ID, "text": {"type": "string", "minLength": 1, "maxLength": 65536}},
         ("request_id", "text")),
]


class Mailbox:
    def __init__(self, provider, session, bridge_home, bus):
        self.args = argparse.Namespace(
            provider=provider.casefold(), session=session, bridge_home=bridge_home.resolve(),
            bus=bus.resolve(), bus_overridden=True, read_limit=2, read_bytes=LIMIT // 2,
            voice=None, speed=None, tts_vendor=None)
        self.lease = DEMUX.lease_identifier(self.args.provider, session)
        self.read_ids = set()

    def capture(self, operation):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            operation(self.args)
        return json.loads(output.getvalue())

    def call(self, name, arguments):
        definition = next((row for row in TOOLS if row["name"] == name), None)
        if not definition or not isinstance(arguments, dict):
            raise ValueError("unknown tool or invalid arguments")
        schema = definition["inputSchema"]
        if set(arguments) - set(schema["properties"]) or set(schema["required"]) - set(arguments):
            raise ValueError("unexpected or missing argument")
        if name == "read_pending":
            result = self.capture(DEMUX.read_pending_command)
            self.read_ids.update(result["read_delivery_ids"])
            return {"events": [{"event": "transcript.ready", "request_id": row["delivery_id"],
                                "delivery": row} for row in result["deliveries"]],
                    "remaining": result["remaining"]}
        identity = arguments["request_id"]
        if not isinstance(identity, str) or not ID.fullmatch(identity) or identity not in self.read_ids:
            raise ValueError("request must be fully read in this connection")
        if name == "ack":
            self.args.ack = [identity]
            return self.capture(DEMUX.acknowledge_delivery)
        text = arguments["text"]
        if not isinstance(text, str) or not text.strip() or len(text.encode("utf-8")) > 65536:
            raise ValueError("reply must contain 1–65536 UTF-8 bytes")
        if not DEMUX.delivery_acknowledged(self.args.bridge_home, self.lease, identity):
            raise ValueError("ACK the fully read request before replying")
        # Recheck ownership even for retries; a prior read cannot authorize a rebound bus.
        self.args.reply_to = identity
        self.args.say = text
        self.args.name = DEMUX.lease_name(self.args.bridge_home, self.args.provider, self.args.session)
        DEMUX.reply_delivery_envelope(self.args)
        directory = self.args.bridge_home / "runtime" / "mcp-probe" / self.lease
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        path = directory / f"{identity}.json"
        digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
        with (directory / f"{identity}.lock").open("a+b") as lock:
            os.fchmod(lock.fileno(), 0o600)
            fcntl.flock(lock, fcntl.LOCK_EX)
            receipt = DEMUX.read_reply_json(path) if path.exists() else None
            if receipt:
                if receipt.get("request_id") != identity:
                    raise ValueError("reply receipt ownership is invalid")
                if receipt.get("text_sha256") != digest:
                    raise ValueError("request already has a different reply")
                if receipt.get("state") != "published":
                    raise ValueError("publication outcome uncertain; inspect receipt before any retry")
                if not isinstance(receipt.get("reply_id"), str) or not ID.fullmatch(receipt["reply_id"]):
                    raise ValueError("reply receipt has no valid publication ID")
                return receipt
            DEMUX.atomic_json(path, {"request_id": identity, "text_sha256": digest, "state": "publishing"})
            _, reply = DEMUX.publish_text_reply(self.args)
            receipt = {"request_id": identity, "reply_id": reply["reply_id"],
                       "text_sha256": digest, "state": "published", "spoken": False}
            DEMUX.atomic_json(path, receipt)
            return receipt


def dispatch(mailbox, request):
    if not isinstance(request, dict) or request.get("jsonrpc") != "2.0" or not isinstance(request.get("method"), str):
        return {"jsonrpc": "2.0", "id": None, "error": {"code": -32600, "message": "Invalid request"}}
    if "id" not in request:
        # Notifications never execute write operations.
        return None
    identity = request["id"]
    method = request["method"]
    params = request.get("params", {})
    try:
        if not isinstance(params, dict):
            raise ValueError("invalid params")
        if method == "initialize":
            result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                      "serverInfo": {"name": "codescribe-mailbox-probe", "version": "0.1.0"}}
        elif method == "ping":
            result = {}
        elif method == "tools/list":
            result = {"tools": TOOLS}
        elif method == "tools/call":
            try:
                value = mailbox.call(params.get("name"), params.get("arguments", {}))
                result = {"content": [{"type": "text", "text": json.dumps(value, ensure_ascii=False)}]}
            except (OSError, ValueError, RuntimeError):
                # Keep paths, provider output and transcripts out of error diagnostics.
                result = {"isError": True, "content": [{"type": "text", "text":
                          "Mailbox operation refused or uncertain. Check owner, complete read, ACK and local receipts; do not blindly retry reply."}]}
        else:
            return {"jsonrpc": "2.0", "id": identity, "error": {"code": -32601, "message": "Method not found"}}
        return {"jsonrpc": "2.0", "id": identity, "result": result}
    except ValueError:
        return {"jsonrpc": "2.0", "id": identity, "error": {"code": -32602, "message": "Invalid params"}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", required=True)
    parser.add_argument("--session", required=True)
    parser.add_argument("--bridge-home", required=True, type=Path)
    parser.add_argument("--bus", required=True, type=Path)
    args = parser.parse_args()
    mailbox = Mailbox(args.provider, args.session, args.bridge_home, args.bus)
    while True:
        line = sys.stdin.buffer.readline(LIMIT + 1)
        if not line:
            return 0
        if len(line) > LIMIT:
            # Do not parse the suffix of an oversized request as a new command.
            return 2
        try:
            result = dispatch(mailbox, json.loads(line))
        except (ValueError, UnicodeDecodeError):
            result = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "Parse error"}}
        if result is not None:
            sys.stdout.write(json.dumps(result, ensure_ascii=False) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    raise SystemExit(main())
