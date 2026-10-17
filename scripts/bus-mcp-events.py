"""MCP Events subscription metadata. No plaintext signing keys or BUS writer.

The injected callback transport must connect to the supplied validated addresses,
preserve the hostname for TLS, block redirects, and enforce a ten-second limit.
Secret storage and transport are injected; production admission is explicit.
"""
import base64
import datetime
import fcntl
import hashlib
import hmac
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import sqlite3
import threading
import time
from urllib.parse import urlsplit


EVENT_NAME = "codescribe.mailbox.ready"
PROTOCOL_VERSION = "2026-07-28"
REQUEST_REASONS = frozenset({
    "mailbox_owner_mismatch", "mailbox_bus_mismatch", "mailbox_pending_invalid",
    "mailbox_envelope_invalid", "mailbox_delivery_owner_mismatch",
    "subscription_parameters_invalid", "event_arguments_invalid", "replay_cursor_unsupported",
    "webhook_delivery_invalid", "callback_url_invalid", "signing_secret_invalid", "ttl_invalid",
})


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def iso_time(seconds):
    return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc).isoformat()


class CallbackError(ValueError):
    code = -32015

    def __init__(self, reason, *, callback_host=None):
        self.reason = reason
        self.callback_host = callback_host
        super().__init__("Callback verification refused")


class EventRequestError(ValueError):
    """Closed diagnostic vocabulary; never retain supplied values or messages."""

    def __init__(self, reason):
        if reason not in REQUEST_REASONS:
            raise ValueError("Unknown event refusal category")
        self.reason = reason
        super().__init__("Event request refused")


def callback_hostname(value):
    """A bounded DNS name only: never userinfo, path, query, port or IP literal."""
    if (not isinstance(value, str) or not 1 <= len(value) <= 253 or "." not in value
            or not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?"
                                r"(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+", value)):
        raise ValueError("DNS callback hostname required")
    try:
        ipaddress.ip_address(value)
    except ValueError:
        return value.lower()
    raise ValueError("IP callback hostname refused")


def signing_key(secret):
    if not isinstance(secret, str) or len(secret) > 100 or not secret.startswith("whsec_"):
        raise ValueError("Standard Webhooks signing secret required")
    try:
        key = base64.b64decode(secret[6:], validate=True)
    except (ValueError, TypeError):
        raise ValueError("Invalid signing secret") from None
    if not 24 <= len(key) <= 64:
        raise ValueError("Invalid signing secret length")
    return key


def signed_headers(identity, timestamp, body, subscription, keys):
    message = identity.encode() + b"." + str(timestamp).encode() + b"." + body
    signatures = ["v1," + base64.b64encode(hmac.digest(key, message, "sha256")).decode()
                  for key in keys]
    return {"Content-Type": "application/json", "webhook-id": identity,
            "webhook-timestamp": str(timestamp), "webhook-signature": " ".join(signatures),
            "X-MCP-Subscription-Id": subscription}


class SecretStoreError(RuntimeError):
    """Categorized failure; never includes keys or backend diagnostics."""


class MemorySecretStore:
    """Synthetic-only backend; callers must share it to model process restart."""
    synthetic_only = True

    def __init__(self):
        self.values = {}

    def put(self, reference, secret):
        self.values[reference] = secret

    def get(self, reference):
        if reference not in self.values:
            raise SecretStoreError("Signing key unavailable")
        return self.values[reference]

    def delete(self, reference):
        self.values.pop(reference, None)

    def references(self, namespace):
        return [r for r in self.values if r.startswith(namespace)]


def key_namespace(principal, session, path):
    return "MCP_EVENTS_" + hashlib.sha256(
        canonical([principal, session, str(Path(path).resolve())]).encode()).hexdigest() + "_"


class Events:
    """One fixed principal and mailbox; persistent transport metadata, no text."""
    callback_error_type = CallbackError
    request_error_type = EventRequestError
    request_error_reasons = REQUEST_REASONS
    callback_hostname = staticmethod(callback_hostname)

    def __init__(self, path, principal, session, *, callback=None, resolve=None,
                 callback_hosts=(), secret_store=None, authorized=lambda: True, clock=time.time,
                 timer_factory=threading.Timer, callback_preflight=False):
        if (type(callback_preflight) is not bool
                or callback_preflight and (callback_hosts or secret_store is not None
                                           or callback is not None or resolve is not None)):
            raise ValueError("Exclusive callback preflight required")
        if secret_store is None and not callback_preflight:
            raise SecretStoreError("Explicit secure signing-key backend required")
        self.store = secret_store
        self.namespace = key_namespace(principal, session, path)
        self.principal, self.session = principal, session
        self.callback, self.resolve = callback, resolve
        self.hosts = frozenset(callback_hostname(host) for host in callback_hosts)
        self.authorized, self.clock = authorized, clock
        self.callback_preflight = callback_preflight
        self.lock = threading.RLock()
        self.timer_factory, self.timer, self.closed = timer_factory, None, False
        self.db = self.process_lock = None
        if callback_preflight:
            return  # No key backend, metadata files, subscriptions, timers or network.
        path = Path(path)
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        os.fchmod(fd, 0o600)
        os.close(fd)
        try:
            descriptor = os.open(path.with_suffix(".lock"), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
            os.fchmod(descriptor, 0o600)
            self.process_lock = os.fdopen(descriptor, "a+b")
        except OSError:
            raise SecretStoreError("Events metadata lock refused") from None
        try:
            fcntl.flock(self.process_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.db = sqlite3.connect(path, check_same_thread=False)
        except Exception:
            self.process_lock.close()
            raise SecretStoreError("Events metadata already owned or unavailable") from None
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA secure_delete=ON")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS subscriptions (
              id TEXT PRIMARY KEY, principal TEXT, url TEXT, expires REAL,
              current_key_ref TEXT, old_key_ref TEXT, rotation_until REAL,
              verified_until REAL, active INTEGER);
            CREATE TABLE IF NOT EXISTS deliveries (
              subscription TEXT, event TEXT, attempts INTEGER, next_attempt REAL,
              state TEXT, PRIMARY KEY(subscription, event));
            CREATE TABLE IF NOT EXISTS cleanup (reference TEXT PRIMARY KEY);

        """)
        columns = {r[1] for r in self.db.execute("PRAGMA table_info(subscriptions)")}
        if "secret" in columns or "old_secret" in columns:
            self.db.close()
            self.process_lock.close()
            raise SecretStoreError("Plaintext prototype database requires separate retirement")
        if "body" not in {r[1] for r in self.db.execute("PRAGMA table_info(deliveries)")}:
            self.db.execute("ALTER TABLE deliveries ADD COLUMN body TEXT")
        try:
            self.maintenance()
        except Exception:
            if self.timer is not None:
                self.timer.cancel()
            self.db.close()
            self.process_lock.close()
            raise

    def key(self, reference):
        if not isinstance(reference, str) or not reference.startswith(self.namespace):
            raise SecretStoreError("Signing-key namespace refused")
        try:
            secret = self.store.get(reference)
            signing_key(secret)
            return secret
        except Exception:
            raise SecretStoreError("Signing key unavailable") from None

    def retire(self, references):
        for reference in references:
            if reference:
                if not reference.startswith(self.namespace):
                    raise SecretStoreError("Signing-key namespace refused")
                self.db.execute("INSERT OR IGNORE INTO cleanup VALUES (?)", (reference,))

    def reconcile(self):
        live = {r[0] for r in self.db.execute(
            "SELECT current_key_ref FROM subscriptions WHERE principal=? UNION "
            "SELECT old_key_ref FROM subscriptions WHERE principal=?",
            (self.principal, self.principal)) if r[0]}
        try:
            references = self.store.references(self.namespace)
            with self.db:
                self.retire(set(references) - live)
            for row in self.db.execute("SELECT reference FROM cleanup").fetchall():
                reference = row[0]
                if not reference.startswith(self.namespace):
                    raise SecretStoreError("Signing-key namespace refused")
                self.store.delete(reference)
                with self.db:
                    self.db.execute("DELETE FROM cleanup WHERE reference=?", (reference,))
        except Exception:
            self.arm_cleanup()
            raise SecretStoreError("Signing-key cleanup pending") from None

    def close(self):
        with self.lock:
            if self.closed:
                return
            self.closed = True
            if self.callback_preflight:
                return
            if self.timer is not None:
                self.timer.cancel()
            self.db.close()
            self.process_lock.close()
            close = getattr(self.store, "close", None)
            if callable(close):
                close()

    def forget(self, identity):
        row = self.db.execute("SELECT current_key_ref, old_key_ref FROM subscriptions "
                              "WHERE id=? AND principal=?", (identity, self.principal)).fetchone()
        if row:
            self.retire(row)
        self.db.execute("DELETE FROM deliveries WHERE subscription=?", (identity,))
        self.db.execute("DELETE FROM subscriptions WHERE id=? AND principal=?", (identity, self.principal))

    def arm_cleanup(self):
        if self.timer is not None:
            self.timer.cancel()
            self.timer = None
        deadlines = self.db.execute("SELECT expires, rotation_until, old_key_ref IS NOT NULL AS rotating "
                                    "FROM subscriptions WHERE principal=?", (self.principal,)).fetchall()
        times = [self.clock() + 1] if self.db.execute("SELECT 1 FROM cleanup LIMIT 1").fetchone() else []
        times += [deadline for row in deadlines for deadline in
                 ([row["expires"], row["rotation_until"]] if row["rotating"] else [row["expires"]])]
        if times:
            self.timer = self.timer_factory(max(0.01, min(times) - self.clock()), self.cleanup_due)
            self.timer.daemon = True
            self.timer.start()

    def cleanup_due(self):
        with self.lock:
            if not self.closed:
                try:
                    self.maintenance()
                except (PermissionError, SecretStoreError):
                    pass  # Metadata is inactive; a failed backend deletion remains scheduled.

    def maintenance(self, principal=None):
        """Erase expired/withdrawn keys on startup, requests and one idle timer.

        secure_delete clears deleted SQLite cells. A stopped process cannot run
        its timer; reopening purges overdue material before any protocol use.
        """
        if principal is not None and principal != self.principal:
            raise PermissionError("Mailbox access refused")
        with self.lock:
            if self.closed:
                raise PermissionError("Events connection closed")
            if self.callback_preflight:
                if not self.authorized():
                    raise PermissionError("Mailbox access refused")
                return
            allowed = self.authorized()
            now = self.clock()
            with self.db:
                for row in self.db.execute("SELECT id FROM subscriptions WHERE principal=? "
                        "AND (expires<=? OR active=0 OR ?=0)", (self.principal, now, int(allowed))).fetchall():
                    self.forget(row["id"])
                for row in self.db.execute("SELECT old_key_ref FROM subscriptions WHERE principal=? "
                        "AND rotation_until<=?", (self.principal, now)).fetchall():
                    self.retire(row)
                self.db.execute("UPDATE subscriptions SET old_key_ref=NULL, rotation_until=0 "
                        "WHERE principal=? AND rotation_until<=?", (self.principal, now))
            try:
                self.reconcile()
            finally:
                self.arm_cleanup()
            if not allowed:
                raise PermissionError("Mailbox access refused")

    def check_owner(self, principal):
        self.maintenance(principal)

    def catalog(self, principal):
        self.check_owner(principal)
        return {"events": [{"name": EVENT_NAME,
                "description": "An unread message is available in this agent's mailbox. No text is sent.",
                "delivery": ["webhook"],
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
                "payloadSchema": {"type": "object", "properties": {
                    "delivery_id": {"type": "string", "pattern": "^[0-9a-f]{24}$"},
                    "provider_session_id": {"type": "string"}},
                    "required": ["delivery_id", "provider_session_id"], "additionalProperties": False}}]}

    def identity(self, principal, params, *, subscribe):
        self.check_owner(principal)
        allowed = {"name", "arguments", "delivery"} | ({"cursor", "ttlMs"} if subscribe else set())
        if not isinstance(params, dict) or set(params) - allowed:
            raise EventRequestError("subscription_parameters_invalid")
        if params.get("name") != EVENT_NAME or params.get("arguments") != {}:
            raise EventRequestError("event_arguments_invalid")
        if subscribe and params.get("cursor") is not None:
            raise EventRequestError("replay_cursor_unsupported")
        delivery = params.get("delivery")
        keys = {"mode", "url", "secret"} if subscribe else {"mode", "url"}
        if not isinstance(delivery, dict) or set(delivery) != keys or delivery.get("mode") != "webhook":
            raise EventRequestError("webhook_delivery_invalid")
        url = delivery.get("url")
        if not isinstance(url, str) or len(url) > 2048:
            raise EventRequestError("callback_url_invalid")
        try:
            if any(ord(c) <= 32 or ord(c) >= 127 for c in url):
                raise ValueError("ASCII URL without whitespace required")
            parts = urlsplit(url)
            host = callback_hostname(parts.hostname)
            permitted = (parts.scheme == "https" and (host in self.hosts or self.callback_preflight)
                         and parts.port in (None, 443) and not parts.username
                         and not parts.password and not parts.fragment)
        except ValueError:
            permitted = False
        if not permitted:
            raise CallbackError("destination_refused")
        identity = "sub_" + hashlib.sha256(canonical(
            [principal, url, EVENT_NAME, {}, self.session]).encode()).hexdigest()
        return identity, url

    def post(self, url, body, headers):
        if self.callback is None or self.resolve is None:
            raise CallbackError("transport_unconfigured")
        host = urlsplit(url).hostname
        try:
            addresses = tuple(self.resolve(host))
            if not addresses or any(not ipaddress.ip_address(a).is_global for a in addresses):
                raise CallbackError("destination_refused")
            return self.callback(url, addresses, body, headers)
        except CallbackError:
            raise
        except (OSError, TimeoutError):
            raise CallbackError("timeout") from None
        except ValueError:
            raise CallbackError("destination_refused") from None

    def subscribe(self, principal, params):
        with self.lock:
            identity, url = self.identity(principal, params, subscribe=True)
            secret = params["delivery"]["secret"]
            try:
                key = signing_key(secret)
            except ValueError:
                raise EventRequestError("signing_secret_invalid") from None
            ttl = params.get("ttlMs", 3600000)
            if ttl is None:
                ttl = 3600000  # Never grant an unbounded lifetime in this MVP.
            if type(ttl) is not int or ttl < 1000:
                raise EventRequestError("ttl_invalid")
            if self.callback_preflight:
                raise CallbackError("host_unpinned", callback_host=callback_hostname(urlsplit(url).hostname))
            now = self.clock()
            previous = self.db.execute("SELECT * FROM subscriptions WHERE id=?", (identity,)).fetchone()
            if previous is None and self.db.execute("SELECT 1 FROM subscriptions WHERE principal=? LIMIT 1",
                                                   (principal,)).fetchone():
                raise ValueError("Unsubscribe the previous callback before replacing its destination")
            if not (previous and previous["active"] and self.key(previous["current_key_ref"]) == secret
                    and previous["verified_until"] > now):
                challenge = secrets.token_urlsafe(24)
                body = canonical({"type": "verification", "challenge": challenge}).encode()
                headers = signed_headers("verification_" + secrets.token_hex(12), int(now), body, identity, [key])
                status, response = self.post(url, body, headers)
                if (not 200 <= status < 300 or not isinstance(response, dict)
                        or not isinstance(response.get("challenge"), str)
                        or not hmac.compare_digest(challenge, response["challenge"])):
                    raise CallbackError("challenge_failed")
            expires = now + min(ttl, 86400000) / 1000
            same = previous and self.key(previous["current_key_ref"]) == secret
            reference = previous["current_key_ref"] if same else self.namespace + secrets.token_hex(16)
            old = previous["old_key_ref"] if same else previous["current_key_ref"] if previous else None
            rotation_until = (previous["rotation_until"] if same else now + 60) if old else 0
            if not same:
                try:
                    self.store.put(reference, secret)
                except Exception:
                    # The backend may have written before reporting failure.
                    with self.db:
                        self.retire([reference])
                    self.arm_cleanup()
                    raise SecretStoreError("Signing-key write refused") from None
            try:
                with self.db:
                    if previous and not same:
                        self.retire([previous["old_key_ref"]])
                    self.db.execute("INSERT OR REPLACE INTO subscriptions VALUES (?,?,?,?,?,?,?,?,?)",
                        (identity, principal, url, expires, reference, old, rotation_until, now + 300, 1))
            except Exception:
                if not same:
                    with self.db:
                        self.retire([reference])
                    self.reconcile()
                raise SecretStoreError("Signing-key metadata write refused") from None
            self.reconcile()
            self.arm_cleanup()
            return {"id": identity, "refreshBefore": iso_time(expires), "cursor": None, "truncated": False}

    def unsubscribe(self, principal, params):
        with self.lock:
            identity, _url = self.identity(principal, params, subscribe=False)
            if self.callback_preflight:
                return {}  # No DB, key store, timer or callback exists to remove.
            with self.db:
                self.forget(identity)
            self.reconcile()
            self.arm_cleanup()
            return {}

    def ready(self, principal, delivery_id):
        """Skip already notified pending rows so later fresh arrivals cannot starve."""
        with self.lock:
            self.check_owner(principal)
            if self.callback_preflight:
                return False
            event = "evt_" + hashlib.sha256((self.session + ":" + delivery_id).encode()).hexdigest()
            now = self.clock()
            return self.db.execute("SELECT 1 FROM subscriptions s LEFT JOIN deliveries d "
                "ON d.subscription=s.id AND d.event=? WHERE s.principal=? AND s.active=1 "
                "AND s.expires>? AND (d.event IS NULL OR "
                "(d.state='retry' AND d.next_attempt<=? AND d.attempts<3)) LIMIT 1",
                (event, principal, now, now)).fetchone() is not None

    def deliver(self, principal, delivery_id, occurred_at):
        """Called with existing owned pending IDs, never with arbitrary bus text.

        One bounded attempt per eligible subscription; the caller schedules later
        ticks. Restart retries from the canonical unread mailbox with the same ID.
        Webhook acceptance never ACKs a Codescribe delivery.
        """
        if self.callback_preflight:
            self.check_owner(principal)
            return []
        if not isinstance(delivery_id, str) or not re.fullmatch(r"[0-9a-f]{24}", delivery_id):
            raise ValueError("Invalid delivery ID")
        if not isinstance(occurred_at, str) or datetime.datetime.fromisoformat(
                occurred_at.replace("Z", "+00:00")).tzinfo is None:
            raise ValueError("Timezone-aware occurrence timestamp required")
        with self.lock:
            self.check_owner(principal)
            now = self.clock()
            event_id = "evt_" + hashlib.sha256((self.session + ":" + delivery_id).encode()).hexdigest()
            body = canonical({"eventId": event_id, "name": EVENT_NAME, "timestamp": occurred_at,
                              "data": {"delivery_id": delivery_id, "provider_session_id": self.session},
                              "cursor": None}).encode()
            if len(body) > 256 * 1024:
                raise ValueError("Event exceeds 256 KiB")
            results = []
            for sub in self.db.execute("SELECT * FROM subscriptions WHERE principal=? AND active=1 AND expires>?",
                                       (principal, now)).fetchall():
                old = self.db.execute("SELECT * FROM deliveries WHERE subscription=? AND event=?",
                                      (sub["id"], event_id)).fetchone()
                if old and (old["state"] != "retry" or old["next_attempt"] > now or old["attempts"] >= 3):
                    continue
                attempt = (old["attempts"] if old else 0) + 1
                # Persist the serialized metadata once; a retry keeps this exact body.
                stable_body = old["body"].encode() if old and old["body"] else body
                # A local store outage sends nothing and consumes no HTTP retry.
                keys = [signing_key(self.key(sub["current_key_ref"]))]
                if sub["old_key_ref"] and sub["rotation_until"] > now:
                    keys.append(signing_key(self.key(sub["old_key_ref"])))
                headers = signed_headers(event_id, int(now), stable_body, sub["id"], keys)
                with self.db:
                    self.db.execute("INSERT OR REPLACE INTO deliveries VALUES (?,?,?,?,?,?)",
                        (sub["id"], event_id, attempt, now + 2 ** attempt, "retry", stable_body.decode()))
                try:
                    status, _response = self.post(sub["url"], stable_body, headers)
                except CallbackError:
                    status = 0
                accepted = 200 <= status < 300
                transient = status == 0 or status == 429 or 500 <= status < 600
                state = "accepted" if accepted else "retry" if transient and attempt < 3 else "failed"
                with self.db:
                    self.db.execute("UPDATE deliveries SET state=? WHERE subscription=? AND event=?",
                                    (state, sub["id"], event_id))
                    if status == 410:
                        self.forget(sub["id"])
                self.reconcile()
                results.append({"subscription_id": sub["id"], "event_id": event_id,
                                "attempt": attempt, "state": state, "status": status})
            self.arm_cleanup()
            return results
