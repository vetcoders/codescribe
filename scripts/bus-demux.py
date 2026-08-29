#!/usr/bin/env python3
"""Named, session-aware follower for the clean Codescribe Transcript Bus.

The helper never opens audio. It reads ``codescribe.transcript.v1`` NDJSON and
emits small agent-bridge envelopes. Product installs run it from the stable
path below, not from a source checkout::

  python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
    --provider codex --session <provider-session-id> --name james --drafts --follow

``--provider`` plus ``--session`` enables a collision-safe lease, heartbeat,
and byte cursor. Re-running the same command resumes after the last consumed
bus line, including lines appended while the provider session was recovering.
Drafts are useful for live replies; only a ``transcript_sealed`` envelope sets
``state_change_allowed`` to true.

Named routing requires an exact name in the immutable snapshot. When it is
absent, this bridge deliberately does not guess an audience; whether unnamed
sealed speech should later broadcast or await a human routing choice remains a
product decision outside this consumer.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import select
import sys
import time
from pathlib import Path
from typing import Any, Iterator

BUS_FILENAME = "transcript-events.jsonl"
CLEAN_SCHEMA = "codescribe.transcript.v1"
#: The app has written its words here since 2026-08-27 22:36. A follower that
#: knows only CLEAN_SCHEMA sees lifecycle rows and reports nothing for a real
#: take — deaf, while looking healthy.
EVIDENCE_SCHEMA = "codescribe.transcript-evidence.v1"
TERMINAL_SEAL = "record_ledger_terminal_seal"
INSTALL_INTERLOCK_FILENAME = "install-runtime.lock"
SEALED = "transcript_sealed"
LIVE_STATUSES = ("utterance_draft", "utterance_revised")
LEASE_SCHEMA = "codescribe.agent-bridge.lease.v1"
ATTACH_SCHEMA = "codescribe.agent-bridge.attach.v1"
EVENT_SCHEMA = "codescribe.agent-bridge.event.v1"
ACTIVE_NAMES_SCHEMA = "codescribe.agent-bridge.active-names.v1"
DEFAULT_LEASE_TTL_SECONDS = 120.0
ASSIGN_RE = re.compile(
    r"(?i)(?:będziesz(?:\s+od)?\s+teraz|nazywam\s+cię|nazywasz\s+się|"
    r"you(?:['’]re|\s+are)|cześć|hello)\s+([A-Za-zĄĆĘŁŃÓŚŹŻąćęłńóśźż]{2,32})"
)
SAFE_LEASE_RE = re.compile(r"^[a-zA-Z0-9_-]{8,80}$")
LAST_SESSION_WAV = "last_session.wav"
BUS_PATH_ENV_KEYS = (
    "CODESCRIBE_TRANSCRIPT_BUS_PATH",
    "XDG_STATE_HOME",
    "CODESCRIBE_DATA_DIR",
)


def _config_dir(env: dict[str, str]) -> Path:
    if "CODESCRIBE_DATA_DIR" in env:
        raw = env["CODESCRIBE_DATA_DIR"]
        path = Path(os.path.expanduser(raw))
        # Rust's canonicalize rejects an empty PathBuf instead of treating it
        # as cwd. Preserve that relative-path edge case exactly.
        if raw:
            try:
                return path.resolve(strict=True)
            except OSError:
                pass
        return path
    return Path.home() / ".codescribe"


def _env_path(seed_env: dict[str, str]) -> Path:
    if "CODESCRIBE_ENV_PATH" in seed_env:
        return Path(os.path.expanduser(seed_env["CODESCRIBE_ENV_PATH"]))
    return _config_dir(seed_env) / ".env"


def _parse_env_file(path: Path) -> dict[str, str]:
    try:
        canonical = path.resolve(strict=True)
        contents = canonical.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return {}

    parsed: dict[str, str] = {}
    for raw in contents.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        parsed[key.strip()] = value.strip().strip('"').strip("'")
    return parsed


def _runtime_path_env() -> dict[str, str]:
    # Config::load_with_keychain_population derives the dotenv path from the
    # process environment first, then injects non-promoted keys only when the
    # process did not already define them (an explicit empty value still wins).
    runtime_env = dict(os.environ)
    env_path = _env_path(runtime_env)
    if env_path.exists():
        file_env = _parse_env_file(env_path)
        for key in BUS_PATH_ENV_KEYS:
            if key not in runtime_env and key in file_env:
                runtime_env[key] = file_env[key]
    return runtime_env


def bus_path() -> Path:
    env = _runtime_path_env()
    explicit = env.get("CODESCRIBE_TRANSCRIPT_BUS_PATH", "").strip()
    if explicit:
        return Path(os.path.expanduser(explicit))
    xdg = env.get("XDG_STATE_HOME", "").strip()
    if xdg:
        return Path(os.path.expanduser(xdg)) / "codescribe" / BUS_FILENAME
    return _config_dir(env) / BUS_FILENAME


def install_interlock_path() -> Path:
    # The app acquires this before dotenv bootstrap. Keep the lease at one
    # process-independent per-user path so data-dir overrides cannot split the
    # installer and runtime onto different lock files.
    return Path.home() / ".codescribe" / INSTALL_INTERLOCK_FILENAME


def installation_idle(path: Path) -> bool:
    if not path.exists():
        return True
    if not path.is_file():
        return False
    open_sessions: set[str] = set()
    try:
        with path.open(encoding="utf-8", errors="strict") as handle:
            for raw in handle:
                raw = raw.strip()
                if not raw:
                    continue
                try:
                    event = json.loads(raw)
                except json.JSONDecodeError:
                    return False
                if not isinstance(event, dict):
                    return False
                status = event.get("status")
                if status == "session_started":
                    session_id = event.get("session_id")
                    if not isinstance(session_id, str) or not session_id:
                        return False
                    open_sessions.add(session_id)
                elif status in ("session_ended", "transcript_sealed"):
                    session_id = event.get("session_id")
                    if not isinstance(session_id, str) or not session_id:
                        return False
                    open_sessions.discard(session_id)
    except (OSError, UnicodeDecodeError):
        return False
    return not open_sessions


def bridge_home() -> Path:
    override = os.environ.get("CODESCRIBE_AGENT_BRIDGE_HOME", "").strip()
    if override:
        return Path(os.path.expanduser(override))
    return Path.home() / ".codescribe" / "agent-bridge"


def name_pat(name: str) -> re.Pattern[str]:
    stem = re.escape(name.strip())
    return re.compile(rf"(?i)\b{stem}(?:ie|owi|a|em|u|ie|ieś|owi)?\b")


def assigned_name(text: str) -> str | None:
    match = ASSIGN_RE.search(text or "")
    if not match:
        return None
    return match.group(1).casefold()


def addressed_to(text: str, name: str) -> bool:
    if not name:
        return False
    return name_pat(name).search(text or "") is not None


def event_kind(status: Any) -> str:
    return {
        "utterance_draft": "draft",
        "utterance_revised": "revised",
        SEALED: "seal",
    }.get(str(status), "event")


def valid_session_audio_id(session_id: Any) -> str | None:
    """Same alphabet as the controller retain path. Never a filesystem path."""
    if not isinstance(session_id, str):
        return None
    if not SAFE_LEASE_RE.fullmatch(session_id):
        return None
    return session_id


def assigned_session_wav(
    event: dict[str, Any], env: dict[str, str] | None = None
) -> str | None:
    """Map a Bus take to its own wav. ``last_session.wav`` is never identity."""
    env = env if env is not None else dict(os.environ)
    sid = valid_session_audio_id(
        event.get("session_id") or event.get("occurrence_session_id")
    )
    if not sid:
        return None
    explicit = event.get("wav")
    if isinstance(explicit, str) and explicit.strip():
        path = Path(os.path.expanduser(explicit.strip()))
        if path.name != LAST_SESSION_WAV:
            return str(path)
    return str(_config_dir(env) / "sessions" / f"{sid}.wav")


def slim(
    event: dict[str, Any], audience: str, kind: str | None = None
) -> dict[str, Any]:
    status = event.get("status")
    producer_schema = event.get("producer_schema") or event.get("schema")
    payload = {
        "schema": EVENT_SCHEMA,
        "audience": audience,
        "kind": kind or event_kind(status),
        "status": status,
        "sequence": event.get("sequence"),
        "session_id": event.get("session_id"),
        "utterance_id": event.get("utterance_id"),
        "emitted_at": event.get("emitted_at"),
        "mode": event.get("mode"),
        # Keep producer provenance and reducer coordinates observable.  This
        # bridge is a consumer: neither field is ours to rewrite.
        "source": event.get("source"),
        "producer_schema": producer_schema,
        "source_event_id": event.get("source_event_id") or source_event_identity(event),
        "text": event.get("text") if isinstance(event.get("text"), str) else "",
        "state_change_allowed": status == SEALED,
    }
    wav = assigned_session_wav(event)
    if wav:
        payload["wav"] = wav
    if producer_schema == EVIDENCE_SCHEMA:
        payload.update(
            {
                "reducer_revision": event.get("reducer_revision"),
                "reducer_action": event.get("reducer_action"),
                "occurrence_session_id": event.get("occurrence_session_id"),
                "capture_epoch": event.get("capture_epoch"),
                "sample_start": event.get("sample_start"),
                "sample_end": event.get("sample_end"),
                "document_index": event.get("document_index"),
            }
        )
    return payload


def parse_line(raw: str) -> dict[str, Any] | None:
    raw = raw.strip()
    if not raw:
        return None
    try:
        event = json.loads(raw)
    except json.JSONDecodeError:
        return None
    if not isinstance(event, dict):
        return None
    if event.get("schema") not in (CLEAN_SCHEMA, EVIDENCE_SCHEMA):
        return None
    return event


def _identity(parts: tuple[Any, ...]) -> str:
    """Stable opaque identity from authoritative metadata, never transcript text."""
    encoded = "\0".join("" if part is None else str(part) for part in parts)
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()[:24]


def source_event_identity(event: dict[str, Any]) -> str:
    """Identify one Bus observation without comparing its rendered payload."""
    if event.get("schema") == EVIDENCE_SCHEMA:
        return _identity(
            (
                "evidence",
                event.get("session_id"),
                event.get("sequence"),
                event.get("reducer_revision"),
                event.get("reducer_action"),
                event.get("occurrence_session_id"),
                event.get("capture_epoch"),
                event.get("sample_start"),
                event.get("sample_end"),
                event.get("document_index"),
            )
        )
    return _identity(
        (
            "clean",
            event.get("session_id"),
            event.get("sequence"),
            event.get("utterance_id"),
            event.get("status"),
        )
    )


def terminal_seal_identity(event: dict[str, Any]) -> str:
    """One terminal reducer phase, even when its receipt projects many rows."""
    return _identity(
        (
            "terminal-seal",
            event.get("session_id"),
            event.get("reducer_revision"),
            event.get("reducer_action"),
        )
    )


class EvidenceNormalizer:
    """Translate ``transcript-evidence.v1`` rows into the shape the bridge speaks.

    ``rendered_text`` is an immutable full snapshot from the reducer.  The
    bridge forwards it verbatim; it never infers a delta, ordering, revision,
    or finality from characters.  Terminal rows are coalesced only by the
    reducer's stable terminal phase identity, because one terminal receipt can
    project once per document entry.
    """

    def __init__(self) -> None:
        self._terminal_seals: set[str] = set()

    def normalize(self, event: dict[str, Any] | None) -> dict[str, Any] | None:
        if event is None or event.get("schema") != EVIDENCE_SCHEMA:
            return event
        document = event.get("rendered_text")
        if not isinstance(document, str):
            return None
        if str(event.get("reducer_action") or "") == TERMINAL_SEAL:
            seal_id = terminal_seal_identity(event)
            if seal_id in self._terminal_seals:
                return None
            self._terminal_seals.add(seal_id)
            return self._as_clean(event, SEALED, document)
        return self._as_clean(event, LIVE_STATUSES[1], document)

    def _as_clean(
        self, event: dict[str, Any], status: str, text: str
    ) -> dict[str, Any]:
        return {
            "schema": CLEAN_SCHEMA,
            "sequence": event.get("sequence"),
            "session_id": event.get("session_id"),
            "mode": event.get("mode"),
            "utterance_id": source_event_identity(event),
            "emitted_at": event.get("emitted_at"),
            "status": status,
            "text": text,
            "source": event.get("source"),
            "producer_schema": event.get("schema"),
            "source_event_id": source_event_identity(event),
            "reducer_revision": event.get("reducer_revision"),
            "reducer_action": event.get("reducer_action"),
            "occurrence_session_id": event.get("occurrence_session_id"),
            "capture_epoch": event.get("capture_epoch"),
            "sample_start": event.get("sample_start"),
            "sample_end": event.get("sample_end"),
            "document_index": event.get("document_index"),
        }


def emit(payload: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n")
    sys.stdout.flush()


def consider(
    event: dict[str, Any],
    *,
    name: str | None,
    hear_all: bool,
    drafts: bool,
    debug: bool,
) -> dict[str, Any] | None:
    status = event.get("status")
    if status != SEALED and not (drafts and status in LIVE_STATUSES):
        return None
    text = event.get("text") or ""
    # Recognition can classify a destination, but never rewrites transcript
    # state.  Exact-name omission intentionally means no named route: there is
    # no heuristic or LLM fallback hidden in this consumer.
    addressable = text
    claimed = assigned_name(text)
    if claimed:
        payload = slim(event, claimed, kind="name_assignment")
        payload["name"] = claimed
        if (
            hear_all
            or (name and claimed == name.casefold())
            or addressed_to(addressable, name or "")
        ):
            return payload
        if debug:
            sys.stderr.write(f"bus-demux: drop assignment name={claimed}\n")
        return None
    if hear_all:
        return slim(event, "*")
    if name and addressed_to(addressable, name):
        return slim(event, name.casefold())
    if debug and status == SEALED:
        sys.stderr.write("bus-demux: drop unnamed-or-other seal\n")
    return None


def iter_new_lines(path: Path, offset: int) -> tuple[list[tuple[str, int]], int]:
    """Return complete UTF-8 lines paired with their exclusive byte cursors."""
    try:
        size = path.stat().st_size
    except FileNotFoundError:
        return [], offset
    if size < offset:
        # Rotation/truncation is an authority boundary. Replaying the new file
        # from byte zero could disclose sealed commands that predate this
        # provider lease, so resume at the new EOF and wait for fresh events.
        return [], size
    entries: list[tuple[str, int]] = []
    with path.open("rb") as handle:
        handle.seek(offset)
        while True:
            raw = handle.readline()
            if not raw:
                break
            if not raw.endswith(b"\n"):
                break
            entries.append((raw.decode("utf-8", errors="replace"), handle.tell()))
    return entries, entries[-1][1] if entries else offset


class BusEventTrigger:
    """Block on an OS file event; interval sleep is a non-macOS fallback only."""

    def __init__(self, path: Path, fallback_interval: float) -> None:
        self.path = path
        self.fallback_interval = max(0.01, fallback_interval)
        self.mode = "interval-fallback"
        self._queue: Any | None = None
        self._descriptor: int | None = None
        self._arm_kqueue()

    def _arm_kqueue(self) -> None:
        if not hasattr(select, "kqueue") or self._queue is not None:
            return
        descriptor: int | None = None
        queue: Any | None = None
        try:
            descriptor = os.open(self.path, os.O_RDONLY)
            queue = select.kqueue()
            notes = (
                select.KQ_NOTE_WRITE
                | select.KQ_NOTE_EXTEND
                | select.KQ_NOTE_RENAME
                | select.KQ_NOTE_DELETE
                | select.KQ_NOTE_REVOKE
            )
            change = select.kevent(
                descriptor,
                filter=select.KQ_FILTER_VNODE,
                flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                fflags=notes,
            )
            queue.control([change], 0, 0)
        except OSError:
            if queue is not None:
                queue.close()
            if descriptor is not None:
                os.close(descriptor)
            return
        self._descriptor = descriptor
        self._queue = queue
        self.mode = "kqueue-vnode"

    def wait(self, timeout: float) -> bool:
        """Return true when the bus emitted a filesystem event."""

        if self._queue is None:
            time.sleep(min(self.fallback_interval, timeout))
            self._arm_kqueue()
            return False
        try:
            events = self._queue.control(None, 1, timeout)
        except OSError:
            self.close()
            self._arm_kqueue()
            return False
        if events and events[0].fflags & (
            select.KQ_NOTE_RENAME | select.KQ_NOTE_DELETE | select.KQ_NOTE_REVOKE
        ):
            self.close()
            self._arm_kqueue()
        return bool(events)

    def close(self) -> None:
        if self._queue is not None:
            self._queue.close()
            self._queue = None
        if self._descriptor is not None:
            os.close(self._descriptor)
            self._descriptor = None
        self.mode = "interval-fallback"


def replay(path: Path) -> Iterator[str]:
    try:
        raw = path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return
        yield from ()  # pragma: no cover - keeps the generator type
    for line in raw.splitlines():
        yield line


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def atomic_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        path.parent.chmod(0o700)
    except OSError:
        pass
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{time.time_ns()}.tmp")
    encoded = (json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n").encode(
        "utf-8"
    )
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        path.chmod(0o600)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def read_json(path: Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None
    return value if isinstance(value, dict) else None


def lease_identifier(provider: str, provider_session_id: str) -> str:
    # The provider session owns the cursor. Name is mutable during --become and
    # therefore cannot participate in the key: binding a name must not fork the
    # greeting follower onto a fresh cursor.
    identity = "\0".join((provider.casefold(), provider_session_id))
    return hashlib.sha256(identity.encode("utf-8")).hexdigest()[:32]


def process_is_alive(pid: Any) -> bool:
    if not isinstance(pid, int) or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def active_leases(
    root: Path, ttl_seconds: float, *, clean: bool = True
) -> list[dict[str, Any]]:
    leases: list[dict[str, Any]] = []
    now = time.time()
    lease_dir = root / "leases"
    try:
        candidates = list(lease_dir.glob("*.json"))
    except OSError:
        return []
    for path in candidates:
        value = read_json(path)
        heartbeat = value.get("heartbeat_unix") if value else None
        fresh = (
            isinstance(heartbeat, (int, float))
            and now - float(heartbeat) <= ttl_seconds
        )
        if not value or value.get("schema") != LEASE_SCHEMA or not fresh:
            if clean:
                try:
                    path.unlink()
                except OSError:
                    pass
            continue
        if value.get("active") is True:
            leases.append(value)
    return leases


class SessionLease:
    """One provider-session cursor and active-name heartbeat."""

    def __init__(
        self,
        *,
        root: Path,
        provider: str,
        provider_session_id: str,
        name: str | None,
        bus: Path,
        requested_id: str | None,
        ttl_seconds: float,
        follow_from_end: bool,
    ) -> None:
        self.root = root
        self.provider = provider.casefold()
        self.provider_session_id = provider_session_id
        self.name = name.casefold() if name else None
        self.bus = str(bus.expanduser().resolve(strict=False))
        self.ttl_seconds = ttl_seconds
        canonical_lease_id = lease_identifier(provider, provider_session_id)
        if requested_id and requested_id != canonical_lease_id:
            raise ValueError(
                "explicit lease id does not belong to this provider session"
            )
        self.lease_id = canonical_lease_id
        if not SAFE_LEASE_RE.fullmatch(self.lease_id):
            raise ValueError("lease id must be 8-80 letters, digits, '_' or '-'")
        self.path = root / "leases" / f"{self.lease_id}.json"
        self.lock_path = root / "leases" / f"{self.lease_id}.lock"
        self.lock_descriptor: int | None = None
        self._acquire_lock()
        try:
            previous = read_json(self.path)
            if previous and not self._matches(previous):
                raise ValueError(
                    f"lease {self.lease_id} belongs to a different provider session or bus"
                )
            self.resumed = False
            self.cursor = 0
            self.last_sequence: Any = None
            if previous and self._matches(previous):
                heartbeat = previous.get("heartbeat_unix")
                fresh = (
                    isinstance(heartbeat, (int, float))
                    and time.time() - float(heartbeat) <= ttl_seconds
                )
                other_pid = previous.get("pid")
                if (
                    fresh
                    and previous.get("active") is True
                    and other_pid != os.getpid()
                    and process_is_alive(other_pid)
                ):
                    raise RuntimeError(
                        f"lease {self.lease_id} is active in pid={other_pid}; "
                        "poll that follower handle"
                    )
                self.cursor = max(0, int(previous.get("cursor", 0)))
                self.last_sequence = previous.get("last_sequence")
                self.name = previous.get("name") or self.name
                self.resumed = True
            elif follow_from_end:
                try:
                    self.cursor = bus.stat().st_size
                except FileNotFoundError:
                    self.cursor = 0
            self.persist(active=True)
            active_leases(root, ttl_seconds, clean=True)
        except BaseException:
            self._release_lock()
            raise

    def _acquire_lock(self) -> None:
        self.lock_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor = os.open(self.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            os.close(descriptor)
            raise RuntimeError(
                f"lease {self.lease_id} already has an active follower; poll that handle"
            ) from error
        self.lock_descriptor = descriptor

    def _release_lock(self) -> None:
        if self.lock_descriptor is None:
            return
        try:
            fcntl.flock(self.lock_descriptor, fcntl.LOCK_UN)
        finally:
            os.close(self.lock_descriptor)
            self.lock_descriptor = None

    def _matches(self, value: dict[str, Any]) -> bool:
        return (
            value.get("schema") == LEASE_SCHEMA
            and value.get("lease_id") == self.lease_id
            and value.get("provider") == self.provider
            and value.get("provider_session_id") == self.provider_session_id
            and value.get("bus") == self.bus
        )

    def persist(
        self,
        *,
        active: bool,
        cursor: int | None = None,
        sequence: Any = None,
    ) -> None:
        if cursor is not None:
            self.cursor = cursor
        if sequence is not None:
            self.last_sequence = sequence
        atomic_json(
            self.path,
            {
                "schema": LEASE_SCHEMA,
                "lease_id": self.lease_id,
                "provider": self.provider,
                "provider_session_id": self.provider_session_id,
                "name": self.name,
                "bus": self.bus,
                "cursor": self.cursor,
                "last_sequence": self.last_sequence,
                "active": active,
                "pid": os.getpid(),
                "heartbeat_unix": time.time(),
                "updated_at": utc_now(),
            },
        )

    def bind_name(self, name: str) -> None:
        self.name = name.casefold()
        self.persist(active=True)

    def enrich(self, payload: dict[str, Any]) -> None:
        payload["lease_id"] = self.lease_id
        payload["provider"] = self.provider
        payload["provider_session_id"] = self.provider_session_id
        # A delivery belongs to one lease owner and one source-event phase.
        # This namespaces native bridge output away from a manual rail while
        # the lease lock refuses a simultaneous second native owner.
        payload["delivery_owner"] = {
            "rail": "native_bus_demux",
            "lease_id": self.lease_id,
            "provider": self.provider,
            "provider_session_id": self.provider_session_id,
        }
        payload["delivery_id"] = _identity(
            (
                "native_bus_demux",
                self.lease_id,
                payload.get("source_event_id"),
                payload.get("kind"),
                payload.get("audience"),
            )
        )

    def attach_receipt(self) -> dict[str, Any]:
        names = sorted(
            {
                str(item["name"])
                for item in active_leases(self.root, self.ttl_seconds)
                if item.get("name")
            }
        )
        return {
            "schema": ATTACH_SCHEMA,
            "kind": "attach",
            "lease_id": self.lease_id,
            "provider": self.provider,
            "provider_session_id": self.provider_session_id,
            "name": self.name,
            "bus": self.bus,
            "cursor": self.cursor,
            "resumed": self.resumed,
            "active_names": names,
        }

    def close(self) -> None:
        try:
            self.persist(active=False)
        finally:
            self._release_lock()


def run(args: argparse.Namespace) -> int:
    path: Path = args.bus
    name: str | None = args.name.casefold() if args.name else None
    hear_all = bool(args.all or args.become)
    if not name and not hear_all:
        sys.stderr.write(
            "bus-demux: unnamed agent does not pass; pass --name or --become/--all\n"
        )
        return 2

    lease: SessionLease | None = None
    if args.provider:
        try:
            lease = SessionLease(
                root=args.bridge_home,
                provider=args.provider,
                provider_session_id=args.session,
                name=name,
                bus=path,
                requested_id=args.lease,
                ttl_seconds=args.lease_ttl,
                follow_from_end=bool(args.follow and not args.from_start),
            )
        except (OSError, RuntimeError, ValueError) as error:
            sys.stderr.write(f"bus-demux: session lease refused: {error}\n")
            return 3
        if lease.name and not name:
            name = lease.name
            hear_all = False
        emit(lease.attach_receipt())

    # One normalizer for the whole run: the evidence grain is stateful (it
    # remembers each session's document and whether its seal was reported), and
    # a fresh one per line would re-emit the entire document every time.
    normalizer = EvidenceNormalizer()
    event_trigger: BusEventTrigger | None = None

    def handle(raw: str, next_cursor: int | None = None) -> None:
        nonlocal name, hear_all
        event = normalizer.normalize(parse_line(raw))
        if event is None:
            if lease and next_cursor is not None:
                lease.persist(active=True, cursor=next_cursor)
            return
        payload = consider(
            event,
            name=name,
            hear_all=hear_all,
            drafts=args.drafts,
            debug=args.debug,
        )
        if payload is None:
            if lease and next_cursor is not None:
                lease.persist(
                    active=True,
                    cursor=next_cursor,
                    sequence=event.get("sequence"),
                )
            return
        if args.become and payload.get("kind") == "name_assignment" and not name:
            name = str(payload["name"])
            hear_all = False
            if lease:
                lease.bind_name(name)
            sys.stderr.write(f"bus-demux: bound name={name}\n")
        if lease:
            lease.enrich(payload)
        emit(payload)
        # At-least-once delivery: advance the durable cursor only after stdout
        # accepted and flushed the command. A crash or broken pipe may replay a
        # command, but it can no longer erase one unseen by the provider.
        if lease and next_cursor is not None:
            lease.persist(
                active=True,
                cursor=next_cursor,
                sequence=event.get("sequence"),
            )

    try:
        if args.once:
            last = None
            for raw in replay(path):
                event = normalizer.normalize(parse_line(raw))
                if event is None:
                    continue
                payload = consider(
                    event,
                    name=name,
                    hear_all=hear_all,
                    drafts=args.drafts,
                    debug=False,
                )
                if payload is not None:
                    last = payload
            if last is None:
                return 1
            if lease:
                lease.enrich(last)
            emit(last)
            return 0

        if lease:
            offset = lease.cursor
        elif args.follow and not args.from_start:
            try:
                offset = path.stat().st_size
            except FileNotFoundError:
                offset = 0
        else:
            offset = 0

        sys.stderr.write(
            f"bus-demux: bus={path} name={name or '*'} follow={int(args.follow)}"
            f" lease={lease.lease_id if lease else '-'}"
        )
        if args.follow:
            event_trigger = BusEventTrigger(path, args.interval)
            sys.stderr.write(f" trigger={event_trigger.mode}")
        sys.stderr.write("\n")
        last_heartbeat = time.monotonic()
        while True:
            previous_offset = offset
            entries, offset = iter_new_lines(path, offset)
            for raw, next_cursor in entries:
                handle(raw, next_cursor)
            if lease and not entries and offset != previous_offset:
                lease.persist(active=True, cursor=offset)
            if not args.follow:
                return 0
            if lease and time.monotonic() - last_heartbeat >= 1.0:
                lease.persist(active=True, cursor=offset)
                last_heartbeat = time.monotonic()
            assert event_trigger is not None
            event_trigger.wait(timeout=1.0)
    except KeyboardInterrupt:
        return 130
    finally:
        if event_trigger:
            event_trigger.close()
        if lease:
            lease.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bus", type=Path, default=None, help="override bus path")
    authority = parser.add_mutually_exclusive_group()
    authority.add_argument(
        "--print-bus-path",
        action="store_true",
        help="print the canonical runtime-equivalent bus path and exit",
    )
    authority.add_argument(
        "--print-install-interlock-path",
        action="store_true",
        help="print the runtime-equivalent app/install interlock path and exit",
    )
    authority.add_argument(
        "--assert-install-idle",
        action="store_true",
        help="exit zero only when the whole canonical Bus proves installation-safe",
    )
    parser.add_argument(
        "--name", default=None, help="bound agent name; exact snapshot match required"
    )
    parser.add_argument("--all", action="store_true", help="promiscuous: every seal")
    parser.add_argument(
        "--become",
        action="store_true",
        help="hear all until a name assignment, then filter",
    )
    parser.add_argument("--follow", action="store_true", help="tail the bus")
    parser.add_argument(
        "--once", action="store_true", help="print last matching event and exit"
    )
    parser.add_argument(
        "--from-start", action="store_true", help="replay existing lines first"
    )
    parser.add_argument(
        "--drafts", action="store_true", help="also emit draft/revised envelopes"
    )
    parser.add_argument(
        "--provider", help="client id, for example codex or claude-code"
    )
    parser.add_argument(
        "--session", help="stable provider-session id used for cursor recovery"
    )
    parser.add_argument("--lease", help="reattach to an explicit lease id")
    parser.add_argument(
        "--bridge-home", type=Path, default=None, help="override lease/receipt root"
    )
    parser.add_argument("--lease-ttl", type=float, default=DEFAULT_LEASE_TTL_SECONDS)
    parser.add_argument(
        "--active-names",
        action="store_true",
        help="print non-stale active session names and exit",
    )
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--interval", type=float, default=0.15)
    args = parser.parse_args()
    if args.bus is None:
        args.bus = bus_path()
    if args.print_bus_path:
        print(args.bus)
        return 0
    if args.print_install_interlock_path:
        print(install_interlock_path())
        return 0
    if args.assert_install_idle:
        return 0 if installation_idle(args.bus) else 2
    if args.bridge_home is None:
        args.bridge_home = bridge_home()
    if bool(args.provider) != bool(args.session):
        parser.error("--provider and --session must be supplied together")
    if args.lease and not args.provider:
        parser.error("--lease requires --provider and --session")
    if args.lease_ttl <= 0:
        parser.error("--lease-ttl must be positive")
    if args.active_names:
        leases = active_leases(args.bridge_home, args.lease_ttl, clean=True)
        emit(
            {
                "schema": ACTIVE_NAMES_SCHEMA,
                "kind": "active_names",
                "names": sorted(
                    {str(item["name"]) for item in leases if item.get("name")}
                ),
                "leases": [
                    {
                        "lease_id": item.get("lease_id"),
                        "provider": item.get("provider"),
                        "provider_session_id": item.get("provider_session_id"),
                        "name": item.get("name"),
                    }
                    for item in leases
                ],
            }
        )
        return 0
    if args.once and args.follow:
        parser.error("--once and --follow cannot combine")
    if not args.once and not args.follow and not args.from_start:
        args.follow = True
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
