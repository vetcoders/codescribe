"""Bounded HTTPS callback transport and one own-mailbox notification pump.

No inbound listener, new follower, credential store or agent relay.
Production construction requires an admitted Codescribe secure-store binding.
"""
import concurrent.futures
import http.client
import io
import ipaddress
import json
import socket
import ssl
import threading
import time
from urllib.parse import urlsplit


class DeadlineReader(io.RawIOBase):
    def __init__(self, sock, deadline, clock, release=None):
        self.sock, self.deadline, self.clock = sock, deadline, clock
        self.release = release

    def close(self):
        if not self.closed:
            try:
                super().close()
            finally:
                if self.release is not None:
                    self.release()

    def readable(self):
        return True

    def readinto(self, buffer):
        remaining = self.deadline - self.clock()
        if remaining <= 0:
            raise TimeoutError("Callback deadline expired")
        self.sock.settimeout(remaining)
        return self.sock.recv_into(buffer)


class DeadlineSocket:
    def __init__(self, sock, deadline, clock):
        self.sock, self.deadline, self.clock = sock, deadline, clock
        self.readers, self.closing, self.closed = 0, False, False

    def release_reader(self):
        self.readers -= 1
        if self.closing and self.readers == 0:
            self.finish_close()

    def finish_close(self):
        if not self.closed:
            self.closed = True
            self.sock.close()

    def makefile(self, mode):
        if mode != "rb":
            raise ValueError("Only bounded response reads are supported")
        if self.closing:
            raise OSError("Callback connection already closed")
        self.readers += 1
        return io.BufferedReader(DeadlineReader(self.sock, self.deadline, self.clock, self.release_reader))

    def sendall(self, data):
        remaining = self.deadline - self.clock()
        if remaining <= 0:
            raise TimeoutError("Callback deadline expired")
        self.sock.settimeout(remaining)
        return self.sock.sendall(data)

    def close(self):
        # HTTPConnection may release its ownership immediately after headers.
        # HTTPResponse's reader retains the socket until the body is consumed.
        self.closing = True
        if self.readers == 0:
            self.finish_close()


class PinnedHTTPSConnection(http.client.HTTPSConnection):
    def __init__(self, host, address, deadline, *, clock=time.monotonic, context=None,
                 socket_factory=socket.socket):
        super().__init__(host, 443, timeout=10, context=context or ssl.create_default_context())
        self.address, self.deadline, self.clock = address, deadline, clock
        self.socket_factory = socket_factory

    def connect(self):
        # Never ask DNS again when connecting. TLS still verifies the URL host.
        remaining = self.deadline - self.clock()
        if remaining <= 0:
            raise TimeoutError("Callback deadline expired")
        family = socket.AF_INET6 if ipaddress.ip_address(self.address).version == 6 else socket.AF_INET
        raw = self.socket_factory(family, socket.SOCK_STREAM)
        try:
            raw.settimeout(remaining)
            raw.connect((self.address, 443))
            remaining = self.deadline - self.clock()
            if remaining <= 0:
                raise TimeoutError("Callback deadline expired")
            raw.settimeout(remaining)
            secure = self._context.wrap_socket(raw, server_hostname=self.host)
            self.sock = DeadlineSocket(secure, self.deadline, self.clock)
        except Exception:
            raw.close()
            raise


class HTTPSCallback:
    """DNS + TLS + response share one ten-second monotonic deadline.

    At most one daemon resolver can remain pending after a DNS timeout. Later
    calls refuse while it remains pending; no unbounded resolver thread pool.
    No redirect, proxy environment lookup, cookies or authentication headers.
    """
    def __init__(self, *, clock=time.monotonic, lookup=socket.getaddrinfo,
                 connection_factory=PinnedHTTPSConnection):
        self.clock, self.lookup, self.connection_factory = clock, lookup, connection_factory
        self.local, self.lock, self.pending = threading.local(), threading.Lock(), None

    def resolve(self, host):
        self.local.deadline = self.clock() + 10
        with self.lock:
            if self.pending is not None and not self.pending.done():
                raise TimeoutError("Callback DNS still pending")
            future = concurrent.futures.Future()
            self.pending = future
            def lookup():
                try:
                    future.set_result(self.lookup(host, 443, type=socket.SOCK_STREAM))
                except Exception:
                    future.set_exception(OSError("Callback DNS refused"))
            threading.Thread(target=lookup, daemon=True).start()
        try:
            records = future.result(timeout=max(0.001, self.local.deadline - self.clock()))
        except concurrent.futures.TimeoutError:
            raise TimeoutError("Callback DNS deadline expired") from None
        addresses = tuple(dict.fromkeys(record[4][0] for record in records))
        if not addresses or any(not ipaddress.ip_address(a).is_global for a in addresses):
            raise ValueError("Callback address refused")
        return addresses

    def __call__(self, url, addresses, body, headers):
        parts = urlsplit(url)
        if (parts.scheme != "https" or parts.port not in (None, 443) or not parts.hostname
                or parts.username or parts.password or parts.fragment
                or not addresses or any(not ipaddress.ip_address(a).is_global for a in addresses)):
            raise ValueError("Callback destination refused")
        deadline = getattr(self.local, "deadline", self.clock() + 10)
        if deadline <= self.clock():
            raise TimeoutError("Callback deadline expired")
        # One pinned address per attempt. Events owns all bounded retries.
        connection = self.connection_factory(parts.hostname, addresses[0], deadline, clock=self.clock)
        response = None
        try:
            connection.request("POST", (parts.path or "/") + ("?" + parts.query if parts.query else ""),
                               body=body, headers=headers)
            response = connection.getresponse()
            data = response.read(65537)
            if len(data) > 65536:
                raise ValueError("Callback response exceeds bound")
            # Redirects are returned to Events as permanent failures.
            if not 200 <= response.status < 300:
                return response.status, {}
            try:
                value = json.loads(data) if data else {}
            except ValueError:
                value = {}
            return response.status, value if isinstance(value, dict) else {}
        finally:
            if response is not None:
                response.close()
            connection.close()


class NotificationPump:
    """Observe existing owned pending IDs; never read/ACK or create a follower."""
    def __init__(self, server, *, interval=1, batch=8):
        if not 0.1 <= interval <= 60 or not 1 <= batch <= 32:
            raise ValueError("Bounded notification schedule required")
        self.server, self.interval, self.batch = server, interval, batch
        self.stop_event, self.thread = threading.Event(), None
        self.state = "stopped"

    def tick(self):
        if self.stop_event.is_set() or self.server.events.callback_preflight:
            return []
        with self.server.lock:
            self.server.events.maintenance(self.server.scope.principal)
            rows = self.server.owned_pending()
        receipts, notified = [], 0
        for row in rows:
            if self.stop_event.is_set() or notified >= self.batch:
                break
            if self.server.events.ready(self.server.scope.principal, row["delivery_id"]):
                receipts.extend(self.server.events.deliver(self.server.scope.principal,
                                                          row["delivery_id"], row["emitted_at"]))
                notified += 1
        return receipts

    def start(self):
        if self.thread is not None or self.stop_event.is_set() or self.state == "preflight":
            raise RuntimeError("Only one notification pump per connection")
        if self.server.events.callback_preflight:
            self.state = "preflight"
            return
        def run():
            self.state = "running"
            # Reuse the exact lease-file trigger used by --watch --until-event.
            source = self.server.scope.root / "leases" / (self.server.lease + ".json")
            trigger = None
            try:
                trigger = self.server.demux.BusEventTrigger(source, self.interval)
                while not self.stop_event.is_set():
                    try:
                        self.tick()
                        self.state = "running"
                    except Exception:
                        self.state = "paused"  # No secrets, callback URLs or backend errors logged.
                    trigger.wait(timeout=self.interval)
            finally:
                if trigger is not None:
                    trigger.close()
                self.state = "stopped"
                self.server.events.close()
        self.thread = threading.Thread(target=run, daemon=True)
        self.thread.start()

    def stop(self):
        self.stop_event.set()
        if self.thread:
            self.thread.join(timeout=1)
        else:
            self.server.events.close()
        return self.thread is None or not self.thread.is_alive()


def configure(server, events_module, secret_store, *, callback_hosts, transport=None,
              synthetic=False, timer_factory=threading.Timer, clock=time.time, callback_preflight=False):
    """Explicit admission; no automatic credentials acquisition or plaintext mode.

    An admitted backend must implement put/get/delete/references in the existing
    Codescribe storage authority. MemorySecretStore is only admitted for tests.
    No production backend is installed by this module.
    """
    if (type(callback_preflight) is not bool or not server.scope.mailbox_enabled
            or not callback_preflight and not callback_hosts
            or callback_preflight and (callback_hosts or secret_store is not None or transport is not None)):
        raise ValueError("Reviewed own-mailbox callback host policy required")
    if getattr(secret_store, "synthetic_only", False) and not synthetic:
        raise ValueError("Synthetic signing-key backend cannot enable live Events")
    if not callback_preflight and not synthetic and not getattr(secret_store, "codescribe_authority", False):
        raise ValueError("Admitted Codescribe signing-key backend required")
    transport = None if callback_preflight else transport or HTTPSCallback()
    def authorized():
        try:
            server.owned_pending()
            return True
        except Exception as error:
            # Preflight cannot retain subscriptions. Surface only canonical
            # admission codes there; normal revocation still retires state.
            if callback_preflight and isinstance(error, server.demux.MailboxReadError):
                raise events_module.EventRequestError(error.reason) from None
            return False
    server.events = events_module.Events(server.calls.parent / "events.sqlite",
        server.scope.principal, server.scope.session, secret_store=secret_store,
        callback=transport, resolve=None if callback_preflight else transport.resolve, callback_hosts=callback_hosts,
        authorized=authorized, timer_factory=timer_factory, clock=clock, callback_preflight=callback_preflight)
    return NotificationPump(server)


def load_components():
    """The agent surface and the Events metadata component, by file name."""
    from pathlib import Path
    import importlib.util
    import sys
    scripts = Path(__file__).resolve().parent
    def load(name, filename):
        spec = importlib.util.spec_from_file_location(name, scripts / filename)
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
        return module
    return load("codescribe_runtime_agent", "bus-mcp-agent.py"), load("codescribe_runtime_events", "bus-mcp-events.py")


SCOPE_CHECK_SCHEMA = "codescribe.agent-mcp.scope-check.v1"


def check_scope(scope_path):
    """Print the resolved identity of one reviewed scope and exit.

    An operator compares an old and a new configuration with this before
    switching. It starts no server, reads no mailbox, builds no signing-key
    store and contacts no network. The namespace is a hash, not a secret.
    """
    from pathlib import Path
    import sys
    sys.dont_write_bytecode = True  # Importing the components must not write next to them.
    agent, events = load_components()
    scope = agent.Scope(json.loads(Path(scope_path).read_text()))
    demux = agent.load_module("codescribe_scope_check_demux",
                              Path(agent.__file__).resolve().with_name("bus-demux.py"))
    lease = demux.lease_identifier(scope.provider, scope.session)
    state = scope.state / lease
    metadata = state / "events.sqlite"
    print(json.dumps({"schema": SCOPE_CHECK_SCHEMA, "provider": scope.provider, "name": scope.name,
                      "lease_id": lease, "state_dir_exists": scope.check_private_directory(scope.state),
                      "state_path": str(state.resolve()),
                      "events_path": str(metadata.resolve()),
                      "key_namespace": events.key_namespace(scope.principal, scope.session, metadata)}))
    return 0


def run(scope_path, *, callback_hosts=(), secret_store=None, secret_store_factory=None, callback_preflight=False):
    """Local embedding entrypoint; the owner supplies its admitted store binding.

    No backend name, credential or callback policy can be selected by an MCP
    argument. The standalone agent CLI deliberately remains tools-only.
    """
    agent, events = load_components()
    if (callback_preflight and (secret_store is not None or secret_store_factory is not None)
            or not callback_preflight and (secret_store is None) == (secret_store_factory is None)):
        raise ValueError("One reviewed signing-key binding required")
    def factory(server):
        namespace = events.key_namespace(server.scope.principal, server.scope.session,
                                         server.calls.parent / "events.sqlite")
        store = secret_store_factory(namespace) if secret_store_factory is not None else secret_store
        try:
            return configure(server, events, store, callback_hosts=callback_hosts, callback_preflight=callback_preflight)
        except Exception:
            close = getattr(store, "close", None)
            if callable(close):
                close()
            raise
    agent.main(["--scope", str(scope_path)], runtime_factory=factory)


def main(argv=None):
    import argparse
    from pathlib import Path
    import importlib.util
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope", type=Path, required=True)
    parser.add_argument("--codescribe", type=Path)
    parser.add_argument("--native-sha256")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--callback-host", action="append")
    mode.add_argument("--callback-preflight", action="store_true")
    mode.add_argument("--check-scope", action="store_true")
    args = parser.parse_args(argv)
    if args.check_scope:
        if args.codescribe is not None or args.native_sha256 is not None:
            parser.error("Scope check does not accept a native key binding")
        return check_scope(args.scope)
    if args.callback_preflight:
        if args.codescribe is not None or args.native_sha256 is not None:
            parser.error("Callback preflight does not accept a native key binding")
        return run(args.scope, callback_preflight=True)
    if args.codescribe is None or args.native_sha256 is None:
        parser.error("Pinned callback mode requires the reviewed native key binding")
    spec = importlib.util.spec_from_file_location("codescribe_native_event_keys",
        Path(__file__).resolve().with_name("bus-mcp-key-store.py"))
    keys = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(keys)
    return run(args.scope, callback_hosts=args.callback_host, secret_store_factory=lambda namespace:
        keys.CodescribeSecretStore(args.codescribe, namespace, args.native_sha256))


if __name__ == "__main__":
    main()
