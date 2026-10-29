"""Trusted host hook capabilities for the existing native mailbox transport.

Each request carries its session capability, so a shared MCP process has no
mutable global owner. This is a local bearer capability, not isolation from
other programs running as the same macOS user. Never include it in reports.
"""
import fcntl
import hashlib
import hmac
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import stat
import threading
import uuid


SCHEMA = "codescribe.native-session.v1"
TOKEN = re.compile(r"[0-9a-f]{32}\.[0-9a-f]{64}")


def private_directory(path):
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    for candidate in (path.parent, path):
        info = candidate.lstat()
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o022):
            raise PermissionError("Native session directory must be owned and not writable by others")
    return path


def read_record(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "r") as source:
        info = os.fstat(source.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o077 or info.st_size > 4096):
            raise PermissionError("Private native session record required")
        return json.load(source)


def host_hook(demux, root, payload):
    """Only the trusted host hook issues/revokes a capability; tools never do."""
    event = payload.get("hook_event_name")
    if event not in ("SessionStart", "UserPromptSubmit", "SessionEnd"):
        return {}
    session = payload.get("session_id")
    if not isinstance(session, str) or str(uuid.UUID(session)) != session:
        raise ValueError("The Codex hook must supply its real canonical session UUID")
    key = hashlib.sha256(session.encode()).hexdigest()[:32]
    directory = private_directory(root / "runtime" / "native-mcp-sessions")
    path = directory / (key + ".json")
    descriptor = os.open(directory / (key + ".lock"), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if event == "SessionEnd":
            if path.exists():
                record = read_record(path)
                if record.get("session_id") != session:
                    raise PermissionError("Session record owner mismatch")
                path.unlink()
            return {}  # Keep lease, cursor, pending, ACKs and follower intact.
        try:
            record = read_record(path)
        except FileNotFoundError:
            record = {"schema": SCHEMA, "provider": "codex", "session_id": session,
                      "capability": key + "." + secrets.token_hex(32)}
            demux.atomic_json(path, record)
        if (record.get("schema") != SCHEMA or record.get("provider") != "codex"
                or record.get("session_id") != session or not TOKEN.fullmatch(record.get("capability", ""))):
            raise PermissionError("Invalid native session record; existing state retained")
    context = (f"Codescribe native identity: provider=codex session_id={session}. "
               f"Use session_capability={record['capability']} in every Codescribe MCP call. "
               "This private capability belongs to this parent conversation; do not print it, copy it to another "
               "conversation or give a subagent a separate mailbox with it. Connect through codescribe_connect "
               "with the requested name and a free channel. Read full deliveries, then ACK exact returned IDs "
               "before work. Reply with a stable request_id; speak=true only for explicitly requested speech. "
               "Codex queue is the local idle wake owner; an async hook is not an idle wake. "
               "Queue acceptance, a new turn, read, ACK, text publication and playback are separate evidence.")
    return {"hookSpecificOutput": {"hookEventName": event, "additionalContext": context}}


class SessionCapabilities:
    def __init__(self, demux, native, root, bus=None):
        self.demux, self.native, self.root, self.bus = demux, native, root, bus
        self.mailboxes = {}
        self.lock = threading.RLock()
        spec = importlib.util.spec_from_file_location("codescribe_agent_mcp", Path(demux.__file__).with_name("bus-mcp-agent.py"))
        self.agent = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.agent)
        self.tools = json.loads(json.dumps([t for t in native.TOOLS if t["name"] != "codescribe_wait"]))
        for tool in self.tools:
            schema = tool["inputSchema"]
            schema["properties"]["session_capability"] = {"type": "string", "pattern": TOKEN.pattern}
            schema["required"].append("session_capability")
            if tool["name"] == "codescribe_connect":
                for key in ("provider", "session_id"):
                    del schema["properties"][key]
                    schema["required"].remove(key)
                tool["description"] = "Connect this trusted Codex hook session to a free channel. Never take over another conversation."
            if tool["name"] == "codescribe_reply":
                schema["properties"].update(request_id={"type": "string", "pattern": "^[0-9a-f]{24}$"},
                                            speak={"type": "boolean"})
                schema["required"].append("request_id")
                tool["description"] = "Publish an owned read/ACK reply. Reuse request_id on retry; uncertain never replays. Default is text only; speak=true requires an explicit voice request."

    def resolve(self, token):
        if not isinstance(token, str) or not TOKEN.fullmatch(token):
            raise PermissionError("Trusted Codescribe session hook capability required")
        directory = private_directory(self.root / "runtime" / "native-mcp-sessions")
        try:
            record = read_record(directory / (token[:32] + ".json"))
        except FileNotFoundError as error:
            raise PermissionError("Session capability unavailable or revoked; trust and run the plugin session hook") from error
        session = record.get("session_id")
        if (record.get("schema") != SCHEMA or record.get("provider") != "codex"
                or not isinstance(session, str) or str(uuid.UUID(session)) != session
                or hashlib.sha256(session.encode()).hexdigest()[:32] != token[:32]
                or not hmac.compare_digest(record.get("capability", ""), token)):
            raise PermissionError("Session capability does not match its host-issued scope")
        with self.lock:
            if token not in self.mailboxes:
                if len(self.mailboxes) >= 128:
                    raise ValueError("Native session capacity reached; reconnect the MCP transport")
                mailbox = self.native.Mailbox(self.demux, self.root, self.bus)
                mailbox.environment = self.agent.child_environment(os.environ, self.demux.BUS_PATH_ENV_KEYS)
                mailbox.provider, mailbox.owner = "codex", session
                self.mailboxes[token] = mailbox
            return self.mailboxes[token], session

    def call(self, name, values, cancel):
        definition = next((t for t in self.tools if t["name"] == name), None)
        if definition is None or not isinstance(values, dict):
            raise ValueError("Unknown session-scoped Codescribe tool")
        schema = definition["inputSchema"]
        if not set(schema["required"]) <= set(values) <= set(schema["properties"]):
            raise ValueError("Unexpected or missing session-scoped arguments")
        mailbox, session = self.resolve(values["session_capability"])
        arguments = {k: v for k, v in values.items() if k != "session_capability"}
        with mailbox.lock:
            mailbox.provider, mailbox.owner = "codex", session
            if name == "codescribe_connect":
                return mailbox.call(name, {**arguments, "provider": "codex", "session_id": session}, cancel)
            if name != "codescribe_reply":
                return mailbox.call(name, arguments, cancel)
            delivery, request, text = arguments["delivery_id"], arguments["request_id"], arguments["text"]
            speak = arguments.get("speak", False)
            if (not isinstance(delivery, str) or not re.fullmatch(r"[0-9a-f]{24}", delivery)
                    or not isinstance(request, str) or not re.fullmatch(r"[0-9a-f]{24}", request)
                    or not isinstance(text, str) or not text.strip() or len(text.encode()) > 65536
                    or type(speak) is not bool or speak and len(text) > 1200):
                raise ValueError("Reply requires a full read/ACK, bounded text, request_id and boolean speech choice")
            from types import SimpleNamespace
            coordinates = SimpleNamespace(bridge_home=self.root, provider="codex", session=session,
                                          bus=mailbox.bus, bus_overridden=mailbox.bus is not None, reply_to=delivery)
            _bus, envelope = self.demux.reply_delivery_envelope(coordinates)
            lease = self.demux.lease_identifier("codex", session)
            if not self.demux.delivery_acknowledged(self.root, lease, delivery, envelope):
                raise PermissionError("Reply requires an owned canonical read/ACK receipt")
            # The constrained connector already owns durable idempotency and
            # playback receipts. Reuse it rather than adding a second reply store.
            agent = self.agent
            state = self.demux.read_json(self.root / "leases" / (self.demux.lease_identifier("codex", session) + ".json")) or {}
            scope = agent.Scope({"principal": session, "provider": "codex", "session_id": session,
                                 "name": state.get("name"), "bridge_home": str(self.root), "bus": state.get("bus"),
                                 "state_dir": str(self.root / "runtime" / "agent-mcp")})
            replies = agent.AgentMailbox(self.demux, mailbox, scope)
            flags = ["--say", text, "--reply-to", delivery]
            if not speak:
                flags.append("--text-only")
            return replies.request_once(request, "say" if speak else "reply", arguments, flags, speech=speak)
