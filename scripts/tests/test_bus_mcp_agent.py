"""Synthetic agent MCP + Events acceptance. No network, credentials or sound.

Two fictional agents share one bridge here: Ada and Lin. Everything either one
is allowed to touch comes from its own reviewed scope, so a test that lets one
act through the other's lease, binding or name is the regression this file
exists to catch.
"""
import argparse
import base64
import contextlib
import hashlib
import hmac
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


DEMUX = load("agent_test_demux", "bus-demux.py")
NATIVE = load("agent_test_native", "bus-mcp.py")
AGENT = load("agent_test_surface", "bus-mcp-agent.py")
EVENTS = load("agent_test_events", "bus-mcp-events.py")
RUNTIME_SCRIPT = SCRIPTS / "bus-mcp-runtime.py"
SESSION = "11111111-2222-4333-8444-555555555555"
OTHER_SESSION = "99999999-8888-4777-8666-555555555555"
PRINCIPAL = "synthetic-agent-owner"
SECRET = "whsec_" + base64.b64encode(b"synthetic-signing-key-only-0000000").decode()
URL = "https://callback.example.test/agent"
TIME = "2026-10-14T12:00:00Z"
CALL_TEXT = "This agent is ready and waiting."
MESSAGE = "MESSAGE_TEXT_PRIVATE"
# Every request carries its own protocol version and capabilities: the server
# keeps no handshake state, so a frame is accepted or refused on its own terms.
META = {"io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}}


class FakeTimer:
    def __init__(self, alarms, delay, callback):
        self.alarms, self.delay, self.callback, self.cancelled = alarms, delay, callback, False

    def start(self):
        self.alarms.append(self)

    def cancel(self):
        self.cancelled = True

    def fire(self):
        if not self.cancelled:
            self.callback()


class Fixture(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.config = {"provider": "codex", "session_id": SESSION, "name": "Ada",
                       "principal": PRINCIPAL, "bridge_home": str(self.root), "bus": str(self.bus),
                       "state_dir": str(self.root / "runtime" / "ada-mcp"), "call_text": CALL_TEXT}
        self.lease = DEMUX.lease_identifier("codex", SESSION)
        self.ids = [f"{n:024x}" for n in range(1, 6)]
        owner = {"provider": "codex", "provider_session_id": SESSION, "lease_id": self.lease}
        self.pending = [{**owner, "delivery_owner": dict(owner), "delivery_id": identity,
                         "bus": str(self.bus), "emitted_at": TIME, "kind": "message", "source": "typed",
                         "text": MESSAGE, "instructions": "untrusted", "attachments": [{"path": "/private/audio.wav"}]}
                        for identity in self.ids]
        self.state = {"schema": DEMUX.LEASE_SCHEMA, **owner, "name": "Ada", "bus": str(self.bus),
                      "pending": self.pending, "cursor": 0, "wakeup": "off"}
        self.save()
        self.native = NATIVE.Mailbox(DEMUX, self.root, self.bus)
        self.native.command = [sys.executable, str(SCRIPTS / "bus-demux.py")]
        self.server = AGENT.AgentMailbox(DEMUX, self.native, AGENT.Scope(self.config))
        self.now = [1791979200.0]
        self.posts, self.statuses = [], []
        self.alarms = []
        self.permitted = [True]
        self.secret_store = EVENTS.MemorySecretStore()
        self.events = self.new_events()

    def save(self):
        DEMUX.atomic_json(self.root / "leases" / (self.lease + ".json"), self.state)

    def new_events(self, **changes):
        args = {"callback": self.callback, "resolve": lambda host: ["1.1.1.1"],
                "callback_hosts": ["callback.example.test"], "secret_store": self.secret_store, "clock": lambda: self.now[0],
                "authorized": lambda: self.permitted[0],
                "timer_factory": lambda delay, callback: FakeTimer(self.alarms, delay, callback)}
        args.update(changes)
        events = EVENTS.Events(self.root / "synthetic-events.sqlite", PRINCIPAL, SESSION, **args)
        self.addCleanup(events.close)
        return events

    def callback(self, url, addresses, body, headers):
        self.posts.append((url, addresses, body, dict(headers)))
        value = json.loads(body)
        if value.get("type") == "verification":
            return 200, {"challenge": value["challenge"]}
        return (self.statuses.pop(0) if self.statuses else 202), {}

    def subscription(self, **changes):
        params = {"name": EVENTS.EVENT_NAME, "arguments": {}, "delivery": {
            "mode": "webhook", "url": URL, "secret": SECRET}, "cursor": None}
        params.update(changes)
        return params

    def subscribe(self, **changes):
        return self.events.subscribe(PRINCIPAL, self.subscription(**changes))

    def call(self, name, args=None):
        return self.server.call(PRINCIPAL, "codescribe_" + name, {} if args is None else args)

    def assert_signature(self, body, headers, secret=SECRET):
        message = headers["webhook-id"].encode() + b"." + headers["webhook-timestamp"].encode() + b"." + body
        expected = "v1," + base64.b64encode(hmac.new(EVENTS.signing_key(secret), message, hashlib.sha256).digest()).decode()
        self.assertIn(expected, headers["webhook-signature"].split())


class ScopeContractTests(Fixture):
    """The scope is the only identity: no name, provider or path is compiled in."""

    def test_state_directory_is_one_reviewed_private_child_of_the_runtime_root(self):
        runtime = self.root / "runtime"
        created = AGENT.Scope(self.config).state
        self.assertEqual(created, runtime / "ada-mcp")
        self.assertEqual(stat.S_IMODE(created.stat().st_mode), 0o700)
        # An existing reviewed directory is adopted, not replaced.
        marker = created / "keep-me"
        marker.mkdir()
        self.assertEqual(AGENT.Scope(self.config).state, created)
        self.assertTrue(marker.is_dir())
        foreign = self.root / "elsewhere"
        foreign.mkdir()
        link = runtime / "linked-mcp"
        link.symlink_to(foreign)
        shared = runtime / "shared-mcp"
        shared.mkdir()
        os.chmod(shared, 0o770)
        for state_dir in (str(self.root / "ada-mcp"), str(runtime / "nested" / "ada-mcp"),
                          str(runtime / "ada"), str(runtime / "Ada-MCP"), str(link), str(shared),
                          str(runtime / "ada-mcp" / "keep-me"), "runtime/ada-mcp", str(created / "..")):
            with self.subTest(state_dir=state_dir), self.assertRaises(ValueError):
                AGENT.Scope({**self.config, "state_dir": state_dir})
        for missing in (None, {}):
            scope = dict(self.config)
            if missing is None:
                scope.pop("state_dir")
            else:
                scope["state_dir"] = missing
            with self.assertRaises(ValueError):
                AGENT.Scope(scope)

    def test_state_layout_and_signing_key_namespace_match_a_named_directory(self):
        # A deployment keeps its signing keys by naming the directory it already
        # uses: path and namespace must equal the independently built layout.
        scope = AGENT.Scope(self.config)
        expected = self.root / "runtime" / "ada-mcp" / self.lease
        self.assertEqual(scope.state / self.lease, expected)
        self.assertEqual(AGENT.AgentMailbox(DEMUX, self.native, scope).calls, expected / "calls")
        self.assertEqual(EVENTS.key_namespace(PRINCIPAL, SESSION, scope.state / self.lease / "events.sqlite"),
                         EVENTS.key_namespace(PRINCIPAL, SESSION, expected / "events.sqlite"))
        renamed = AGENT.Scope({**self.config, "state_dir": str(self.root / "runtime" / "other-mcp")})
        self.assertNotEqual(EVENTS.key_namespace(PRINCIPAL, SESSION, renamed.state / self.lease / "events.sqlite"),
                            EVENTS.key_namespace(PRINCIPAL, SESSION, expected / "events.sqlite"))

    def test_absent_call_text_removes_the_fixed_call_entirely(self):
        config = {k: v for k, v in self.config.items() if k != "call_text"}
        server = AGENT.AgentMailbox(DEMUX, self.native, AGENT.Scope(config))
        self.assertNotIn("codescribe_call", {t["name"] for t in server.tools()})
        with patch.object(self.native, "run", side_effect=AssertionError("call attempted")):
            with self.assertRaises(ValueError):
                server.call(PRINCIPAL, "codescribe_call", {"request_id": "a" * 24})
        self.assertEqual(self.call("call", {"request_id": "a" * 24})["text"], CALL_TEXT)
        for bad in ("", "   ", "x" * 201, "with\nnewline", 7, None):
            with self.subTest(call_text=repr(bad)), self.assertRaises(ValueError):
                AGENT.Scope({**self.config, "call_text": bad})

    def test_scope_identity_fields_are_bounded_and_never_defaulted(self):
        for change in ({"provider": "Codex"}, {"provider": "code x"}, {"provider": ""},
                       {"provider": "a" * 65}, {"provider": 1}, {"name": ""}, {"name": "x" * 81},
                       {"name": "line\nbreak"}, {"session_id": "not-a-uuid"},
                       {"session_id": SESSION.replace("-", "")}, {"principal": ""},
                       {"bridge_home": "relative/path"}, {"bus": "relative/path"},
                       {"speech_enabled": "yes"}, {"mailbox_enabled": 1},
                       {"broadcast_enabled": None}):
            with self.subTest(change=list(change)), self.assertRaises(ValueError):
                AGENT.Scope({**self.config, **change})
        with self.assertRaises(ValueError):
            AGENT.Scope({**self.config, "unknown": True})
        scope = AGENT.Scope(self.config)
        self.assertEqual((scope.provider, scope.name), ("codex", "Ada"))
        self.assertFalse(scope.speech_enabled or scope.broadcast_enabled)
        self.assertTrue(scope.mailbox_enabled)

    def test_check_scope_prints_the_resolved_identity_without_any_side_effect(self):
        path = self.root / "scope.json"
        path.write_text(json.dumps(self.config))
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*"))
        result = subprocess.run([sys.executable, str(RUNTIME_SCRIPT), "--scope", str(path), "--check-scope"],
                                text=True, capture_output=True, check=True, timeout=15)
        value = json.loads(result.stdout)
        expected = self.root / "runtime" / "ada-mcp" / self.lease
        self.assertEqual(value, {"schema": "codescribe.agent-mcp.scope-check.v1", "provider": "codex",
                                 "name": "Ada", "lease_id": self.lease, "state_dir_exists": True,
                                 "state_path": str(expected),
                                 "events_path": str(expected / "events.sqlite"),
                                 "key_namespace": EVENTS.key_namespace(PRINCIPAL, SESSION, expected / "events.sqlite")})
        self.assertNotIn(PRINCIPAL, result.stdout)
        self.assertEqual(sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*")), before)
        self.assertFalse((self.root / "acknowledgments").exists())
        self.assertFalse(expected.exists())

    def test_a_symlinked_runtime_root_cannot_relocate_state(self):
        home = self.root / "other-home"
        elsewhere = self.root / "elsewhere"
        home.mkdir()
        elsewhere.mkdir()
        (home / "runtime").symlink_to(elsewhere, target_is_directory=True)
        config = {**self.config, "bridge_home": str(home), "state_dir": str(home / "runtime" / "x-mcp")}
        with self.assertRaises(ValueError):
            AGENT.Scope(config)
        self.assertEqual(list(elsewhere.iterdir()), [])

    def test_validation_and_check_scope_never_create_an_absent_state_directory(self):
        absent = self.root / "runtime" / "lin-mcp"
        config = {**self.config, "state_dir": str(absent)}
        scope = AGENT.Scope(config)
        self.assertFalse(os.path.lexists(absent))
        path = self.root / "absent-scope.json"
        path.write_text(json.dumps(config))
        result = subprocess.run([sys.executable, str(RUNTIME_SCRIPT), "--scope", str(path), "--check-scope"],
                                text=True, capture_output=True, check=True, timeout=15)
        value = json.loads(result.stdout)
        self.assertFalse(value["state_dir_exists"])
        self.assertEqual(value["events_path"], str(absent / self.lease / "events.sqlite"))
        self.assertFalse(os.path.lexists(absent))
        # Only a starting connection prepares it, privately.
        self.assertEqual(scope.prepare_state(), absent)
        self.assertEqual(stat.S_IMODE(absent.stat().st_mode), 0o700)
        self.assertEqual(scope.prepare_state(), absent)

    def test_check_scope_writes_no_bytecode_next_to_the_components(self):
        scripts = self.root / "payload-bin"
        scripts.mkdir()
        for name in ("bus-mcp-runtime.py", "bus-mcp-agent.py", "bus-mcp-events.py", "bus-demux.py"):
            (scripts / name).write_bytes((SCRIPTS / name).read_bytes())
        path = self.root / "scope.json"
        path.write_text(json.dumps(self.config))
        environment = {k: v for k, v in os.environ.items() if not k.startswith("PYTHON")}
        result = subprocess.run([sys.executable, str(scripts / "bus-mcp-runtime.py"), "--scope", str(path),
                                 "--check-scope"], text=True, capture_output=True, check=True, timeout=15,
                                env=environment)
        self.assertEqual(json.loads(result.stdout)["lease_id"], self.lease)
        self.assertEqual(sorted(p.name for p in scripts.iterdir()),
                         ["bus-demux.py", "bus-mcp-agent.py", "bus-mcp-events.py", "bus-mcp-runtime.py"])

    def child_view(self, planted):
        """What a BUS helper child sees: its environment and the canonical bus it resolves."""
        dump = self.root / "dump-env.py"
        dump.write_text(
            "import importlib.util, json, os, sys\n"
            "spec = importlib.util.spec_from_file_location('child_demux', sys.argv[1])\n"
            "demux = importlib.util.module_from_spec(spec)\n"
            "spec.loader.exec_module(demux)\n"
            "print(json.dumps({'env': dict(os.environ), 'bus_path': str(demux.bus_path())}))\n")
        with patch.dict(os.environ, planted, clear=True):
            server = AGENT.AgentMailbox(DEMUX, NATIVE.Mailbox(DEMUX, self.root, self.bus), AGENT.Scope(self.config))
        server.mailbox.command = [sys.executable, "-B", str(dump), str(SCRIPTS / "bus-demux.py")]
        return server.mailbox.run(["--status"])

    def test_bus_helper_children_receive_only_the_allowlisted_environment(self):
        planted = {"TRANSPORT_TOKEN": "transport-secret", "CODESCRIBE_STT_SIDECAR_TOKEN": "sidecar-secret",
                   "LLM_ANTHROPIC_API_KEY": "model-secret", "LLM_OPENAI_API_KEY": "speech-key",
                   "PATH": "/usr/bin:/bin", "HOME": str(self.root), "LANG": "pl_PL.UTF-8", "LC_ALL": "C",
                   "TZ": "UTC"}
        seen = self.child_view(planted)["env"]
        for name in ("TRANSPORT_TOKEN", "CODESCRIBE_STT_SIDECAR_TOKEN", "LLM_ANTHROPIC_API_KEY"):
            self.assertNotIn(name, seen)
        self.assertNotIn("transport-secret", json.dumps(seen))
        # The speech credential the BUS helper itself consumes before Keychain.
        for name in ("LLM_OPENAI_API_KEY", "PATH", "HOME", "LANG", "LC_ALL", "TZ"):
            self.assertEqual(seen[name], planted[name])
        allowed = AGENT.CHILD_ENVIRONMENT | set(DEMUX.BUS_PATH_ENV_KEYS) | {"__CF_USER_TEXT_ENCODING"}
        self.assertTrue({k for k in seen if not k.startswith("LC_")} <= allowed)

    def test_bus_helper_children_resolve_the_same_canonical_bus_as_the_agent(self):
        custom = self.root / "custom" / "transcript.jsonl"
        base = {"PATH": "/usr/bin:/bin", "HOME": str(self.root), "TRANSPORT_TOKEN": "transport-secret"}
        for config in ({"CODESCRIBE_TRANSCRIPT_BUS_PATH": str(custom)},
                       {"XDG_STATE_HOME": str(self.root / "state")},
                       {"CODESCRIBE_DATA_DIR": str(self.root / "data")},
                       {"CODESCRIBE_ENV_PATH": str(self.root / "elsewhere.env")}):
            with self.subTest(config=list(config)):
                (self.root / "elsewhere.env").write_text(f"CODESCRIBE_TRANSCRIPT_BUS_PATH={custom}\n")
                with patch.dict(os.environ, {**base, **config}, clear=True):
                    expected = str(DEMUX.bus_path())
                view = self.child_view({**base, **config})
                self.assertEqual(view["bus_path"], expected)
                self.assertNotIn("TRANSPORT_TOKEN", view["env"])
        self.assertNotEqual(expected, str(self.root / ".codescribe" / DEMUX.BUS_FILENAME))

    def test_check_scope_refuses_other_modes_and_an_invalid_scope(self):
        path = self.root / "scope.json"
        path.write_text(json.dumps(self.config))
        for extra in (["--callback-preflight"], ["--callback-host", "callback.example.test"],
                      ["--codescribe", str(self.root / "codescribe"), "--native-sha256", "0" * 64]):
            result = subprocess.run([sys.executable, str(RUNTIME_SCRIPT), "--scope", str(path),
                                     "--check-scope", *extra], text=True, capture_output=True, timeout=15)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
        path.write_text(json.dumps({k: v for k, v in self.config.items() if k != "state_dir"}))
        result = subprocess.run([sys.executable, str(RUNTIME_SCRIPT), "--scope", str(path), "--check-scope"],
                                text=True, capture_output=True, timeout=15)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")


class ForeignAgentTests(Fixture):
    """Ada and Lin are separate connections; neither inherits the other's rights."""

    def lin(self, **changes):
        """A second connection with its own native mailbox, as a second client has."""
        config = {**self.config, "provider": "claude-code", "session_id": OTHER_SESSION, "name": "Lin",
                  "state_dir": str(self.root / "runtime" / "lin-mcp"), **changes}
        self.other_native = NATIVE.Mailbox(DEMUX, self.root, self.bus)
        self.other_native.command = [sys.executable, str(SCRIPTS / "bus-demux.py")]
        return AGENT.AgentMailbox(DEMUX, self.other_native, AGENT.Scope(config))

    def test_each_agent_keeps_its_own_lease_state_and_namespace(self):
        lin = self.lin()
        lease = DEMUX.lease_identifier("claude-code", OTHER_SESSION)
        self.assertNotEqual(lin.lease, self.server.lease)
        self.assertEqual(lin.calls, self.root / "runtime" / "lin-mcp" / lease / "calls")
        self.assertNotEqual(
            EVENTS.key_namespace(PRINCIPAL, OTHER_SESSION, lin.calls.parent / "events.sqlite"),
            EVENTS.key_namespace(PRINCIPAL, SESSION, self.server.calls.parent / "events.sqlite"))

    def test_an_agent_cannot_read_or_ack_through_a_foreign_lease(self):
        # Ada's mailbox holds the pending messages; Lin's lease does not exist.
        self.assertEqual(self.call("read")["read_delivery_ids"], self.ids[:1])
        lin = self.lin()
        with self.assertRaises(DEMUX.MailboxReadError) as refusal:
            lin.call(PRINCIPAL, "codescribe_read", {})
        self.assertEqual(refusal.exception.reason, "mailbox_owner_mismatch")
        with self.assertRaises(ValueError):
            lin.call(PRINCIPAL, "codescribe_ack", {"delivery_ids": self.ids[:1]})
        # The same session with the wrong name on the lease is refused outright.
        with self.assertRaises(PermissionError):
            self.lin(provider="codex", session_id=SESSION).call(PRINCIPAL, "codescribe_read", {})

    def test_a_mismatched_provider_resolves_to_a_different_lease_not_a_shared_one(self):
        # Same session id and same name, different provider: the lease key is
        # derived from both, so Ada's pending messages stay out of reach.
        mismatched = self.lin(name="Ada", session_id=SESSION)
        self.assertNotEqual(mismatched.lease, self.server.lease)
        with self.assertRaises(DEMUX.MailboxReadError) as refusal:
            mismatched.call(PRINCIPAL, "codescribe_read", {})
        self.assertEqual(refusal.exception.reason, "mailbox_owner_mismatch")
        self.assertEqual(self.call("read")["read_delivery_ids"], self.ids[:1])
        # A lease written under the mismatched provider is its own, not Ada's.
        DEMUX.atomic_json(self.root / "leases" / (mismatched.lease + ".json"),
                          {**self.state, "provider": "claude-code", "lease_id": mismatched.lease})
        mismatched.scope.speech_enabled = True
        with patch.object(self.other_native, "run", return_value={"receipts": []}) as run:
            mismatched.call(PRINCIPAL, "codescribe_call", {"request_id": "a" * 24})
            self.assertEqual(run.call_args.args[0], ["--say", CALL_TEXT, "--name", "Ada"])
        self.assertTrue((mismatched.calls / ("a" * 24 + ".json")).exists())
        self.assertFalse((self.server.calls / ("a" * 24 + ".json")).exists())

    def test_a_fixed_call_refuses_a_lease_owned_by_the_other_agent(self):
        self.state["name"] = "Lin"
        self.save()
        self.server.scope.speech_enabled = True
        with patch.object(self.native, "run", side_effect=AssertionError("foreign lease used")):
            with self.assertRaises(PermissionError):
                self.call("call", {"request_id": "a" * 24})

    def test_a_binding_owned_by_the_other_agent_cannot_be_used_to_send(self):
        DEMUX.write_channel_binding(self.root, "2", "Lin", "claude-code", OTHER_SESSION, str(self.bus))
        DEMUX.write_channel_binding(self.root, "3", "Peer", "codex", "peer-3", str(self.bus))
        server = AGENT.AgentMailbox(DEMUX, self.native, AGENT.Scope({**self.config, "broadcast_enabled": True}))
        with patch.object(self.native, "run", side_effect=AssertionError("foreign binding used")):
            for target in ("0", "3"):
                with self.subTest(to=target), self.assertRaises(PermissionError):
                    server.call(PRINCIPAL, "codescribe_send",
                                {"request_id": "a" * 24, "text": "Hello", "to": target})

    def test_a_principal_from_another_connection_is_refused_before_any_work(self):
        with patch.object(self.native, "run", side_effect=AssertionError("foreign principal used")):
            for name in ("status", "read", "call"):
                with self.subTest(tool=name), self.assertRaises(PermissionError):
                    self.server.call("someone-else", "codescribe_" + name, {})


class AgentToolsTests(Fixture):
    def test_phase_one_discovery_and_direct_calls_allow_only_status_and_call(self):
        self.server = AGENT.AgentMailbox(DEMUX, self.native,
            AGENT.Scope({**self.config, "mailbox_enabled": False}), events=self.events)
        self.assertEqual({t["name"] for t in self.server.dispatch(PRINCIPAL, "tools/list", {})["tools"]},
                         {"codescribe_status", "codescribe_call"})
        self.assertNotIn("events", self.server.dispatch(PRINCIPAL, "server/discover", {})["capabilities"])
        for name, values in (("read", {}), ("ack", {"delivery_ids": self.ids[:1]})):
            with self.assertRaises(ValueError):
                self.call(name, values)
        with self.assertRaises(ValueError):
            self.server.dispatch(PRINCIPAL, "events/subscribe", self.subscription())
        self.assertEqual(self.server.pump_events(PRINCIPAL), [])

    def test_fixed_outbound_call_needs_no_follower_and_refuses_foreign_existing_lease(self):
        (self.root / "leases" / (self.lease + ".json")).unlink()
        self.server.scope.speech_enabled = True
        with patch.object(self.native, "run", return_value={"receipts": []}) as run:
            self.call("call", {"request_id": "a" * 24})
            self.assertEqual(run.call_count, 1)
            self.assertEqual(run.call_args.args[0], ["--say", CALL_TEXT, "--name", "Ada"])
        self.state["name"] = "Lin"
        self.save()
        with patch.object(self.native, "run", side_effect=AssertionError("foreign lease used")):
            with self.assertRaises(PermissionError):
                self.call("call", {"request_id": "b" * 24})

    def test_unsolicited_speech_without_a_lease_cannot_borrow_a_bound_name(self):
        (self.root / "leases" / (self.lease + ".json")).unlink()
        self.server.scope.speech_enabled = True
        binding = self.root / DEMUX.AUDIENCE_BINDING_FILENAME
        def bind(provider, session):
            DEMUX.atomic_json(binding, {"schema": DEMUX.AUDIENCE_BINDING_SCHEMA, "bindings": {"4": {
                "audience": "ada", "bus": str(self.bus), "provider": provider, "provider_session_id": session}}})
        bind("codex", OTHER_SESSION)
        with patch.object(self.native, "run", side_effect=AssertionError("borrowed name spoke")):
            for tool, args in (("call", {"request_id": "c" * 24}),
                               ("say", {"request_id": "d" * 24, "text": "Hello."})):
                with self.subTest(tool=tool), self.assertRaises(PermissionError):
                    self.call(tool, args)
        bind("claude-code", SESSION)  # Same session, other provider: still another participant.
        with patch.object(self.native, "run", side_effect=AssertionError("borrowed name spoke")):
            with self.assertRaises(PermissionError):
                self.call("call", {"request_id": "e" * 24})
        bind("codex", SESSION)  # This agent's own binding.
        with patch.object(self.native, "run", return_value={"receipts": []}) as run:
            self.call("call", {"request_id": "f" * 24})
            self.assertEqual(run.call_count, 1)

    def test_six_tools_and_no_model_controlled_owner_or_path(self):
        tools = self.server.dispatch(PRINCIPAL, "tools/list", {})["tools"]
        self.assertEqual({t["name"] for t in tools}, {"codescribe_status", "codescribe_read", "codescribe_ack", "codescribe_call", "codescribe_send", "codescribe_say"})
        for tool in tools:
            self.assertFalse(set(tool["inputSchema"]["properties"]) & {"command", "path", "session", "provider", "name"})
        send = next(t for t in tools if t["name"] == "codescribe_send")
        self.assertNotIn("enum", send["inputSchema"]["properties"]["to"])
        self.assertFalse(self.server.scope.broadcast_enabled)
        for name in ("connect", "detach", "reply", "shell", "read_file"):
            with self.assertRaises(ValueError):
                self.call(name)
        with self.assertRaises(ValueError):
            self.call("status", {"session": "other"})
        with self.assertRaises(PermissionError):
            self.server.call("someone-else", "codescribe_read", {})

    def speech_receipts(self):
        return {"receipts": [{"kind": "agent_reply", "reply_id": "b" * 24},
                {"kind": "agent_reply_playback", "reply_id": "b" * 24, "state": "spoken", "spoken": True,
                 "provider": "codex", "provider_session_id": SESSION, "lease_id": self.lease}]}

    def test_normal_say_supports_owned_ack_association_and_exact_retry(self):
        self.call("read")
        with self.assertRaises(PermissionError):
            self.call("say", {"request_id": "a" * 24, "text": "Answering.", "reply_to": self.ids[0]})
        self.call("ack", {"delivery_ids": self.ids[:1]})
        self.server.scope.speech_enabled = True
        values = {"request_id": "a" * 24, "text": "Message received.", "reply_to": self.ids[0]}
        with patch.object(self.native, "run", return_value=self.speech_receipts()) as run:
            first = self.call("say", values)
            self.server = AGENT.AgentMailbox(DEMUX, self.native, AGENT.Scope({**self.config, "speech_enabled": True}))
            second = self.call("say", values)
            with self.assertRaises(ValueError):
                self.call("say", {**values, "text": "A different answer."})
        self.assertTrue(first["spoken"])
        self.assertEqual(first, second)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0], ["--say", values["text"], "--name", "Ada", "--reply-to", self.ids[0]])

    def test_say_rejects_foreign_delivery_and_bounds_text_without_speech(self):
        with patch.object(self.native, "run", side_effect=AssertionError("voice attempted")):
            with self.assertRaises(ValueError):
                self.call("say", {"request_id": "a" * 24, "text": "Hello", "reply_to": "f" * 24})
            with self.assertRaises(ValueError):
                self.call("say", {"request_id": "a" * 24, "text": "x" * 1201})
            result = self.call("say", {"request_id": "a" * 24, "text": "The task is finished."})
        self.assertEqual(result["state"], "prepared")
        self.assertFalse(result["speech_attempted"])

    def test_send_uses_native_publisher_and_owned_sender_no_echo_and_dedupes(self):
        DEMUX.write_channel_binding(self.root, "2", "Ada", "codex", SESSION, str(self.bus))
        for slot, name in (("1", "Lin"), ("3", "Nova")):
            DEMUX.write_channel_binding(self.root, slot, name, "codex", "peer-" + slot, str(self.root / ("peer-" + slot + ".jsonl")))
        self.server = AGENT.AgentMailbox(DEMUX, self.native,
            AGENT.Scope({**self.config, "broadcast_enabled": True}))
        published = []

        def publisher(bus, event, **kwargs):
            published.append(event)
            return {"stream_id": "synthetic", "stream_dev": 1, "stream_inode": 2, "offset": 0, "length": 12}

        def native_send(flags, **kwargs):
            args = argparse.Namespace(bridge_home=self.root, provider="codex", session=SESSION,
                                      send=flags[1], to=flags[3])
            stream = io.StringIO()
            with patch.object(DEMUX, "publish_reply_event", side_effect=publisher), \
                    patch.object(DEMUX, "live_follower_pid", return_value=None), contextlib.redirect_stdout(stream):
                DEMUX.send_peer_command(args)
            return json.loads(stream.getvalue())

        values = {"request_id": "a" * 24, "text": "Hello, this agent is on the BUS.", "to": "0"}
        with patch.object(self.native, "run", side_effect=native_send) as run:
            first = self.call("send", values)
            second = self.call("send", values)
            with self.assertRaises(ValueError):
                self.call("send", {**values, "text": "A different message"})
            self.assertEqual(run.call_count, 1)
        self.assertEqual(first, second)
        self.assertEqual(first["state"], "published")
        self.assertEqual({e["peer_to"] for e in published}, {"lin", "nova"})
        self.assertEqual({e["sender"]["name"] for e in published}, {"Ada"})
        self.assertEqual(len({e["message_id"] for e in published}), 1)
        for event in published:
            payload = DEMUX.consider(event, name=event["peer_to"], hear_all=False, drafts=False, debug=False)
            self.assertFalse(payload["state_change_allowed"])
        metadata = (self.server.calls.parent / "send" / ("a" * 24 + ".json")).read_text()
        self.assertNotIn(values["text"], metadata)

    def test_roster_and_direct_send_follow_all_nine_slots_and_renames(self):
        self.server.scope = AGENT.Scope({**self.config, 'message_targets': 'current_bus'})
        for slot in map(str, range(1, 10)):
            DEMUX.write_channel_binding(self.root, slot, 'Ada' if slot == '2' else 'Peer-' + slot,
                'codex', SESSION if slot == '2' else 'session-' + slot, str(self.bus))
        with patch.object(self.native, 'status', return_value={}):
            roster = self.call('status')['participants']
        self.assertEqual([r['channel'] for r in roster], list(map(str, range(1, 10))))
        schema = next(t for t in self.server.tools() if t['name'] == 'codescribe_send')['inputSchema']
        self.assertNotIn('enum', schema['properties']['to'])
        with patch.object(self.native, 'run', return_value={}) as run:
            for slot in ('1', '3', '4', '5', '6', '7', '8', '9'):
                self.call('send', {'request_id': f'{int(slot):024x}', 'text': 'Hello', 'to': slot})
                self.assertEqual(run.call_args.args[0][-1], 'peer-' + slot)
            # Rename the canonical occupant without editing this server or its scope.
            with DEMUX.channel_bindings(self.root) as rows:
                rows['9']['audience'] = 'NewName'
            self.call('send', {'request_id': 'a' * 24, 'text': 'Hello', 'to': 'NewName'})
            self.assertEqual(run.call_args.args[0][-1], 'newname')
            self.call('send', {'request_id': 'b' * 24, 'text': 'Hello', 'to': '9'})
            self.assertEqual(run.call_args.args[0][-1], 'newname')
            for target in ('peer-9', '2', 'Ada', '0', '10', 'unknown'):
                with self.assertRaises(PermissionError):
                    self.call('send', {'request_id': 'c' * 24, 'text': 'Hello', 'to': target})
        with patch.object(self.native, 'status', return_value={}):
            self.assertEqual(self.call('status')['participants'][-1]['name'], 'NewName')

    def test_send_refuses_ambiguous_names_and_preserves_separate_recipient_policy(self):
        for slot, name in (('2', 'Ada'), ('1', 'Duplicate'), ('9', 'duplicate')):
            DEMUX.write_channel_binding(self.root, slot, name, 'codex',
                SESSION if slot == '2' else 'session-' + slot, str(self.bus))
        with patch.object(self.native, 'run', side_effect=AssertionError('no ambiguous send')):
            for target in ('duplicate', '1', '9'):
                with self.assertRaises(PermissionError):
                    self.call('send', {'request_id': 'a' * 24, 'text': 'Hello', 'to': target})
        with DEMUX.channel_bindings(self.root) as rows:
            rows['9']['audience'] = 'new-peer'
        self.server.scope = AGENT.Scope({**self.config, 'message_targets': ['duplicate']})
        with patch.object(self.native, 'run', side_effect=AssertionError('unapproved send')):
            with self.assertRaises(PermissionError):
                self.call('send', {'request_id': 'a' * 24, 'text': 'Hello', 'to': '9'})

    def test_send_refuses_unapproved_target_and_detached_binding(self):
        DEMUX.write_channel_binding(self.root, "2", "Ada", "codex", SESSION, str(self.bus))
        with patch.object(self.native, "run", side_effect=AssertionError("unexpected send")):
            with self.assertRaises(PermissionError):
                self.call("send", {"request_id": "a" * 24, "text": "Hello", "to": "any-agent"})
            with DEMUX.channel_bindings(self.root) as rows:
                rows['2']['audience'] = 'other'
            with self.assertRaises(PermissionError):
                self.call("send", {"request_id": "a" * 24, "text": "Hello", "to": "0"})

    def test_uncertain_send_never_retries_automatically(self):
        DEMUX.write_channel_binding(self.root, "2", "Ada", "codex", SESSION, str(self.bus))
        self.server.scope.broadcast_enabled = True
        values = {"request_id": "a" * 24, "text": "One message", "to": "0"}
        with patch.object(self.native, "run", side_effect=subprocess.TimeoutExpired("synthetic", 30)) as run:
            first = self.call("send", values)
            second = self.call("send", values)
        self.assertEqual(first, second)
        self.assertEqual(first["state"], "uncertain")
        self.assertEqual(run.call_count, 1)

    def test_read_requires_matching_lease_and_returns_complete_text_without_paths(self):
        text = "message " * 2000
        self.state['pending'][0].update(channel='2', origin_channel='0', audience='*')
        self.state["pending"][0]["text"] = text
        self.save()
        result = self.call("read")
        self.assertEqual(result["deliveries"][0]["text"], text)
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(result["remaining"], 4)
        self.assertEqual(result['deliveries'][0]['addressed_to'], '0 (all audience)')
        self.assertFalse((self.root / "acknowledgments").exists())
        for field in ("bus", "instructions", "attachments", "recipients"):
            self.assertNotIn(field, result["deliveries"][0])
        self.state["name"] = "Lin"
        self.save()
        with self.assertRaises(PermissionError):
            self.call("read")

    def test_breaking_mailbox_api_refuses_before_any_bus_operation(self):
        with patch.object(self.native, 'api_version', 2), patch.object(self.native, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'mailbox API v1 required'):
                AGENT.AgentMailbox(DEMUX, self.native, AGENT.Scope(self.config))
            run.assert_not_called()

    def test_foreign_envelope_and_status_diagnostics_cannot_expand_scope(self):
        self.state["pending"][0]["delivery_owner"]["provider_session_id"] = "foreign"
        self.save()
        with self.assertRaises(ValueError):
            self.call("read")
        with patch.object(self.native, "status", return_value={"kind": "status",
                "follower_log": "/private/log", "voice": {"voice": "leo", "unknown": "/private/value"},
                "listener": {"consecutive_losses": 1, "last_loss": {"path": "/private/log"}}}):
            result = self.call("status")
        self.assertNotIn("/private", json.dumps(result))
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_ack_only_read_exact_ids_and_uses_canonical_receipt(self):
        with self.assertRaises(ValueError):
            self.call("ack", {"delivery_ids": self.ids[:1]})
        self.call("read")
        with self.assertRaises(ValueError):
            self.call("ack", {"delivery_ids": self.ids[1:2]})
        with self.assertRaises(ValueError):
            self.call("ack", {"delivery_ids": [{"injection": True}]})
        self.call("ack", {"delivery_ids": self.ids[:1]})
        receipt = self.root / "acknowledgments" / self.lease / (self.ids[0] + ".json")
        self.assertEqual(json.loads(receipt.read_text())["delivery_id"], self.ids[0])
        self.assertEqual(self.call("read")["read_delivery_ids"], self.ids[1:2])

    def test_call_is_prepared_with_zero_helper_or_state_effects_by_default(self):
        with patch.object(self.native, "run", side_effect=AssertionError("speech attempted")):
            result = self.call("call", {"request_id": "a" * 24})
        self.assertEqual(result["state"], "prepared")
        self.assertFalse(result["speech_attempted"])
        self.assertEqual(result["text"], CALL_TEXT)
        self.assertFalse(self.server.calls.exists())

    def test_opted_in_call_uses_existing_say_receipts_and_dedupes_across_restart(self):
        self.server.scope.speech_enabled = True
        result = {"receipts": [{"kind": "agent_reply", "reply_id": "b" * 24},
            {"kind": "agent_reply_playback", "reply_id": "b" * 24, "state": "spoken",
             "spoken": True, "playback_ticket": "c" * 24, "provider": "codex",
             "provider_session_id": SESSION, "lease_id": self.lease}]}
        with patch.object(self.native, "run", return_value=result) as run:
            first = self.call("call", {"request_id": "a" * 24})
            self.server = AGENT.AgentMailbox(DEMUX, self.native, AGENT.Scope({**self.config, "speech_enabled": True}))
            second = self.call("call", {"request_id": "a" * 24})
        self.assertEqual(first, second)
        self.assertTrue(first["spoken"])
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0], ["--say", CALL_TEXT, "--name", "Ada"])
        self.assertNotIn("text", json.loads((self.server.calls / ("a" * 24 + ".json")).read_text()))

    def test_uncertain_speech_never_replays(self):
        self.server.scope.speech_enabled = True
        with patch.object(self.native, "run", side_effect=subprocess.TimeoutExpired("mock", 90)) as run:
            result = self.call("call", {"request_id": "a" * 24})
            self.call("call", {"request_id": "a" * 24})
        self.assertEqual(result["state"], "uncertain")
        self.assertEqual(run.call_count, 1)

    def test_foreign_or_unassociated_playback_cannot_claim_speech(self):
        self.server.scope.speech_enabled = True
        for n, reply in enumerate(({}, {"kind": "agent_reply", "reply_id": "b" * 24})):
            mocked = {"receipts": [reply, {"kind": "agent_reply_playback", "state": "spoken",
                                          "spoken": True, "reply_id": "b" * 24, "provider_session_id": "foreign"}]}
            with patch.object(self.native, "run", return_value=mocked):
                result = self.call("call", {"request_id": f"{n:024x}"})
            self.assertEqual(result["state"], "uncertain")
            self.assertFalse(result["spoken"])

    def test_stdio_protocol_discovery_and_voice_plan_without_external_effects(self):
        config = self.root / "scope.json"
        config.write_text(json.dumps(self.config))
        requests = [{"jsonrpc": "2.0", "id": 1, "method": "server/discover"},
                    {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                        "_meta": META, "name": "codescribe_call", "arguments": {"request_id": "a" * 24}}}]
        result = subprocess.run([sys.executable, str(SCRIPTS / "bus-mcp-agent.py"), "--scope", str(config)],
                                input="".join(json.dumps(r) + "\n" for r in requests), text=True, capture_output=True, check=True)
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(rows[0]["result"]["supportedVersions"], ["2026-07-28"])
        self.assertNotIn("events", rows[0]["result"]["capabilities"])
        self.assertEqual(rows[1]["result"]["structuredContent"]["state"], "prepared")

    def test_oversized_stdio_frame_stops_before_following_request(self):
        config = self.root / "scope.json"
        config.write_text(json.dumps(self.config))
        result = subprocess.run([sys.executable, str(SCRIPTS / "bus-mcp-agent.py"), "--scope", str(config)],
            input=b"x" * 70000 + b'\n{"jsonrpc":"2.0","id":2,"method":"tools/list"}\n',
            capture_output=True, check=True)
        rows = result.stdout.splitlines()
        self.assertEqual(len(rows), 1)
        self.assertEqual(json.loads(rows[0])["error"]["code"], -32602)


class ProtocolTests(Fixture):
    def test_client_metadata_extensions_are_stripped_without_granting_authority(self):
        self.preflight()
        extra = {f'com.example/context{n}': {'private': SECRET} for n in range(8)}
        extra.update(principal='foreign', callbackHost=URL, speech_enabled=True)
        with patch.object(self.native, 'status', return_value={'attached': True}):
            rows, output = self.wire([self.frame('tools/call', {
                'name': 'codescribe_status', 'arguments': {}, '_meta': {**META, **extra}})])
        self.assertTrue(rows[0]['result']['structuredContent']['attached'])
        self.assertNotIn(SECRET, output)
        self.assertEqual(self.server.scope.principal, PRINCIPAL)
        self.assertFalse(self.server.scope.speech_enabled)
        self.assertFalse(self.server.scope.broadcast_enabled)
        self.assertEqual([t['name'] for t in self.server.tools()], ['codescribe_status'])
        with patch.object(self.server, 'dispatch') as dispatch:
            self.wire([self.frame('tools/list', {'_meta': {**META, 'context': SECRET * 200}})])
            dispatch.assert_not_called()

    def test_standard_optional_client_metadata(self):
        # Official 2026-07-28 server/discover example includes clientInfo.
        self.preflight()
        meta = {**META, 'io.modelcontextprotocol/clientInfo': {
            'name': 'ExampleClient', 'version': '1.0.0'},
            'io.modelcontextprotocol/logLevel': 'warning'}
        requests = [self.frame(method, {'_meta': meta}, n)
                    for n, method in enumerate(('server/discover', 'tools/list', 'events/list'), 1)]
        rows, _ = self.wire(requests)
        self.assertTrue(all('result' in row for row in rows), rows)
        self.assertEqual(rows[0]['result']['supportedVersions'], ['2026-07-28'])
        self.assertEqual([t['name'] for t in rows[1]['result']['tools']], ['codescribe_status'])
        self.assertFalse(self.server.scope.speech_enabled)
        self.assertEqual(self.server.scope.principal, PRINCIPAL)

    def test_malformed_optional_client_metadata(self):
        info_key = 'io.modelcontextprotocol/clientInfo'
        level_key = 'io.modelcontextprotocol/logLevel'
        bad = [{info_key: value} for value in (None, [], {},
               {'name': '', 'version': '1.0.0'}, {'name': 'Client', 'version': 1},
               {'name': 'C' * 201, 'version': '1.0.0'},
               {'name': 'Client', 'version': '1.0.0', 'extra': SECRET * 200})]
        bad += [{level_key: value} for value in (None, [], 'ALL_PRIVATE')]
        for extra in bad:
            with self.subTest(extra=list(extra)), patch.object(self.server, 'dispatch') as dispatch:
                rows, output = self.wire([self.frame('tools/list', {'_meta': {**META, **extra}})])
                self.assertEqual(rows[0]['error']['code'], -32602)
                dispatch.assert_not_called()
                self.assertNotIn(SECRET, output)
                self.assertNotIn('ALL_PRIVATE', output)

    def frame(self, method, params=None, identity=1):
        return {"jsonrpc": "2.0", "id": identity, "method": method,
                "params": {"_meta": dict(META), **(params or {})}}

    def wire(self, requests, *, raw=None):
        payload = raw if raw is not None else ''.join(json.dumps(r) + '\n' for r in requests)
        output, errors = io.StringIO(), io.StringIO()
        input_stream = type('Input', (), {'buffer': io.BytesIO(payload.encode())})()
        with patch.object(AGENT.sys, 'stdin', input_stream), patch.object(AGENT.sys, 'stdout', output), \
                patch.object(AGENT.sys, 'stderr', errors):
            AGENT.serve(self.server, self.server.scope)
        self.last_trace = [json.loads(line) for line in errors.getvalue().splitlines()]
        for trace in self.last_trace:
            self.assertEqual(trace['kind'], 'codescribe_refusal_shape')
        self.assertNotIn(SECRET, errors.getvalue())
        return [json.loads(line) for line in output.getvalue().splitlines()], output.getvalue()

    def test_refusal_trace_reports_shape_without_request_values_or_unknown_keys(self):
        request = self.frame('tools/call', {'name': 'codescribe_status', 'arguments': {}})
        request['params']['_meta']['PRIVATE_KEY_NAME'] = SECRET
        request['params']['_meta']['io.modelcontextprotocol/protocolVersion'] = '2025-11-25'
        rows, _ = self.wire([request])
        self.assertEqual(rows[0]['error']['code'], -32602)
        trace = self.last_trace[0]
        self.assertEqual(trace['stage'], 'metadata')
        self.assertEqual(trace['meta_unknown_count'], 1)
        self.assertTrue(trace['status_call'])
        self.assertTrue(trace['empty_arguments'])
        self.assertNotIn('PRIVATE_KEY_NAME', json.dumps(trace))
        request['params'].pop('_meta')
        self.wire([request])
        self.assertFalse(self.last_trace[0]['meta_object'])
        rows, _ = self.wire([self.frame([], {}), self.frame('tools/list', {}, 2)])
        self.assertEqual(rows[0]['error']['code'], -32602)
        self.assertIn('result', rows[1])

    def preflight(self):
        self.events.close()
        path = self.root / 'unused-preflight-directory/events.sqlite'
        with patch.object(EVENTS.sqlite3, 'connect', side_effect=AssertionError('no metadata backend')):
            events = EVENTS.Events(path, PRINCIPAL, SESSION, callback_preflight=True,
                                   authorized=lambda: self.permitted[0])
        self.addCleanup(events.close)
        self.server.events = events
        self.assertFalse(path.parent.exists())
        return events

    def test_v0016_self_contained_stdio_tools_read_ack_and_events_lifecycle(self):
        self.server.events = self.events
        methods = [('server/discover', {}), ('tools/list', {}), ('events/list', {}),
                   ('tools/call', {'name': 'codescribe_read', 'arguments': {}}),
                   ('tools/call', {'name': 'codescribe_ack', 'arguments': {'delivery_ids': self.ids[:1]}}),
                   ('events/subscribe', self.subscription()),
                   ('events/unsubscribe', {'name': EVENTS.EVENT_NAME, 'arguments': {},
                                          'delivery': {'mode': 'webhook', 'url': URL}}),
                   ('tools/call', {'name': 'codescribe_call', 'arguments': {'request_id': 'a' * 24}})]
        rows, output = self.wire([self.frame(m, p, n) for n, (m, p) in enumerate(methods, 1)])
        self.assertTrue(all('result' in row for row in rows), rows)
        self.assertEqual(rows[3]['result']['structuredContent']['read_delivery_ids'], self.ids[:1])
        self.assertEqual(rows[-1]['result']['structuredContent']['state'], 'prepared')
        self.assertEqual(self.events.db.execute('SELECT COUNT(*) FROM subscriptions').fetchone()[0], 0)
        self.assertFalse(self.secret_store.values)
        self.assertEqual(len(self.posts), 1)
        self.assertNotIn(SECRET, output)

    def test_invalid_metadata_refused_before_dispatch_and_without_echo(self):
        self.server.events = self.events
        version, capabilities = META.keys()
        variants = [None, [], {}, {**META, version: '2025-11-25'}, {**META, version: '2027-01-01'},
                    {**META, version: SECRET}, {**META, capabilities: None},
                    {**META, capabilities: []}, {**META, capabilities: {'sampling': True}},
                    {**META, capabilities: {'sampling': {'nan': float('nan')}}},
                    {**META, capabilities: {'sampling': {'text': SECRET * 200}}},
                    {**META, 'x' * 201: SECRET}, {**META, 'control\nkey': SECRET},
                    {**META, **{f'com.example/{n}': None for n in range(65)}}]
        for meta in variants:
            with self.subTest(meta=type(meta).__name__), patch.object(self.server, 'dispatch') as dispatch:
                rows, output = self.wire([self.frame('tools/list', {'_meta': meta})])
                self.assertEqual(rows[0]['error']['code'], -32602)
                dispatch.assert_not_called()
                self.assertNotIn(SECRET, output)
                self.assertNotIn(URL, output)
        for params in ({}, [], None):
            rows, _ = self.wire([{'jsonrpc': '2.0', 'id': 1, 'method': 'tools/list', 'params': params}])
            self.assertEqual(rows[0]['error']['code'], -32602)

    def test_duplicate_json_metadata_and_argument_members_are_refused(self):
        raw = json.dumps(self.frame('tools/call', {'name': 'codescribe_call',
                         'arguments': {'request_id': 'a' * 24}}))
        for payload in (raw.replace('"_meta": {', '"_meta": null, "_meta": {'),
                        raw.replace('"2026-07-28"', '"2025-11-25", "io.modelcontextprotocol/protocolVersion": "2026-07-28"'),
                        raw.replace('"request_id":', '"request_id": "secret", "request_id":')):
            with patch.object(self.server, 'dispatch') as dispatch:
                rows, output = self.wire([], raw=payload + '\n')
                self.assertEqual(rows[0]['error']['code'], -32602)
                dispatch.assert_not_called()
                self.assertNotIn('secret', output)

    def test_metadata_does_not_relax_business_validation_or_enable_speech(self):
        self.server.events = self.events
        bad = [('server/discover', {'extra': SECRET}), ('tools/list', {'cursor': SECRET}),
               ('tools/call', {'name': 'codescribe_call', 'arguments': {'request_id': 'a' * 24, '_meta': META}}),
               ('tools/call', {'name': 'codescribe_call', 'arguments': {'request_id': 'b' * 24}, 'provider': 'foreign'}),
               ('events/subscribe', {**self.subscription(), 'extra': SECRET})]
        for method, params in bad:
            rows, output = self.wire([self.frame(method, params)])
            self.assertEqual(rows[0]['error']['code'], -32602)
            self.assertNotIn(SECRET, output)
        request = self.frame('tools/call', {'name': 'codescribe_call', 'arguments': {'request_id': 'c' * 24}})
        request['params']['_meta'] = {**META, 'io.modelcontextprotocol/clientCapabilities': {
            'experimental': {'speech_enabled': True}}}
        with patch.object(self.native, 'run') as run:
            rows, _ = self.wire([request])
            self.assertEqual(rows[0]['result']['structuredContent']['state'], 'prepared')
            run.assert_not_called()

    def test_preflight_returns_only_host_without_files_keys_network_or_activation(self):
        events = self.preflight()
        params = self.subscription()
        params['delivery']['url'] = 'https://CallBack.Example.test/URL_PATH_PRIVATE?token=QUERY_PRIVATE'
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*'))
        rows, output = self.wire([self.frame('events/subscribe', params)])
        self.assertEqual(rows[0]['error']['data'], {'reason': 'host_unpinned', 'callbackHost': 'callback.example.test'})
        self.assertEqual(rows[0]['error']['code'], -32015)
        for private in ('URL_PATH_PRIVATE', 'QUERY_PRIVATE', SECRET, 'https://'):
            self.assertNotIn(private, output)
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*')))
        self.assertIsNone(events.db)
        self.assertIsNone(events.store)
        self.assertFalse(events.hosts)
        self.assertFalse(events.ready(PRINCIPAL, self.ids[0]))
        self.assertEqual(events.deliver(PRINCIPAL, self.ids[0], TIME), [])
        self.assertEqual(self.posts, [])
        self.assertEqual(self.alarms, [])
        events.close()
        self.events = self.new_events()
        self.server.events = self.events
        rows, _ = self.wire([self.frame('events/subscribe', params)])
        self.assertIn('id', rows[0]['result'])
        self.assertEqual(len(self.posts), 1)
        self.assertEqual(self.events.db.execute('SELECT active FROM subscriptions').fetchone()[0], 1)

    def test_preflight_rejects_bad_shapes_urls_and_revoked_owner_without_host(self):
        self.preflight()
        invalid = []
        for url in ('http://callback.example.test/PATH_PRIVATE', 'https://127.0.0.1/PATH_PRIVATE',
                    'https://[::1]/PATH_PRIVATE', 'https://user:SECRET_PRIVATE@callback.example.test/PATH_PRIVATE',
                    'https://callback.example.test:8443/PATH_PRIVATE', 'https://callback.example.test/x#FRAGMENT_PRIVATE',
                    'https://callback.example.test/\nCONTROL_PRIVATE', 'https://secret_private.example.test/x',
                    'https://callback.example.test./x'):
            p = self.subscription(); p['delivery']['url'] = url; invalid.append(p)
        for change in ({'ttlMs': True}, {'arguments': {'other': SECRET}}, {'extra': SECRET}):
            invalid.append(self.subscription(**change))
        p = self.subscription(); p['delivery']['secret'] = 'SECRET_PRIVATE'; invalid.append(p)
        for params in invalid:
            rows, output = self.wire([self.frame('events/subscribe', params)])
            self.assertNotIn('callbackHost', rows[0]['error'].get('data', {}))
            for private in ('PATH_PRIVATE', 'SECRET_PRIVATE', 'FRAGMENT_PRIVATE', 'CONTROL_PRIVATE', SECRET):
                self.assertNotIn(private, output)
        self.permitted[0] = False
        rows, output = self.wire([self.frame('events/subscribe', self.subscription())])
        self.assertEqual(rows[0]['error']['code'], -32602)
        self.assertNotIn('callbackHost', output)

    def test_preflight_blocks_read_ack_send_speech_and_malformed_unsubscribe(self):
        self.preflight()
        rows, _ = self.wire([self.frame('tools/list')])
        self.assertEqual([t['name'] for t in rows[0]['result']['tools']], ['codescribe_status'])
        with patch.object(self.native, 'run') as run:
            for name in ('read', 'ack', 'send', 'say', 'call'):
                rows, _ = self.wire([self.frame('tools/call', {'name': 'codescribe_' + name, 'arguments': {}})])
                self.assertEqual(rows[0]['error']['code'], -32602)
            rows, _ = self.wire([self.frame('events/unsubscribe', {})])
            self.assertEqual(rows[0]['error']['code'], -32602)
            run.assert_not_called()

    def test_preflight_unsubscribe_is_authorized_validated_idempotent_and_inert(self):
        events = self.preflight()
        params = {'name': EVENTS.EVENT_NAME, 'arguments': {},
                  'delivery': {'mode': 'webhook', 'url': URL + '?QUERY_PRIVATE'}}
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*'))
        with patch.object(self.native, 'run', side_effect=AssertionError('no helper')):
            rows, output = self.wire([self.frame('events/unsubscribe', params, i) for i in (1, 2)])
        self.assertEqual([row['result'] for row in rows], [{}, {}])
        self.assertNotIn('QUERY_PRIVATE', output)
        self.assertEqual(self.last_trace, [])
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*')))
        self.assertIsNone(events.db)
        self.assertIsNone(events.store)
        self.assertEqual((self.posts, self.alarms), ([], []))
        with self.assertRaises(PermissionError):
            self.server.dispatch('foreign', 'events/unsubscribe', params)
        self.permitted[0] = False
        rows, _ = self.wire([self.frame('events/unsubscribe', params)])
        self.assertEqual(rows[0]['error']['code'], -32602)
        self.permitted[0] = True
        for change, code in (({'delivery': {'mode': 'webhook', 'url': 'http://callback.example.test/private'}}, -32015),
                             ({'arguments': {'private': SECRET}}, -32602), ({'extra': SECRET}, -32602)):
            rows, output = self.wire([self.frame('events/unsubscribe', {**params, **change})])
            self.assertEqual(rows[0]['error']['code'], code)
            self.assertNotIn(SECRET, output)

    def test_event_refusal_codes_are_closed_and_never_include_supplied_values(self):
        self.preflight()
        variants = [({'arguments': {'PRIVATE_KEY': SECRET}}, 'event_arguments_invalid'),
                    ({'cursor': 'CURSOR_PRIVATE'}, 'replay_cursor_unsupported'),
                    ({'extra': SECRET}, 'subscription_parameters_invalid'),
                    ({'ttlMs': True}, 'ttl_invalid')]
        p = self.subscription(); p['delivery']['secret'] = 'SIGNING_PRIVATE'
        variants.append((p, 'signing_secret_invalid'))
        p = self.subscription(); p['delivery']['extra'] = SECRET
        variants.append((p, 'webhook_delivery_invalid'))
        for change, reason in variants:
            rows, output = self.wire([self.frame('events/subscribe', self.subscription(**change))])
            self.assertEqual(rows[0]['error']['data'], {'reason': reason})
            self.assertEqual(self.last_trace[0]['reason'], reason)
            for private in ('PRIVATE_KEY', 'CURSOR_PRIVATE', 'SIGNING_PRIVATE', SECRET, URL):
                self.assertNotIn(private, output + json.dumps(self.last_trace))
        error = EVENTS.EventRequestError('ttl_invalid')
        error.reason = 'SECRET_PRIVATE'
        with patch.object(self.server.events, 'subscribe', side_effect=error):
            rows, output = self.wire([self.frame('events/subscribe', self.subscription())])
        self.assertEqual(self.last_trace[0]['reason'], 'request_refused')
        self.assertNotIn('SECRET_PRIVATE', output + json.dumps(self.last_trace))

    def test_pinned_mode_unknown_host_stays_refused_without_candidate_disclosure(self):
        self.server.events = self.events
        params = self.subscription(); params['delivery']['url'] = 'https://other.example.test/PATH_PRIVATE'
        rows, output = self.wire([self.frame('events/subscribe', params)])
        self.assertEqual(rows[0]['error']['data'], {'reason': 'destination_refused'})
        self.assertNotIn('other.example.test', output)
        self.assertFalse(self.secret_store.values)
        self.assertEqual(self.posts, [])

    def test_forged_or_malformed_preflight_errors_do_not_escape(self):
        self.preflight()
        for error in (EVENTS.CallbackError('host_unpinned', callback_host='https://user:SECRET_PRIVATE@host.test/path'),
                      EVENTS.CallbackError('SECRET_PRIVATE'), ValueError('URL_PATH_PRIVATE')):
            with patch.object(self.server.events, 'subscribe', side_effect=error):
                rows, output = self.wire([self.frame('events/subscribe', self.subscription())])
                self.assertEqual(rows[0]['error']['code'], -32602)
                self.assertNotIn('SECRET_PRIVATE', output)
                self.assertNotIn('URL_PATH_PRIVATE', output)

    def test_cli_preflight_needs_no_native_binding_and_eof_leaves_no_metadata(self):
        scope = self.root / 'scope.json'; scope.write_text(json.dumps(self.config))
        requests = [self.frame('server/discover'), self.frame('events/list', identity=2),
                    self.frame('events/subscribe', self.subscription(), 3)]
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*'))
        result = subprocess.run([sys.executable, str(RUNTIME_SCRIPT),
            '--scope', str(scope), '--callback-preflight'], input=''.join(json.dumps(r) + '\n' for r in requests),
            text=True, capture_output=True, check=True, timeout=15)
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertIn('events', rows[0]['result']['capabilities'])
        self.assertEqual(rows[2]['error']['data']['callbackHost'], 'callback.example.test')
        trace = json.loads(result.stderr)
        self.assertEqual(trace['kind'], 'codescribe_refusal_shape')
        self.assertEqual(trace['stage'], 'dispatch')
        self.assertNotIn(SECRET, result.stderr)
        self.assertNotIn(SECRET, result.stdout)
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob('*')))


class EventsTests(Fixture):
    def test_verified_idempotent_subscription_signed_and_private(self):
        first = self.subscribe()
        self.assertEqual(first["id"], self.subscribe()["id"])
        self.assertEqual(len(self.posts), 1)
        self.assert_signature(self.posts[0][2], self.posts[0][3])
        self.assertEqual((self.root / "synthetic-events.sqlite").stat().st_mode & 0o777, 0o600)
        self.assertNotIn(SECRET, json.dumps(first))

    def test_five_identical_messages_have_five_ids_no_text_no_ack_and_one_delivery_each(self):
        self.subscribe()
        self.server.events = self.events
        receipts = self.server.pump_events(PRINCIPAL)
        self.assertEqual(len(receipts), 5)
        posts = self.posts[1:]
        self.assertEqual(len({json.loads(p[2])["eventId"] for p in posts}), 5)
        for _url, addresses, body, headers in posts:
            self.assertEqual(addresses, ("1.1.1.1",))
            self.assert_signature(body, headers)
            self.assertEqual(headers["webhook-id"], json.loads(body)["eventId"])
            self.assertNotIn(MESSAGE, body.decode())
            self.assertEqual(set(json.loads(body)["data"]), {"delivery_id", "provider_session_id"})
        self.assertEqual(self.server.pump_events(PRINCIPAL), [])
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_retry_preserves_body_id_with_fresh_signature_across_restart(self):
        self.subscribe()
        self.statuses = [503, 202]
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[0], TIME)[0]["state"], "retry")
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[0], TIME), [])
        self.now[0] += 3
        self.events.close()
        restarted = self.new_events()
        self.assertEqual(restarted.deliver(PRINCIPAL, self.ids[0], TIME)[0]["state"], "accepted")
        self.assertEqual(self.posts[-1][2], self.posts[-2][2])
        self.assertNotEqual(self.posts[-1][3]["webhook-signature"], self.posts[-2][3]["webhook-signature"])

    def test_bounded_retries_and_410_413_never_retry(self):
        self.subscribe()
        self.statuses = [503, 503, 503]
        for i in range(3):
            result = self.events.deliver(PRINCIPAL, self.ids[0], TIME)[0]
            self.assertEqual(result["attempt"], i + 1)
            self.now[0] += 20
        self.assertEqual(result["state"], "failed")
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[0], TIME), [])
        self.statuses = [413]
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[1], TIME)[0]["state"], "failed")
        self.now[0] += 20
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[1], TIME), [])
        self.statuses = [410]
        self.events.deliver(PRINCIPAL, self.ids[2], TIME)
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[3], TIME), [])
        self.assertEqual(self.events.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0], 0)
        self.assertNotIn(SECRET.encode(), (self.root / "synthetic-events.sqlite").read_bytes())

    def test_unsubscribe_owner_check_idempotence_and_pending_retry_cancellation(self):
        self.subscribe()
        self.statuses = [503]
        self.events.deliver(PRINCIPAL, self.ids[0], TIME)
        params = self.subscription()
        params.pop("cursor")
        params["delivery"].pop("secret")
        with self.assertRaises(PermissionError):
            self.events.unsubscribe("someone-else", params)
        self.assertEqual(self.events.unsubscribe(PRINCIPAL, params), {})
        self.assertEqual(self.events.unsubscribe(PRINCIPAL, params), {})
        self.now[0] += 20
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[0], TIME), [])
        self.assertEqual(self.events.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0], 0)

    def test_expiration_and_revocation_stop_delivery(self):
        self.subscribe(ttlMs=1000)
        self.now[0] += 2
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[0], TIME), [])
        self.subscribe()
        self.permitted[0] = False
        with self.assertRaises(PermissionError):
            self.events.deliver(PRINCIPAL, self.ids[0], TIME)
        self.assertEqual(self.events.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0], 0)
        self.assertNotIn(SECRET.encode(), (self.root / "synthetic-events.sqlite").read_bytes())

    def test_signing_rotation_reverifies_and_uses_two_signatures_temporarily(self):
        first = self.subscribe()
        second_secret = "whsec_" + base64.b64encode(b"different-synthetic-key-00000000").decode()
        params = self.subscription()
        params["delivery"]["secret"] = second_secret
        second = self.events.subscribe(PRINCIPAL, params)
        self.assertEqual(first["id"], second["id"])
        self.assertEqual(len(self.posts), 2)
        self.events.deliver(PRINCIPAL, self.ids[0], TIME)
        body, headers = self.posts[-1][2:]
        self.assert_signature(body, headers)
        self.assert_signature(body, headers, second_secret)
        self.now[0] += 61
        self.events.deliver(PRINCIPAL, self.ids[1], TIME)
        self.assertEqual(len(self.posts[-1][3]["webhook-signature"].split()), 1)
        row = self.events.db.execute("SELECT * FROM subscriptions").fetchone()
        self.assertIsNone(row["old_key_ref"])
        self.assertEqual(self.events.key(row["current_key_ref"]), second_secret)
        self.assertNotIn(SECRET.encode(), (self.root / "synthetic-events.sqlite").read_bytes())

    def test_idle_timer_erases_expired_keys_and_retry_metadata_without_callback(self):
        self.subscribe(ttlMs=1000)
        self.statuses = [503]
        self.events.deliver(PRINCIPAL, self.ids[0], TIME)
        before = len(self.posts)
        timer = self.events.timer
        self.now[0] += 2
        timer.fire()
        self.assertEqual(len(self.posts), before)
        self.assertEqual(self.events.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0], 0)
        self.assertEqual(self.events.db.execute("SELECT COUNT(*) FROM deliveries").fetchone()[0], 0)
        self.assertIsNone(self.events.timer)
        self.assertNotIn(SECRET.encode(), (self.root / "synthetic-events.sqlite").read_bytes())

    def test_real_cleanup_timer_erases_synthetic_key_without_protocol_activity(self):
        self.events.close()
        live = self.new_events(clock=time.time, timer_factory=threading.Timer)
        live.subscribe(PRINCIPAL, self.subscription(ttlMs=1000))
        deadline = time.monotonic() + 3
        remaining = 1
        while time.monotonic() < deadline:
            with live.lock:
                remaining = live.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0]
            if not remaining:
                break
            time.sleep(0.02)
        self.assertEqual(remaining, 0)
        self.assertNotIn(SECRET.encode(), (self.root / "synthetic-events.sqlite").read_bytes())

    def test_idle_timer_erases_old_key_without_extending_rotation_on_refresh(self):
        self.subscribe()
        newer = "whsec_" + base64.b64encode(b"different-synthetic-key-00000000").decode()
        params = self.subscription()
        params["delivery"]["secret"] = newer
        self.events.subscribe(PRINCIPAL, params)
        deadline = self.events.db.execute("SELECT rotation_until FROM subscriptions").fetchone()[0]
        self.now[0] += 20
        self.events.subscribe(PRINCIPAL, params)
        self.assertEqual(self.events.db.execute("SELECT rotation_until FROM subscriptions").fetchone()[0], deadline)
        self.now[0] += 41
        self.events.timer.fire()
        row = self.events.db.execute("SELECT * FROM subscriptions").fetchone()
        self.assertIsNone(row["old_key_ref"])
        self.assertEqual(self.events.key(row["current_key_ref"]), newer)

    def test_restart_purges_expired_keys_before_protocol_use_and_close_cancels_timer(self):
        self.subscribe(ttlMs=1000)
        timer = self.events.timer
        self.events.close()
        self.assertTrue(timer.cancelled)
        self.now[0] += 2
        restarted = self.new_events()
        self.assertEqual(restarted.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0], 0)
        self.assertNotIn(SECRET.encode(), (self.root / "synthetic-events.sqlite").read_bytes())

    def test_foreign_principal_cannot_erase_live_signing_material(self):
        self.subscribe()
        with self.assertRaises(PermissionError):
            self.events.maintenance("someone-else")
        self.assertEqual(self.events.key(self.events.db.execute("SELECT current_key_ref FROM subscriptions").fetchone()[0]), SECRET)

    def test_unconfigured_transport_and_failed_challenge_do_not_store_subscription(self):
        self.events.close()
        isolated = self.new_events(callback=None)
        with self.assertRaises(EVENTS.CallbackError) as failure:
            isolated.subscribe(PRINCIPAL, self.subscription())
        self.assertEqual(failure.exception.reason, "transport_unconfigured")
        isolated.callback = lambda *args: (200, {"challenge": "wrong"})
        with self.assertRaises(EVENTS.CallbackError):
            isolated.subscribe(PRINCIPAL, self.subscription())
        self.assertEqual(isolated.db.execute("SELECT COUNT(*) FROM subscriptions").fetchone()[0], 0)

    def test_destination_revalidated_before_delivery_and_no_redirects_accepted(self):
        self.subscribe()
        self.events.resolve = lambda host: ["127.0.0.1"]
        count = len(self.posts)
        self.events.deliver(PRINCIPAL, self.ids[0], TIME)
        self.assertEqual(len(self.posts), count)
        self.events.resolve = lambda host: ["1.1.1.1"]
        self.statuses = [302]
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[1], TIME)[0]["state"], "failed")

    def test_foreign_filters_private_callbacks_and_invalid_secrets_refused(self):
        for change in ({"arguments": {"session_id": "other"}}, {"cursor": "older"}, {"ttlMs": True}):
            with self.assertRaises(ValueError):
                self.subscribe(**change)
        for url in ("http://callback.example.test/agent", "https://127.0.0.1/x", "https://callback.example.test:8443/x",
                    "https://name:password@callback.example.test/x", "https://callback.example.test/x#fragment"):
            params = self.subscription()
            params["delivery"]["url"] = url
            with self.assertRaises(EVENTS.CallbackError):
                self.events.subscribe(PRINCIPAL, params)
        for secret in ("not-a-key", "whsec_???", "whsec_" + base64.b64encode(b"short").decode()):
            params = self.subscription()
            params["delivery"]["secret"] = secret
            with self.assertRaises(ValueError):
                self.events.subscribe(PRINCIPAL, params)

    def test_mcp2_events_discovery_catalog_and_unsubscribe_dispatch(self):
        self.server.events = self.events
        self.assertIn("events", self.server.dispatch(PRINCIPAL, "server/discover", {})["capabilities"])
        event = self.server.dispatch(PRINCIPAL, "events/list", {})["events"][0]
        self.assertEqual(event["delivery"], ["webhook"])
        self.server.dispatch(PRINCIPAL, "events/subscribe", self.subscription())
        params = self.subscription()
        params.pop("cursor")
        params["delivery"].pop("secret")
        self.assertEqual(self.server.dispatch(PRINCIPAL, "events/unsubscribe", params), {})

    def test_receiver_rejects_tampered_signature_without_retry(self):
        self.subscribe()
        original = self.callback

        def verify(url, addresses, body, headers):
            try:
                self.assert_signature(body, headers)
            except AssertionError:
                return 403, {}
            return original(url, addresses, body, headers)

        def tamper(url, addresses, body, headers):
            return verify(url, addresses, body + b" ", headers)

        self.events.callback = tamper
        result = self.events.deliver(PRINCIPAL, self.ids[0], TIME)[0]
        self.assertEqual(result["status"], 403)
        self.assertEqual(result["state"], "failed")
        self.now[0] += 20
        self.assertEqual(self.events.deliver(PRINCIPAL, self.ids[0], TIME), [])


if __name__ == "__main__":
    unittest.main()
