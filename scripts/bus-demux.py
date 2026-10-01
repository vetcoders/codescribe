#!/usr/bin/env python3
"""Named, session-aware follower for the clean Codescribe Transcript Bus.

The helper never opens audio. It reads ``codescribe.transcript.v1`` NDJSON and
emits small agent-bridge envelopes. Product installs run it from the stable
path below, not from a source checkout::

  python3 ~/.codescribe/agent-bridge/runtime/bin/bus-demux.py \
    --provider codex --session <provider-session-id> --name james --drafts --follow

``--provider`` plus ``--session`` enables a collision-safe lease, heartbeat,
and byte cursor. Re-running the same command replays unacknowledged envelopes
and resumes bus consumption. Use --ack with the same provider/session only
after the receiving conversation accepts the complete delivery.
Drafts are useful for live replies; only a ``transcript_sealed`` envelope sets
``state_change_allowed`` to true.

Exact names take precedence. A unique one-edit opening name can match a
registered recipient; competing matches produce a non-executable ambiguity
notice. The original transcript is never rewritten.
"""

from __future__ import annotations

import argparse
import datetime
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
#: Channel lifecycle receipts from the agent channel (open / silence seal).
#: The bridge reads them only as flush boundaries for the coverage-refused
#: safety net below; it never turns them into transcript envelopes.
CHANNEL_SESSION_SCHEMA = "codescribe.channel-session.v1"
#: Clean lifecycle terminal. For the flush net it is the hang-up boundary:
#: a channel closed by a second digit / Fn press is not reopened, so no
#: later channel-session row would ever release its refused take.
SESSION_ENDED = "session_ended"
TERMINAL_SEAL = "record_ledger_terminal_seal"
#: A ledger that refuses terminal finality (incomplete acoustic coverage or
#: pending text recovery) emits no terminal seal at all, and before this net
#: the utterance silently vanished for the agent (observed live 2026-09-29:
#: two Founder takes at 98% and 53% coverage were never delivered). The
#: flush delivers the words with ``coverage: "refused"`` and
#: ``state_change_allowed: false`` — parity with the Stop lane's degraded
#: handoff: the take is delivered but no longer allowed to certify itself.
COVERAGE_REFUSED = "refused"
INSTALL_INTERLOCK_FILENAME = "install-runtime.lock"
AGENT_TURN_LEASE_FILENAME = "agent-turn.lock"
SEALED = "transcript_sealed"
CLI_FILE_VERDICT_SOURCE = "cli_file_verdict"
LIVE_STATUSES = ("utterance_draft", "utterance_revised")
LEASE_SCHEMA = "codescribe.agent-bridge.lease.v1"
ATTACH_SCHEMA = "codescribe.agent-bridge.attach.v1"
EVENT_SCHEMA = "codescribe.agent-bridge.event.v1"
ACTIVE_NAMES_SCHEMA = "codescribe.agent-bridge.active-names.v1"
AGENT_REPLY_SCHEMA = "codescribe.agent-reply.v1"
AUDIENCE_BINDING_SCHEMA = "vc.agent-audience-binding.v1"
AUDIENCE_BINDING_FILENAME = "vc.agent-audience-binding.v1.json"
ATTACH_RECEIPT_SCHEMA = "codescribe.agent-bridge.attach-receipt.v1"
STATUS_SCHEMA = "codescribe.agent-bridge.status.v1"
DEFAULT_LEASE_TTL_SECONDS = 120.0
PLAYBACK_WAIT_SECONDS = 120.0
TAKE_WAIT_SECONDS = 120.0
PLAYBACK_POLL_SECONDS = 0.5
DEFAULT_SPEECH_SPEED = 1.25
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


def agent_turn_lease_path() -> Path:
    # Held shared by the app only while an agent turn streams or runs tools.
    # Same invariant directory as the runtime interlock.
    return install_interlock_path().with_name(AGENT_TURN_LEASE_FILENAME)


def installation_idle(
    path: Path,
    *,
    sealed_is_idle: bool = True,
    cursor: dict[str, Any] | None = None,
    bridge_root: Path | None = None,
) -> bool:
    """True when no current take is in flight.

    One microphone: the live app take is the most recently started app
    session that has not ended or sealed. Historical ``session_started``
    rows without terminals are abandoned (crash / pre-lifecycle-terminal
    buses), not a live recording — treating them as live makes
    ``install-if-idle`` refuse forever after the first missing end.

    CLI ``cli_file_verdict`` sessions may run while the app records and do
    not hold the install flock, so an unpaired CLI session is live — but
    only within :data:`CLI_ABANDONED_AFTER_SECONDS` of its start. A file
    transcription is bounded by its audio; a start that old with no
    terminal is a crashed CLI (a SIGPIPE-killed ``codescribe transcribe |
    head`` left exactly this residue), and, like an abandoned app start,
    it must not refuse installs forever. An unpaired CLI start whose
    ``emitted_at`` is missing or unparseable stays live: abandonment must
    be proven, not presumed.
    Channel buses from the binding file and ``buses/channel-*.jsonl`` share
    this guard. Only ``open`` holds a channel; any other lifecycle state or
    a successor session on that channel ends it, as in the channel consumer.
    Silence alone does not release it. An open channel with no receipt for
    six hours uses the CLI abandonment threshold; unknown timestamps keep
    blocking. Evidence rows refresh the session's last known activity.
    """
    root = bridge_root if bridge_root is not None else bridge_home()
    cursors = cursor if cursor is not None else {}
    channel_cursors = cursors.setdefault("channel_buses", {})
    try:
        paths = {path, *channel_cursors}
        paths.update((root / "buses").glob("channel-*.jsonl"))
        binding_path = root / AUDIENCE_BINDING_FILENAME
        try:
            binding = json.loads(binding_path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            binding = {"schema": AUDIENCE_BINDING_SCHEMA, "bindings": {}}
        if (
            not isinstance(binding, dict)
            or binding.get("schema") != AUDIENCE_BINDING_SCHEMA
            or not isinstance(binding.get("bindings"), dict)
        ):
            return False
        for entry in binding["bindings"].values():
            if not isinstance(entry, dict):
                return False
            raw_bus = entry.get("bus")
            if raw_bus is not None:
                if not isinstance(raw_bus, str) or not raw_bus.strip():
                    return False
                paths.add(Path(os.path.expanduser(raw_bus.strip())))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return False
    idle = True
    for source in sorted(paths):
        state = cursors if source == path else channel_cursors.setdefault(source, {})
        if not source.exists():
            # A disappearing known bus is no terminal receipt. Preserve its
            # open channels until their timestamp proves abandonment.
            if any(
                not _cli_session_abandoned(activity, time.time())
                for _, activity in state.get("open_channels", {}).values()
            ):
                idle = False
            continue
        if not source.is_file():
            return False
        open_cli: dict[str, str | None] = {}
        open_channels: dict[str, tuple[str, str | None]] = {}
        live_app: str | None = None
        try:
            with source.open(encoding="utf-8", errors="strict") as handle:
                stat = os.fstat(handle.fileno())
                identity = (stat.st_dev, stat.st_ino, sealed_is_idle)
                if (
                    state
                    and state.get("identity") == identity
                    and state["offset"] <= stat.st_size
                ):
                    handle.seek(state["offset"])
                    open_cli = dict(state["open_cli"])
                    live_app = state["live_app"]
                    open_channels = dict(state["open_channels"])
                # readline keeps tell() usable: playback can consume only the new
                # tail after its initial scan, instead of rescanning a large bus.
                while True:
                    offset = handle.tell()
                    raw = handle.readline()
                    if not raw:
                        break
                    if not raw.endswith("\n"):
                        raise ValueError("incomplete bus row")
                    raw = raw.strip()
                    if not raw:
                        continue
                    event = json.loads(raw)
                    if not isinstance(event, dict):
                        raise ValueError("bus row is not an object")
                    session_id = event.get("session_id")
                    if event.get("schema") == CHANNEL_SESSION_SCHEMA:
                        channel = str(event.get("channel") or "")
                        if (
                            not channel
                            or not isinstance(session_id, str)
                            or not session_id
                        ):
                            raise ValueError("channel row has no identity")
                        # A newer session on this channel supersedes its old open
                        # receipt, matching _consume_channel_row and orphan seals.
                        open_channels = {
                            sid: value
                            for sid, value in open_channels.items()
                            if value[0] != channel
                        }
                        if str(event.get("state") or "") == "open":
                            emitted = event.get("emitted_at") or event.get("opened_at")
                            open_channels[session_id] = (
                                channel, emitted if isinstance(emitted, str) else None
                            )
                        continue
                    if (
                        event.get("schema") == EVIDENCE_SCHEMA
                        and isinstance(session_id, str)
                        and session_id in open_channels
                    ):
                        channel, _ = open_channels[session_id]
                        emitted = event.get("emitted_at")
                        # Missing or invalid activity never proves abandonment.
                        if isinstance(emitted, str):
                            open_channels[session_id] = (channel, emitted)
                    status = event.get("status")
                    # Speech must wait for session_ended: a channel can seal an
                    # utterance while the microphone keeps capturing the room.
                    if status == SEALED and not sealed_is_idle:
                        continue
                    if status not in ("session_started", "session_ended", SEALED):
                        continue
                    if not isinstance(session_id, str) or not session_id:
                        raise ValueError("lifecycle row has no session")
                    is_cli = event.get("source") == CLI_FILE_VERDICT_SOURCE
                    if status == "session_started":
                        if is_cli:
                            emitted = event.get("emitted_at")
                            open_cli[session_id] = (
                                emitted if isinstance(emitted, str) else None
                            )
                        else:
                            live_app = session_id
                    elif is_cli:
                        open_cli.pop(session_id, None)
                    elif session_id == live_app:
                        live_app = None
                state.update(
                    identity=identity,
                    offset=handle.tell(),
                    open_cli=open_cli,
                    live_app=live_app,
                    open_channels=open_channels,
                )
        except ValueError:
            # Retry the offending row, without replaying the valid prefix of
            # a multi-gigabyte bus while its writer finishes a partial tail.
            state.update(
                identity=identity,
                offset=offset,
                open_cli=open_cli,
                live_app=live_app,
                open_channels=open_channels,
            )
            return False
        except (OSError, UnicodeDecodeError):
            return False
        now = time.time()
        live_cli = [
            session_id
            for session_id, emitted_at in open_cli.items()
            if not _cli_session_abandoned(emitted_at, now)
        ]
        live_channels = any(
            not _cli_session_abandoned(activity, now)
            for _, activity in open_channels.values()
        )
        if live_app is not None or live_cli or live_channels:
            idle = False
    return idle


# A CLI file transcription is bounded by its audio; six hours is generous.
CLI_ABANDONED_AFTER_SECONDS = 6 * 3600.0


def _cli_session_abandoned(emitted_at: str | None, now: float) -> bool:
    """Only a provably old, unpaired CLI start counts as abandoned."""
    if not emitted_at:
        return False
    try:
        started = datetime.datetime.fromisoformat(emitted_at.replace("Z", "+00:00"))
    except ValueError:
        return False
    if started.tzinfo is None:
        started = started.replace(tzinfo=datetime.timezone.utc)
    return now - started.timestamp() > CLI_ABANDONED_AFTER_SECONDS


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


def resolve_recipients(text: str, names: set[str]) -> tuple[set[str], str]:
    """Exact addresses win; a distorted opening vocative must be unique."""
    names = {name.casefold() for name in names if name}
    exact = {name for name in names if addressed_to(text, name)}
    if exact:
        return exact, "exact"
    opening = re.match(
        r"(?i)^\s*(?:(?:hej|cześć|hello|hey)\s*[,!:]?\s+)?([^\W\d_]{4,32})\b",
        text,
    )
    if not opening:
        return set(), "none"
    token = opening.group(1).casefold()

    def one_edit(left: str, right: str) -> bool:
        if abs(len(left) - len(right)) > 1:
            return False
        if len(left) == len(right):
            differences = [
                i for i, pair in enumerate(zip(left, right)) if pair[0] != pair[1]
            ]
            return len(differences) <= 1 or (
                len(differences) == 2
                and differences[1] == differences[0] + 1
                and left[differences[0]] == right[differences[1]]
                and left[differences[1]] == right[differences[0]]
            )
        shorter, longer = sorted((left, right), key=len)
        return any(longer[:i] + longer[i + 1 :] == shorter for i in range(len(longer)))

    candidates = {
        name
        for name in names
        if 4 <= len(name) <= 32
        and any(
            one_edit(token, name + suffix)
            for suffix in ("", "ie", "owi", "a", "em", "u")
        )
    }
    return candidates, "fuzzy" if len(
        candidates
    ) == 1 else "ambiguous" if candidates else "none"


def registered_recipients(root: Path, bus: Path) -> set[str] | None:
    """Offline names still reserve their address; incomplete discovery forbids guessing."""
    names: set[str] = set()
    resolved_bus = str(bus.expanduser().resolve(strict=False))
    try:
        for path in (root / "leases").glob("*.json"):
            value = read_json(path)
            if not value or value.get("schema") != LEASE_SCHEMA:
                return None
            if value.get("bus") != resolved_bus:
                continue
            name = value.get("name")
            if name is not None and not isinstance(name, str):
                return None
            if name:
                names.add(name.casefold())
    except OSError:
        return None
    return names


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
        # A coverage-refused flush delivers the words but not the authority:
        # the ledger declined to certify the take, so the envelope may inform
        # a reply and may not authorize a state change.
        "state_change_allowed": status == SEALED
        and event.get("coverage") != COVERAGE_REFUSED,
    }
    coverage = event.get("coverage")
    if isinstance(coverage, str) and coverage:
        payload["coverage"] = coverage
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
    if event.get("schema") not in (
        CLEAN_SCHEMA,
        EVIDENCE_SCHEMA,
        CHANNEL_SESSION_SCHEMA,
    ):
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


def channel_of_session(session_id: str) -> str | None:
    """The channel digit a session belongs to, from its stable label prefix."""
    match = re.match(r"agent-channel-(\d+)-", session_id or "")
    return match.group(1) if match else None


class EvidenceNormalizer:
    """Translate ``transcript-evidence.v1`` rows into the shape the bridge speaks.

    ``rendered_text`` is an immutable full snapshot from the reducer.  The
    bridge forwards it verbatim; it never infers a delta, ordering, revision,
    or finality from characters.  Terminal rows are coalesced only by the
    reducer's stable terminal phase identity, because one terminal receipt can
    project once per document entry.

    Channel safety net: a ledger that refuses terminal finality emits no
    terminal seal, so an addressed utterance would vanish without a trace.
    The normalizer keeps the latest snapshot per document of each unsealed
    channel session and, when the session is over — a ``channel-session``
    receipt shows the channel moved on (any non-open state for the session,
    or a fresh open of the same channel), or the session's own
    ``session_ended`` lifecycle row marks a hang-up — flushes those documents
    as ``coverage: "refused"`` seal envelopes via :meth:`pop_flushes`.  The
    words are delivered once; certification is honestly withheld.
    """

    #: Unsealed channel sessions retained for the flush net. The quiet
    #: contract reopens a channel within seconds of every real utterance, so
    #: anything beyond a handful of sessions is an abandoned bus replay.
    MAX_TRACKED_SESSIONS = 16

    def __init__(self) -> None:
        self._terminal_seals: set[str] = set()
        # Sessions whose delivery is settled: a terminal seal was reported or
        # the refused flush already carried their words. A late row for such
        # a session must not re-arm the net and deliver the take twice.
        self._settled_sessions: set[str] = set()
        self._session_docs: dict[str, dict[Any, dict[str, Any]]] = {}
        self._flushes: list[dict[str, Any]] = []

    def pop_flushes(self) -> list[dict[str, Any]]:
        """Coverage-refused envelopes triggered by the last normalized row."""
        flushes, self._flushes = self._flushes, []
        return flushes

    def normalize(self, event: dict[str, Any] | None) -> dict[str, Any] | None:
        if event is None:
            return None
        if event.get("schema") == CHANNEL_SESSION_SCHEMA:
            self._consume_channel_row(event)
            return None
        if event.get("schema") != EVIDENCE_SCHEMA:
            if event.get("status") == SESSION_ENDED:
                self._flush_session(
                    str(event.get("session_id") or ""), event.get("emitted_at")
                )
            return event
        document = event.get("rendered_text")
        if not isinstance(document, str):
            return None
        if str(event.get("reducer_action") or "") == TERMINAL_SEAL:
            session = str(event.get("session_id") or "")
            self._settled_sessions.add(session)
            self._session_docs.pop(session, None)
            seal_id = terminal_seal_identity(event)
            if seal_id in self._terminal_seals:
                return None
            self._terminal_seals.add(seal_id)
            return self._as_clean(event, SEALED, document)
        clean = self._as_clean(event, LIVE_STATUSES[1], document)
        self._remember_channel_document(event, clean)
        return clean

    def _remember_channel_document(
        self, event: dict[str, Any], clean: dict[str, Any]
    ) -> None:
        session = str(event.get("session_id") or "")
        if not clean.get("audience") or channel_of_session(session) is None:
            return
        if session in self._settled_sessions:
            return
        docs = self._session_docs.setdefault(session, {})
        docs[clean.get("document_index")] = clean
        while len(self._session_docs) > self.MAX_TRACKED_SESSIONS:
            self._session_docs.pop(next(iter(self._session_docs)))

    def _consume_channel_row(self, row: dict[str, Any]) -> None:
        channel = str(row.get("channel") or "")
        session = str(row.get("session_id") or "")
        due: list[str] = []
        # Only "open" keeps a session alive; a silence seal or any closing
        # state the app writes for this session ends it.
        if str(row.get("state") or "") != "open" and session in self._session_docs:
            due.append(session)
        for cached in list(self._session_docs):
            if (
                cached != session
                and cached not in due
                and channel_of_session(cached) == channel
            ):
                due.append(cached)
        for stale in due:
            self._flush_session(stale, row.get("emitted_at"))

    def _flush_session(self, session: str, emitted_at: Any) -> None:
        docs = self._session_docs.pop(session, None)
        if not docs or session in self._settled_sessions:
            return
        self._settled_sessions.add(session)
        seen_texts: set[str] = set()
        for doc_index, clean in sorted(docs.items(), key=lambda item: str(item[0])):
            text = clean.get("text") or ""
            if not text.strip():
                continue
            # Channel documents often re-project one full-session snapshot
            # under several document indexes; identical words are one refused
            # delivery, not N.
            if text in seen_texts:
                continue
            seen_texts.add(text)
            # A stable per-document identity keys the delivery, so a bus
            # replay after restart is the same refused phase, not a repeat.
            identity = _identity(("coverage-refused-seal", session, doc_index))
            refused = dict(clean)
            refused.update(
                {
                    "status": SEALED,
                    "coverage": COVERAGE_REFUSED,
                    "utterance_id": identity,
                    "source_event_id": identity,
                    "emitted_at": emitted_at or clean.get("emitted_at"),
                }
            )
            self._flushes.append(refused)

    def _as_clean(
        self, event: dict[str, Any], status: str, text: str
    ) -> dict[str, Any]:
        clean = {
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
        audience = event.get("audience")
        if isinstance(audience, str) and audience:
            clean["audience"] = audience
        return clean


def emit(payload: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n")
    sys.stdout.flush()


FOLLOWER_TEXT_LIMIT = 200


def follower_paths(root: Path, lease_id: str) -> tuple[Path, Path]:
    base = root / "runtime" / "followers" / lease_id
    return base.with_suffix(".log"), base.with_suffix(".events.jsonl")


def human_line(payload: dict[str, Any], channel: str | None = None) -> str:
    stamp = str(payload.get("emitted_at") or utc_now())
    try:
        clock = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00")).astimezone()
        when = clock.strftime("%H:%M:%S")
    except ValueError:
        when = datetime.datetime.now().strftime("%H:%M:%S")
    kind = str(payload.get("kind") or "notice")
    audience = str(payload.get("audience") or payload.get("name") or "*")
    who = f"{channel}·{audience}" if channel else audience
    if kind == "attach":
        return f"{when} attach {who} cursor={payload.get('cursor')} follower={payload.get('follower_pid')}"
    if kind == "routing_ambiguity":
        kind = "routing?"
    elif payload.get("coverage") == COVERAGE_REFUSED:
        kind = "refused"
    elif kind == "revised":
        kind = "draft"
    delivery = payload.get("delivery_id")
    identity = f" id={delivery}" if delivery else ""
    revision = payload.get("reducer_revision")
    revision_label = f" (rev {revision})" if kind == "draft" and revision is not None else ""
    words = " ".join(str(payload.get("text") or "").split())[:FOLLOWER_TEXT_LIMIT]
    return f"{when} {kind:<7} {who}{identity}{revision_label} {json.dumps(words, ensure_ascii=False)}"


def emit_follower(
    payload: dict[str, Any], events_path: Path | None, channel: str | None,
    *, human_output: bool = True,
) -> None:
    if events_path is None:
        emit(payload)
        return
    encoded = (json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n").encode("utf-8")
    descriptor = os.open(events_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        remaining = memoryview(encoded)
        while remaining:
            written = os.write(descriptor, remaining)
            if written == 0:
                raise OSError("follower event write made no progress")
            remaining = remaining[written:]
    finally:
        os.close(descriptor)
    if human_output:
        sys.stdout.write(human_line(payload, channel) + "\n")
        sys.stdout.flush()


def consider(
    event: dict[str, Any],
    *,
    name: str | None,
    hear_all: bool,
    drafts: bool,
    debug: bool,
    recipients: set[str] | None = None,
) -> dict[str, Any] | None:
    status = event.get("status")
    if status != SEALED and not (drafts and status in LIVE_STATUSES):
        return None
    text = event.get("text") or ""
    audience = event.get("audience")
    if isinstance(audience, str):
        if audience == "*":
            payload = slim(event, "*")
            payload["routing_match"] = "audience"
            return payload
        if name and audience.casefold() == name.casefold():
            payload = slim(event, name.casefold())
            payload["routing_match"] = "audience"
            return payload
    # Destination recognition never rewrites the transcript. Fuzzy routing
    # requires complete registered-recipient discovery and a unique result.
    addressable = text
    if name and not hear_all and recipients is not None:
        matches, method = resolve_recipients(text, recipients | {name.casefold()})
        if name.casefold() not in matches:
            return None
        if method == "ambiguous":
            payload = slim(event, name.casefold(), kind="routing_ambiguity")
            payload["state_change_allowed"] = False
            payload["routing_candidates"] = sorted(matches)
            return payload
        if method == "fuzzy":
            payload = slim(event, name.casefold())
            payload["routing_match"] = "fuzzy"
            return payload
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
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def read_json(path: Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, OSError):
        return None
    return value if isinstance(value, dict) else None


def lease_identifier(provider: str, provider_session_id: str) -> str:
    # The provider session owns the cursor. Name is mutable during --become and
    # therefore cannot participate in the key: binding a name must not fork the
    # greeting follower onto a fresh cursor.
    identity = "\0".join((provider.casefold(), provider_session_id))
    return hashlib.sha256(identity.encode("utf-8")).hexdigest()[:32]


#: Providers that hand their own conversation id to child processes. Codex
#: exposes none, so its session is never guessed and stays an explicit flag.
PROVIDER_SESSION_ENV = {"claude-code": "CLAUDE_CODE_SESSION_ID"}


def provider_session_from_env(provider: str) -> str | None:
    """The provider's own session id from its environment, when it sets one."""
    key = PROVIDER_SESSION_ENV.get(provider.casefold())
    value = os.environ.get(key, "").strip() if key else ""
    return value or None


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


def active_leases(root: Path, ttl_seconds: float) -> list[dict[str, Any]]:
    """Discover presence without deleting durable identity or recovery cursors.

    A missed heartbeat means offline, not forgotten.
    """
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
            and 0 <= now - float(heartbeat) <= ttl_seconds
        )
        if not value or value.get("schema") != LEASE_SCHEMA or not fresh:
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
        coalesce: bool = False,
    ) -> None:
        self.root = root
        self.provider = provider.casefold()
        self.provider_session_id = provider_session_id
        self.name = name.casefold() if name else None
        self.bus = str(bus.expanduser().resolve(strict=False))
        self.ttl_seconds = ttl_seconds
        self.coalesce = coalesce
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
            if previous is None and self.path.exists():
                raise ValueError(
                    f"lease {self.lease_id} has unreadable recovery state; "
                    "preserved on disk, attachment refused"
                )
            if previous is not None and not self._matches(previous):
                raise ValueError(
                    f"lease {self.lease_id} belongs to a different provider session or bus"
                )
            self.resumed = False
            self.cursor = 0
            self.last_sequence: Any = None
            self.pending: dict[str, dict[str, Any]] = {}
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
                saved_cursor = previous.get("cursor")
                if type(saved_cursor) is not int or saved_cursor < 0:
                    raise ValueError(
                        f"lease {self.lease_id} has an invalid recovery cursor; "
                        "preserved on disk, attachment refused"
                    )
                self.cursor = saved_cursor
                self.last_sequence = previous.get("last_sequence")
                self.name = previous.get("name") or self.name
                pending = previous.get("pending", [])
                if not isinstance(pending, list) or any(
                    not isinstance(payload, dict)
                    or not isinstance(payload.get("delivery_id"), str)
                    or payload.get("lease_id") != self.lease_id
                    or payload.get("provider") != self.provider
                    or payload.get("provider_session_id") != self.provider_session_id
                    or not re.fullmatch(r"[0-9a-f]{24}", payload["delivery_id"])
                    for payload in pending
                ):
                    raise ValueError(
                        "invalid pending deliveries; recovery state preserved"
                    )
                self.pending = {payload["delivery_id"]: payload for payload in pending}
                if len(self.pending) != len(pending):
                    raise ValueError(
                        "duplicate pending identities; recovery state preserved"
                    )
                self.resumed = True
            elif follow_from_end:
                try:
                    self.cursor = bus.stat().st_size
                except FileNotFoundError:
                    self.cursor = 0
            self.persist(active=True)
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
                "pending": list(self.pending.values()),
                "active": active,
                "pid": os.getpid(),
                "heartbeat_unix": time.time(),
                "updated_at": utc_now(),
            },
        )

    def queue_delivery(self, payload: dict[str, Any]) -> bool:
        delivery_id = payload["delivery_id"]
        if delivery_acknowledged(self.root, self.lease_id, delivery_id):
            return False
        if delivery_id in self.pending:
            return False
        if self.coalesce and payload.get("kind") in ("draft", "revised"):
            # A reducer storm re-states one document ~250 times per sentence.
            # Under coalescing the newest revision replaces its predecessors
            # in the mailbox instead of stacking toward the 256 cap; the seal
            # stays a separate envelope so terminal delivery is never merged.
            key = (payload.get("session_id"), payload.get("document_index"))
            stale = [
                queued_id
                for queued_id, item in self.pending.items()
                if item.get("kind") in ("draft", "revised")
                and (item.get("session_id"), item.get("document_index")) == key
            ]
            for queued_id in stale:
                del self.pending[queued_id]
        pending_bytes = sum(
            len(json.dumps(item, ensure_ascii=False).encode("utf-8"))
            for item in self.pending.values()
        )
        if (
            len(self.pending) >= 256
            or pending_bytes
            + len(json.dumps(payload, ensure_ascii=False).encode("utf-8"))
            > 8 * 1024 * 1024
        ):
            raise BufferError(
                "pending mailbox is full; acknowledge received deliveries and resume "
                "this same lease; the unread bus cursor is preserved"
            )
        self.pending[delivery_id] = payload
        self.persist(active=True)
        return True

    def collect_acknowledgments(self) -> None:
        completed = [
            delivery_id
            for delivery_id in self.pending
            if delivery_acknowledged(self.root, self.lease_id, delivery_id)
        ]
        if completed:
            for delivery_id in completed:
                del self.pending[delivery_id]
            self.persist(active=True)

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
        # Terminal evidence projects once per document entry. After restart,
        # a later row is the same delivery phase, not another command. Keep
        # source_event_id intact for provenance while keying that delivery by
        # the same reducer phase used by EvidenceNormalizer.
        phase_id = payload.get("source_event_id")
        if (
            payload.get("producer_schema") == EVIDENCE_SCHEMA
            and payload.get("reducer_action") == TERMINAL_SEAL
            and payload.get("status") == SEALED
        ):
            phase_id = terminal_seal_identity(payload)
        payload["delivery_id"] = _identity(
            (
                "native_bus_demux",
                self.lease_id,
                phase_id,
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
            "follower_pid": os.getpid(),
        }

    def close(self) -> None:
        try:
            self.persist(active=False)
        finally:
            self._release_lock()


def delivery_acknowledged(root: Path, lease_id: str, delivery_id: str) -> bool:
    receipt = read_json(root / "acknowledgments" / lease_id / f"{delivery_id}.json")
    return receipt == {"lease_id": lease_id, "delivery_id": delivery_id}


def acknowledge_delivery(args: argparse.Namespace) -> int:
    """Record receipt of one or more deliveries, all or nothing.

    Every id is checked before any marker is written: one unknown id refuses
    the whole call, so a batch never half-acknowledges the mailbox.
    """
    delivery_ids = list(dict.fromkeys(args.ack))
    if not delivery_ids or any(
        not re.fullmatch(r"[0-9a-f]{24}", delivery_id) for delivery_id in delivery_ids
    ):
        raise ValueError("invalid delivery id; nothing acknowledged")
    lease_id = lease_identifier(args.provider, args.session)
    state = read_json(args.bridge_home / "leases" / f"{lease_id}.json")
    if (
        not state
        or state.get("schema") != LEASE_SCHEMA
        or state.get("lease_id") != lease_id
        or state.get("provider") != args.provider.casefold()
        or state.get("provider_session_id") != args.session
        or (
            getattr(args, "bus_overridden", True)
            and state.get("bus") != str(args.bus.expanduser().resolve(strict=False))
        )
    ):
        raise ValueError(
            "acknowledgment does not belong to this provider session and bus"
        )
    pending = state.get("pending", [])
    pending_ids = (
        {
            payload.get("delivery_id")
            for payload in pending
            if isinstance(payload, dict)
        }
        if isinstance(pending, list)
        else set()
    )
    unread = [
        delivery_id
        for delivery_id in delivery_ids
        if not delivery_acknowledged(args.bridge_home, lease_id, delivery_id)
    ]
    for delivery_id in unread:
        if delivery_id not in pending_ids:
            raise ValueError(
                f"delivery {delivery_id} is not pending for this provider session; "
                "nothing acknowledged"
            )
    for delivery_id in unread:
        atomic_json(
            args.bridge_home / "acknowledgments" / lease_id / f"{delivery_id}.json",
            {"lease_id": lease_id, "delivery_id": delivery_id},
        )
    for delivery_id in delivery_ids:
        emit({"kind": "acknowledged", "lease_id": lease_id, "delivery_id": delivery_id})
    return 0


def fire_seal_hook(command: str, payload: dict[str, Any]) -> None:
    """Detached wake hook, exactly once per freshly queued seal.

    The hook must never block or kill the delivery loop: it runs in its own
    session with the seal's identity in the environment, and a hook that
    cannot spawn is reported on stderr rather than raised.
    """
    import subprocess

    environment = dict(os.environ)
    environment["CODESCRIBE_SEAL_DELIVERY_ID"] = str(payload.get("delivery_id") or "")
    environment["CODESCRIBE_SEAL_SESSION_ID"] = str(payload.get("session_id") or "")
    environment["CODESCRIBE_SEAL_TEXT"] = str(payload.get("text") or "")
    try:
        subprocess.Popen(
            ["/bin/sh", "-c", command],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError as error:
        sys.stderr.write(f"bus-demux: on-seal hook failed to spawn: {error}\n")


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
    events_path: Path | None = getattr(args, "follower_events", None)
    follower_channel: str | None = getattr(args, "follower_channel", None)
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
                coalesce=bool(args.coalesce),
            )
        except (OSError, RuntimeError, ValueError) as error:
            sys.stderr.write(f"bus-demux: session lease refused: {error}\n")
            return 3
        if lease.name:
            name = lease.name
            hear_all = False
        emit_follower(lease.attach_receipt(), events_path, follower_channel)

    # One normalizer for the whole run: the evidence grain is stateful (it
    # remembers each session's document and whether its seal was reported), and
    # a fresh one per line would re-emit the entire document every time.
    normalizer = EvidenceNormalizer()
    event_trigger: BusEventTrigger | None = None
    deferred: tuple[list[dict[str, Any]], int | None] | None = None
    recipients: set[str] | None = None
    human_drafts: dict[tuple[Any, Any], dict[str, Any]] = {}

    def draft_key(payload: dict[str, Any]) -> tuple[Any, Any]:
        return payload.get("session_id"), payload.get("document_index")

    def flush_human_drafts(key: tuple[Any, Any] | None = None) -> None:
        keys = [key] if key is not None else list(human_drafts)
        for current in keys:
            draft = human_drafts.pop(current, None)
            if draft is not None:
                sys.stdout.write(human_line(draft, follower_channel) + "\n")
        sys.stdout.flush()

    def publish(payload: dict[str, Any]) -> None:
        if events_path is not None and lease and lease.coalesce:
            key = draft_key(payload)
            if payload.get("kind") in ("draft", "revised"):
                emit_follower(payload, events_path, follower_channel, human_output=False)
                human_drafts[key] = payload
                return
            flush_human_drafts(key)
        emit_follower(payload, events_path, follower_channel)

    def deliver(payloads: list[dict[str, Any]], next_cursor: int | None) -> None:
        nonlocal deferred
        remaining = list(payloads)
        try:
            while remaining:
                payload = remaining[0]
                if not lease or lease.queue_delivery(payload):
                    publish(payload)
                    if args.on_seal and payload.get("kind") == "seal":
                        fire_seal_hook(args.on_seal, payload)
                remaining.pop(0)
        except BufferError:
            # Preserve the already normalized envelopes. Re-normalizing their
            # raw evidence rows would suppress a terminal phase — or a
            # coverage-refused flush — on retry.
            deferred = (remaining, next_cursor)
            raise
        if lease and next_cursor is not None:
            lease.persist(
                active=True,
                cursor=next_cursor,
                sequence=payloads[-1].get("sequence") if payloads else None,
            )
        deferred = None

    def handle(raw: str, next_cursor: int | None = None) -> None:
        nonlocal name, hear_all
        event = normalizer.normalize(parse_line(raw))
        # Coverage-refused flushes precede the row that triggered them: they
        # carry utterances older than the channel receipt on this line.
        events = normalizer.pop_flushes()
        if event is not None:
            events.append(event)
        payloads: list[dict[str, Any]] = []
        for event in events:
            payload = consider(
                event,
                name=name,
                hear_all=hear_all,
                drafts=args.drafts,
                debug=args.debug,
                recipients=recipients,
            )
            if payload is None:
                continue
            if args.become and payload.get("kind") == "name_assignment" and not name:
                name = str(payload["name"])
                hear_all = False
                if lease:
                    lease.bind_name(name)
                sys.stderr.write(f"bus-demux: bound name={name}\n")
            if lease:
                lease.enrich(payload)
            payloads.append(payload)
        if not payloads:
            if lease and next_cursor is not None:
                lease.persist(
                    active=True,
                    cursor=next_cursor,
                    sequence=events[-1].get("sequence") if events else None,
                )
            return
        # Read progress is independent of receipt. The original envelopes are
        # made durable before emission and remain pending until acknowledged.
        deliver(payloads, next_cursor)

    try:
        if lease:
            lease.collect_acknowledgments()
            for payload in lease.pending.values():
                publish(payload)
            flush_human_drafts()
        if args.once:
            last = None
            recipients = registered_recipients(args.bridge_home, path)
            for raw in replay(path):
                event = normalizer.normalize(parse_line(raw))
                events = normalizer.pop_flushes()
                if event is not None:
                    events.append(event)
                for event in events:
                    payload = consider(
                        event,
                        name=name,
                        hear_all=hear_all,
                        drafts=args.drafts,
                        debug=False,
                        recipients=recipients,
                    )
                    if payload is not None:
                        last = payload
            if last is None:
                return 1
            if lease:
                lease.enrich(last)
            if not lease or lease.queue_delivery(last):
                publish(last)
            flush_human_drafts()
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
        waiting_for_ack = False
        while True:
            if lease:
                lease.collect_acknowledgments()
            try:
                if deferred is not None:
                    deliver(*deferred)
                    assert lease is not None
                    offset = lease.cursor
                previous_offset = offset
                entries, offset = iter_new_lines(path, offset)
                if entries:
                    recipients = registered_recipients(args.bridge_home, path)
                for raw, next_cursor in entries:
                    handle(raw, next_cursor)
                flush_human_drafts()
                if lease and not entries and offset != previous_offset:
                    lease.persist(active=True, cursor=offset)
            except BufferError:
                if not args.follow:
                    raise
                assert lease is not None
                offset = lease.cursor
                if not waiting_for_ack:
                    sys.stderr.write(
                        "bus-demux: mailbox full; waiting for acknowledgment\n"
                    )
                waiting_for_ack = True
            else:
                waiting_for_ack = False
            if not args.follow:
                return 0
            if lease and time.monotonic() - last_heartbeat >= 1.0:
                lease.persist(active=True, cursor=offset)
                last_heartbeat = time.monotonic()
            assert event_trigger is not None
            event_trigger.wait(timeout=1.0)
    except KeyboardInterrupt:
        return 130
    except BufferError as error:
        sys.stderr.write(f"bus-demux: {error}\n")
        return 4
    finally:
        if event_trigger:
            event_trigger.close()
        if lease:
            lease.close()


def _xai_speech_key() -> str | None:
    """OAuth from the grok CLI's OIDC session first, Keychain API key as fallback."""
    import subprocess

    try:
        data = json.loads((Path.home() / ".grok" / "auth.json").read_text())
        for value in data.values():
            if (
                isinstance(value, dict)
                and value.get("auth_mode") == "oidc"
                and value.get("key")
            ):
                return str(value["key"])
    except (OSError, ValueError):
        pass
    probe = subprocess.run(
        [
            "security",
            "find-generic-password",
            "-s",
            "com.vetcoders.codescribe",
            "-a",
            "LLM_XAI_API_KEY",
            "-w",
        ],
        capture_output=True,
        text=True,
    )
    key = probe.stdout.strip()
    return key or None


def _openai_speech_key() -> str | None:
    """Explicit env first, then a plain Keychain item — never the app bundle.

    Codex OAuth carries no public audio permission, so the OpenAI speaker is
    key-only. The app's Keychain bundle stays app-private by design; this
    helper reads only the surfaces meant for external tools.
    """
    import subprocess

    env_key = os.environ.get("LLM_OPENAI_API_KEY", "").strip()
    if env_key:
        return env_key
    probe = subprocess.run(
        [
            "security",
            "find-generic-password",
            "-s",
            "com.vetcoders.codescribe",
            "-a",
            "LLM_OPENAI_API_KEY",
            "-w",
        ],
        capture_output=True,
        text=True,
    )
    key = probe.stdout.strip()
    return key or None


VOICES_FILENAME = "voices.json"
DEFAULT_VOICE = "leo"


def _stored_voice(root: Path, name: str | None) -> str | None:
    """The flat ``bindings`` voice for a name, when the store has one."""
    if name:
        mapping = read_json(root / VOICES_FILENAME) or {}
        bindings = mapping.get("bindings", mapping)
        voice = bindings.get(str(name).lower()) if isinstance(bindings, dict) else None
        if isinstance(voice, str) and voice:
            return voice
    return None


def voice_profile_with_source(
    root: Path, name: str | None
) -> tuple[dict[str, Any], str]:
    """Voice profile plus where its voice came from: ``profile`` or ``default``.

    `profiles.<name>` (voice, speed, style, provider) wins over the flat
    `bindings` entry; both fall back to the male default so an unprofiled
    agent still speaks.
    """
    profile: dict[str, Any] = {}
    if name:
        mapping = read_json(root / VOICES_FILENAME) or {}
        profiles = mapping.get("profiles")
        if isinstance(profiles, dict):
            candidate = profiles.get(str(name).lower())
            if isinstance(candidate, dict):
                profile = dict(candidate)
    source = "profile"
    if not isinstance(profile.get("voice"), str) or not profile.get("voice"):
        stored = _stored_voice(root, name)
        profile["voice"] = stored or DEFAULT_VOICE
        source = "profile" if stored else "default"
    speed = profile.get("speed")
    if not isinstance(speed, (int, float)) or speed <= 0:
        profile["speed"] = DEFAULT_SPEECH_SPEED
    return profile, source


def voice_profile(root: Path, name: str | None) -> dict[str, Any]:
    """Full voice profile for a name from <bridge-home>/voices.json.

    The Founder's approved profiles are the single voice truth — callers must
    not hardcode a voice around this.
    """
    return voice_profile_with_source(root, name)[0]


def _voice_store(path: Path) -> dict[str, Any]:
    """Current voices.json content; an unreadable store refuses, never resets."""
    if not path.exists():
        return {}
    state = read_json(path)
    if state is None or (
        "profiles" in state and not isinstance(state.get("profiles"), dict)
    ):
        raise OSError("voice profiles are unreadable or invalid; nothing changed")
    return state


def write_voice_profile(
    root: Path,
    name: str,
    *,
    voice: str | None,
    speed: float | None,
    vendor: str | None,
) -> Path:
    """Merge one name's voice profile into voices.json; other profiles stay.

    Same convention as :func:`write_channel_binding`: an exclusive lock on a
    stable sibling around read/merge/atomic replace, so concurrent attaches
    cannot drop each other's profiles.
    """
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = root / VOICES_FILENAME
    with open(path.with_suffix(".lock"), "a+b") as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        state = _voice_store(path)
        profiles = dict(state.get("profiles") or {})
        key = name.lower()
        entry = profiles.get(key)
        entry = dict(entry) if isinstance(entry, dict) else {}
        if voice:
            entry["voice"] = voice
        if speed is not None:
            entry["speed"] = speed
        if vendor:
            entry["provider"] = vendor
        profiles[key] = entry
        state["profiles"] = profiles
        atomic_json(path, state)
    return path


#: Bounded read of a refused TTS response, for classification only.
TTS_ERROR_BODY_BYTES = 8192
#: Error-code field markers. Only these named fields of a JSON error body are
#: inspected, and only the resulting enum ever leaves the classifier: a vendor
#: body can quote the rejected credential back.
TTS_QUOTA_MARKERS = (
    "spending-limit",
    "spending limit",
    "insufficient_quota",
    "credits",
    "quota",
)
TTS_CREDENTIAL_MARKERS = (
    "could not be validated",
    "invalid_api_key",
    "invalid api key",
    "incorrect api key",
    "invalid_token",
    "unauthenticated",
    "unauthorized",
)


def tts_failure_reason(status: int, body: bytes) -> str:
    """``quota_exhausted`` | ``credential_rejected`` | ``http_<status>``.

    xAI answers both an unvalidated token and an exhausted spending limit
    with 403 (two Founder replies failed that way on 2026-09-29 as a bare
    "tts request failed (403)"), so the status alone cannot tell the agent
    whether to re-login or to top up.
    """
    fields: list[str] = []

    def collect(node: Any, depth: int) -> None:
        # Vendors nest at most one ``error`` object; deeper is not an error code.
        if not isinstance(node, dict) or depth > 2:
            return
        for key in ("code", "error", "message", "type"):
            value = node.get(key)
            if isinstance(value, str):
                fields.append(value.casefold())
            elif isinstance(value, dict):
                collect(value, depth + 1)

    try:
        collect(json.loads(body.decode("utf-8", errors="replace")), 0)
    except (ValueError, RecursionError):
        pass
    marker_text = "\n".join(fields)
    if any(marker in marker_text for marker in TTS_QUOTA_MARKERS):
        return "quota_exhausted"
    if status == 401 or any(
        marker in marker_text for marker in TTS_CREDENTIAL_MARKERS
    ):
        return "credential_rejected"
    return f"http_{status}"


def _tts_exchange(request: Any) -> tuple[bytes | None, str | None, str | None]:
    """POST one TTS request: (pcm, None, None) or (None, error, reason).

    Install only TLS transport: no file handler, proxy or redirect handler.
    Certificate and hostname verification are enabled explicitly by the
    default TLS context. The error string names the status or exception
    class only; the reason is an enum. Neither echoes the body.
    """
    import http.client
    import ssl
    import urllib.error
    import urllib.request

    opener = urllib.request.OpenerDirector()
    opener.add_handler(
        urllib.request.HTTPSHandler(context=ssl.create_default_context())
    )
    try:
        with opener.open(request, timeout=60) as response:
            if not 200 <= response.status < 300:
                try:
                    detail = response.read(TTS_ERROR_BODY_BYTES)
                except (OSError, http.client.HTTPException):
                    detail = b""
                return (
                    None,
                    f"tts request failed ({response.status})",
                    tts_failure_reason(response.status, detail),
                )
            return response.read(), None, None
    except (OSError, urllib.error.URLError, http.client.HTTPException) as error:
        return None, f"tts request failed ({error.__class__.__name__})", "network"


def _speak_xai(
    text: str,
    voice: str,
    speed: float,
    *,
    playback_root: Path | None = None,
    bus: Path | None = None,
) -> tuple[bool, str | None, str | None]:
    """Same TTS lane as the app (api.x.ai/v1/tts, PCM s16le 24 kHz), played via afplay."""
    import urllib.request

    key = _xai_speech_key()
    if not key:
        return (
            False,
            "no xAI credential (run: grok login, or Keychain LLM_XAI_API_KEY)",
            "credential_missing",
        )
    body = json.dumps(
        {
            "text": text,
            "voice_id": voice,
            "language": "auto",
            "output_format": {"codec": "pcm", "sample_rate": 24000},
            "speed": speed,
        }
    ).encode("utf-8")
    # The URL stays literal where the Request is built. Headers ride on the
    # Request: opener.addheaders lose to the default Content-Type that
    # do_request_ installs first for any request with data, and the API
    # rejects a JSON body labelled x-www-form-urlencoded (415).
    request = urllib.request.Request(
        "https://api.x.ai/v1/tts",
        data=body,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
    )
    return _speak_pcm(*_tts_exchange(request), playback_root=playback_root, bus=bus)


def _speak_pcm(
    pcm: bytes | None,
    error: str | None,
    reason: str | None,
    *,
    playback_root: Path | None = None,
    bus: Path | None = None,
) -> tuple[bool, str | None, str | None]:
    if pcm is None:
        return False, error, reason
    return _play_pcm_24k(pcm, playback_root=playback_root, bus=bus)


def _acquire_playback_lock(descriptor: int) -> bool:
    """Bound the wait with a monotonic clock; the kernel owns exclusivity."""
    deadline = time.monotonic() + PLAYBACK_WAIT_SECONDS
    while True:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except BlockingIOError:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            time.sleep(min(0.05, remaining))


def _wait_for_take_end(
    bus: Path, cursor: dict[str, Any], *, bridge_root: Path | None = None
) -> bool:
    deadline = time.monotonic() + TAKE_WAIT_SECONDS
    while not installation_idle(
        bus, sealed_is_idle=False, cursor=cursor, bridge_root=bridge_root
    ):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        time.sleep(min(PLAYBACK_POLL_SECONDS, remaining))
    return True


def _stop_playback(player: Any) -> None:
    import subprocess

    player.terminate()
    try:
        player.wait(timeout=2)
    except subprocess.TimeoutExpired:
        player.kill()
        player.wait()


def _play_pcm_24k(
    pcm: bytes,
    *,
    playback_root: Path | None = None,
    bus: Path | None = None,
) -> tuple[bool, str | None, str | None]:
    """Prepare WAV independently, then serialize only playback across agents."""
    import subprocess
    import tempfile
    import wave

    wav_path = None
    lock_descriptor = None
    player = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as handle:
            wav_path = handle.name
            with wave.open(handle, "wb") as sink:
                sink.setnchannels(1)
                sink.setsampwidth(2)
                sink.setframerate(24000)
                sink.writeframes(pcm)
        root = playback_root if playback_root is not None else bridge_home()
        runtime = root / "runtime"
        runtime.mkdir(parents=True, exist_ok=True)
        # Never unlink this file: all current and waiting processes must share
        # the same inode even when the runtime payload is replaced.
        lock_descriptor = os.open(
            runtime / "playback.lock", os.O_RDWR | os.O_CREAT, 0o600
        )
        if not _acquire_playback_lock(lock_descriptor):
            return False, "playback wait timed out", "playback_busy"
        # A take may have started during synthesis or the lock wait.
        speech_bus = bus if bus is not None else bus_path()
        cursor: dict[str, Any] = {}
        if not _wait_for_take_end(speech_bus, cursor, bridge_root=root):
            return False, "live take wait timed out", "take_live"
        player = subprocess.Popen(
            ["afplay", wav_path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        while True:
            if not installation_idle(
                speech_bus, sealed_is_idle=False, cursor=cursor, bridge_root=root
            ):
                _stop_playback(player)
                return False, "take started during playback", "take_started"
            result = player.poll()
            if result is not None:
                if result != 0:
                    return False, "afplay failed", "playback_failed"
                break
            time.sleep(PLAYBACK_POLL_SECONDS)
    except OSError as error:
        return False, f"playback failed ({error.__class__.__name__})", "playback_failed"
    finally:
        if player is not None and player.poll() is None:
            _stop_playback(player)
        if lock_descriptor is not None:
            os.close(lock_descriptor)
        if wav_path:
            try:
                os.unlink(wav_path)
            except OSError:
                pass
    return True, None, None


def _speak_openai(
    text: str,
    voice: str,
    speed: float,
    *,
    playback_root: Path | None = None,
    bus: Path | None = None,
) -> tuple[bool, str | None, str | None]:
    """Same TTS lane as the app (api.openai.com/v1/audio/speech, PCM s16le 24 kHz)."""
    import urllib.request

    key = _openai_speech_key()
    if not key:
        return (
            False,
            "no OpenAI credential (add Keychain item LLM_OPENAI_API_KEY or export it)",
            "credential_missing",
        )
    model = (
        os.environ.get("SPEECH_TTS_MODEL_OPENAI", "").strip()
        or "gpt-4o-mini-tts-2025-12-15"
    )
    body = json.dumps(
        {
            "model": model,
            "input": text,
            "voice": voice,
            "response_format": "pcm",
            "speed": speed,
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        "https://api.openai.com/v1/audio/speech",
        data=body,
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
    )
    return _speak_pcm(*_tts_exchange(request), playback_root=playback_root, bus=bus)


def lease_name(root: Path, provider: str, session: str) -> str | None:
    """The name a provider session's lease already carries, if any."""
    lease_id = lease_identifier(provider, session)
    state = read_json(root / "leases" / f"{lease_id}.json")
    if (
        not state
        or state.get("schema") != LEASE_SCHEMA
        or state.get("provider") != provider.casefold()
        or state.get("provider_session_id") != session
    ):
        return None
    name = state.get("name")
    return name if isinstance(name, str) and name else None


def say_reply(args: argparse.Namespace) -> int:
    """Append one agent-reply row to the canonical Bus, then speak it.

    The Bus is the canonical relay for agent replies; xAI is only the speaker,
    so a failed synthesis still lands the row (spoken=false with the error
    and a ``reason`` enum the agent can act on).
    """
    profile = voice_profile(args.bridge_home, args.name)
    voice = args.voice or str(profile["voice"])
    speed = args.speed if args.speed is not None else float(profile["speed"])
    vendor = args.tts_vendor or str(profile.get("provider") or "xai")
    reply: dict[str, Any] = {
        "schema": AGENT_REPLY_SCHEMA,
        "kind": "agent_reply",
        "emitted_at": utc_now(),
        "reply_id": os.urandom(12).hex(),
        "name": args.name,
        "provider": args.provider,
        "provider_session_id": args.session,
        "text": args.say,
        "voice": voice,
        "speed": speed,
        "tts_vendor": vendor,
        "spoken": False,
    }
    speaker = _speak_openai if vendor == "openai" else _speak_xai
    spoken, error, reason = speaker(
        args.say, voice, speed, playback_root=args.bridge_home, bus=bus_path()
    )
    reply["spoken"] = spoken
    if error:
        reply["tts_error"] = error
    if reason:
        reply["reason"] = reason
    line = json.dumps(reply, ensure_ascii=False) + "\n"
    descriptor = os.open(args.bus, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o644)
    try:
        os.write(descriptor, line.encode("utf-8"))
    finally:
        os.close(descriptor)
    emit(reply)
    return 0 if spoken else 5


def follower_pidfile(root: Path, lease_id: str) -> Path:
    return root / "runtime" / "followers" / f"{lease_id}.pid"


def live_follower_pid(root: Path, lease_id: str) -> int | None:
    """Live follower pid for a lease: pidfile first, then the lease heartbeat.

    A manually started follower has no pidfile but still owns the lease lock;
    spawning next to it would only produce a child that loses the lock and
    dies, so the lease's own fresh heartbeat also counts as a live follower.
    """
    value = read_json(follower_pidfile(root, lease_id))
    pid = value.get("pid") if isinstance(value, dict) else None
    if isinstance(pid, int) and process_is_alive(pid):
        return pid
    state = read_json(root / "leases" / f"{lease_id}.json")
    if isinstance(state, dict) and state.get("active") is True:
        heartbeat = state.get("heartbeat_unix")
        lease_pid = state.get("pid")
        if (
            isinstance(heartbeat, (int, float))
            and time.time() - float(heartbeat) <= DEFAULT_LEASE_TTL_SECONDS
            and isinstance(lease_pid, int)
            and process_is_alive(lease_pid)
        ):
            return lease_pid
    return None


def write_channel_binding(
    root: Path,
    channel: str,
    name: str,
    provider: str,
    provider_session_id: str,
    bus: str | None = None,
) -> Path:
    """Claim a channel without replacing another session's routing."""
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = root / AUDIENCE_BINDING_FILENAME
    # Lock a stable sibling: atomic_json replaces the data file's inode.
    with open(path.with_suffix(".lock"), "a+b") as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        state = read_json(path)
        if path.exists() and (
            not isinstance(state, dict)
            or state.get("schema") != AUDIENCE_BINDING_SCHEMA
            or not isinstance(state.get("bindings"), dict)
        ):
            raise OSError("channel bindings are unreadable or invalid; nothing changed")
        bindings = state["bindings"] if state else {}
        requested = {
            "audience": name.casefold(),
            "provider": provider.casefold(),
            "provider_session_id": provider_session_id,
        }
        entry = dict(requested)
        if bus:
            entry["bus"] = bus
        current = bindings.get(str(channel))
        if str(channel) in bindings:
            # Occupancy is decided by identity alone; the bus is routing and
            # may be updated for the same owner.
            if not isinstance(current, dict) or any(
                current.get(key) != value for key, value in requested.items()
            ):
                owner = (
                    current.get("audience", "unknown")
                    if isinstance(current, dict)
                    else "unknown"
                )
                free = ", ".join(
                    str(slot) for slot in range(1, 10) if str(slot) not in bindings
                )
                raise OSError(
                    f"channel {channel} is occupied by {owner}; "
                    f"free channels: {free or 'none'}; nothing changed"
                )
            if not bus or current.get("bus") == bus:
                return path
        bindings[str(channel)] = entry
        atomic_json(path, {"schema": AUDIENCE_BINDING_SCHEMA, "bindings": bindings})
    return path


def attach_command(args: argparse.Namespace) -> int:
    """One command from zero to a listening channel: binding, follower, receipt.

    Writes the audience binding for the channel, ensures exactly one follower
    owns this provider session's lease (spawning a coalescing one when none is
    alive), and prints a receipt with the lease, cursor, voice profile and
    follower pid. ``--voice`` / ``--speed`` / ``--tts-vendor`` persist into
    this name's profile in voices.json, so every later ``--say`` speaks with
    it. The receipt alone never proves listening — only a take that lands in
    the mailbox does, so callers verify with a fresh seal before claiming
    they hear anything.
    """
    import signal
    import subprocess

    root: Path = args.bridge_home
    name = args.name.casefold()
    persist_voice = bool(args.voice or args.speed is not None or args.tts_vendor)
    voice_source = voice_profile_with_source(root, name)[1]
    if persist_voice:
        # Refuse an unreadable profile store before claiming the channel, so a
        # refusal still means nothing changed.
        _voice_store(root / VOICES_FILENAME)
    if args.voice:
        voice_source = "flag"
    if not getattr(args, "bus_overridden", False):
        # W5: every channel owns a dedicated bus so one follower never chews
        # another audience's rows. An explicit --bus keeps the caller's word.
        channel_bus = root / "buses" / f"channel-{args.channel}.jsonl"
        channel_bus.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if not channel_bus.exists():
            descriptor = os.open(channel_bus, os.O_WRONLY | os.O_CREAT, 0o600)
            os.close(descriptor)
        args.bus = channel_bus
    resolved_bus = str(Path(args.bus).expanduser().resolve(strict=False))
    binding_path = write_channel_binding(
        root, args.channel, name, args.provider, args.session, bus=resolved_bus
    )
    if persist_voice:
        write_voice_profile(
            root, name, voice=args.voice, speed=args.speed, vendor=args.tts_vendor
        )
    lease_id = lease_identifier(args.provider, args.session)
    lease_path = root / "leases" / f"{lease_id}.json"
    resumed = lease_path.exists()
    pid = live_follower_pid(root, lease_id)
    lease_state = read_json(lease_path)
    if (
        isinstance(lease_state, dict)
        and lease_state.get("schema") == LEASE_SCHEMA
        and lease_state.get("bus") not in (None, resolved_bus)
    ):
        # The session migrates onto this channel's dedicated bus: retire the
        # follower that sits on the old bus and restart the byte cursor on the
        # new file. Pending deliveries and acknowledgment markers survive.
        if pid is not None:
            os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline and process_is_alive(pid):
                time.sleep(0.1)
            if process_is_alive(pid):
                sys.stderr.write(
                    "bus-demux: attach failed: the follower on the old bus "
                    f"(pid={pid}) did not exit; nothing changed\n"
                )
                return 3
            pid = None
        lease_state["bus"] = resolved_bus
        lease_state["cursor"] = 0
        lease_state["active"] = False
        atomic_json(lease_path, lease_state)
    spawned = False
    log_path, events_path = follower_paths(root, lease_id)
    errors_path = log_path.with_suffix(".errors.log")
    if pid is None:
        command = [
            sys.executable,
            os.path.abspath(__file__),
            "--bus",
            str(args.bus),
            "--bridge-home",
            str(root),
            "--provider",
            args.provider,
            "--session",
            args.session,
            "--name",
            name,
            "--drafts",
            "--follow",
            "--coalesce",
            "--follower-events",
            str(events_path),
            "--follower-channel",
            str(args.channel),
        ]
        if args.on_seal:
            command += ["--on-seal", args.on_seal]
        log_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        log_descriptor = os.open(log_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        os.fchmod(log_descriptor, 0o600)
        errors_descriptor = os.open(errors_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        os.fchmod(errors_descriptor, 0o600)
        with os.fdopen(log_descriptor, "ab") as log, os.fdopen(errors_descriptor, "ab") as errors:
            child = subprocess.Popen(
                command,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=errors,
                start_new_session=True,
            )
        atomic_json(
            follower_pidfile(root, lease_id),
            {"lease_id": lease_id, "pid": child.pid, "started_at": utc_now()},
        )
        pid = child.pid
        spawned = True
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            state = read_json(lease_path)
            if (
                state
                and state.get("schema") == LEASE_SCHEMA
                and state.get("pid") == pid
                and events_path.exists()
            ):
                break
            if not process_is_alive(pid):
                sys.stderr.write(
                    "bus-demux: attach failed: follower exited during startup; "
                    f"see {errors_path}\n"
                )
                return 3
            time.sleep(0.1)
    state = read_json(lease_path) or {}
    emit(
        {
            "schema": ATTACH_RECEIPT_SCHEMA,
            "kind": "attach_receipt",
            "channel": str(args.channel),
            "audience": name,
            "provider": args.provider.casefold(),
            "provider_session_id": args.session,
            "lease_id": lease_id,
            "cursor": state.get("cursor"),
            "resumed": resumed,
            "follower_pid": pid,
            "follower_spawned": spawned,
            "follower_log": str(log_path),
            "follower_events": str(events_path),
            "coalesce_requested": True,
            "on_seal_hook": bool(args.on_seal),
            "voice": voice_profile(root, name),
            "voice_source": voice_source,
            "voices_file": "present"
            if (root / VOICES_FILENAME).exists()
            else "missing",
            "binding_path": str(binding_path),
        }
    )
    return 0


def status_command(args: argparse.Namespace) -> int:
    """One truthful read of a session's channel.

    The lease file keeps acknowledged envelopes until the follower's next
    sweep, so its raw pending length overstates the backlog; the marker
    store is the receipt truth and the backlog here is pending minus markers.
    """
    root: Path = args.bridge_home
    lease_id = lease_identifier(args.provider, args.session)
    log_path, events_path = follower_paths(root, lease_id)
    state = read_json(root / "leases" / f"{lease_id}.json")
    pending = state.get("pending") if isinstance(state, dict) else None
    if not isinstance(pending, list):
        pending = []
    try:
        markers = {
            entry[: -len(".json")]
            for entry in os.listdir(root / "acknowledgments" / lease_id)
            if entry.endswith(".json")
        }
    except OSError:
        markers = set()
    unacked = [
        item
        for item in pending
        if isinstance(item, dict) and item.get("delivery_id") not in markers
    ]
    seals = [item for item in unacked if item.get("kind") == "seal"]
    last_seal = max(
        seals, key=lambda item: str(item.get("emitted_at") or ""), default=None
    )
    heartbeat = state.get("heartbeat_unix") if isinstance(state, dict) else None
    lease_pid = state.get("pid") if isinstance(state, dict) else None
    follower_alive = bool(
        isinstance(heartbeat, (int, float))
        and time.time() - float(heartbeat) <= args.lease_ttl
        and process_is_alive(lease_pid)
    )
    channel = None
    binding = read_json(root / AUDIENCE_BINDING_FILENAME) or {}
    bindings = binding.get("bindings")
    if isinstance(bindings, dict):
        for slot, entry in bindings.items():
            if (
                isinstance(entry, dict)
                and entry.get("provider_session_id") == args.session
            ):
                channel = str(slot)
                break
    name = state.get("name") if isinstance(state, dict) else None
    emit(
        {
            "schema": STATUS_SCHEMA,
            "kind": "status",
            "lease_id": lease_id,
            "follower_log": str(log_path),
            "follower_events": str(events_path),
            "attached": state is not None,
            "channel": channel,
            "name": name,
            "follower_alive": follower_alive,
            "follower_pid": lease_pid if follower_alive else None,
            "pending_file": len(pending),
            "acknowledged_markers": len(markers),
            "backlog": len(unacked),
            "unacked_seals": len(seals),
            "last_seal": (
                {
                    "delivery_id": last_seal.get("delivery_id"),
                    "emitted_at": last_seal.get("emitted_at"),
                    "text": str(last_seal.get("text") or "")[:160],
                }
                if last_seal
                else None
            ),
            "voice": voice_profile(root, name if isinstance(name, str) else None),
            "cursor": state.get("cursor") if isinstance(state, dict) else None,
        }
    )
    return 0


WATCH_TEXT_LIMIT = 500


def watch_line(payload: Any, lease_id: str | None) -> dict[str, Any] | None:
    """One compact monitor line for an envelope worth waking the agent for.

    Drafts and revisions stay in the mailbox. The watch surfaces seals
    (certified or coverage-refused), anything allowed to change state, and
    routing-ambiguity notices that need the Founder to re-address.
    """
    if not isinstance(payload, dict) or payload.get("schema") != EVENT_SCHEMA:
        return None
    if lease_id and payload.get("lease_id") not in (None, lease_id):
        return None
    if not (
        payload.get("status") == SEALED
        or payload.get("state_change_allowed") is True
        or payload.get("kind") == "routing_ambiguity"
        or payload.get("coverage") == COVERAGE_REFUSED
    ):
        return None
    return {
        "kind": payload.get("kind"),
        "status": payload.get("status"),
        "coverage": payload.get("coverage"),
        "sca": payload.get("state_change_allowed") is True,
        "delivery_id": payload.get("delivery_id"),
        "text": str(payload.get("text") or "")[:WATCH_TEXT_LIMIT],
    }


def watch_command(args: argparse.Namespace) -> int:
    """Compact, line-buffered monitor of one session's follower output.

    Reads the follower events that ``--attach`` writes (or ``--from-file``) and
    prints one JSON line per notable envelope, each delivery once. It never
    touches the lease, cursor or acknowledgments: the watch observes, the
    conversation acknowledges with ``--ack`` after it accepted the words.
    """
    lease_id = (
        lease_identifier(args.provider, args.session) if args.provider else None
    )
    if args.from_file is not None:
        source = args.from_file
    else:
        old_log, events_path = follower_paths(args.bridge_home, lease_id)
        source = events_path
        if not events_path.exists() and old_log.is_file():
            # A pre-sidecar follower still writes JSON to .log. An empty or
            # human .log belongs to the new writer, whose sidecar may not yet
            # exist when the monitor starts.
            with old_log.open(encoding="utf-8", errors="replace") as old:
                if any(line.lstrip().startswith("{") for _, line in zip(range(8), old)):
                    source = old_log
    channel = None
    bindings = (read_json(args.bridge_home / AUDIENCE_BINDING_FILENAME) or {}).get("bindings")
    if isinstance(bindings, dict):
        for slot, binding in bindings.items():
            if (
                isinstance(binding, dict)
                and binding.get("provider") == (args.provider or "").casefold()
                and binding.get("provider_session_id") == args.session
            ):
                channel = str(slot)
                break
    seen: set[str] = set()

    def pump(entries: list[tuple[str, int]]) -> None:
        drafts: dict[tuple[Any, Any], dict[str, Any]] = {}

        def flush(key: tuple[Any, Any] | None = None) -> None:
            keys = [key] if key is not None else list(drafts)
            for current in keys:
                draft = drafts.pop(current, None)
                if draft is not None:
                    print(human_line(draft, channel), flush=True)

        for raw, _cursor in entries:
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError:
                continue  # older logs can contain diagnostics
            line = watch_line(payload, lease_id)
            if args.human:
                if not isinstance(payload, dict) or payload.get("schema") not in (EVENT_SCHEMA, ATTACH_SCHEMA):
                    continue
                if lease_id and payload.get("lease_id") not in (None, lease_id):
                    continue
            elif line is None:
                continue
            identity = str(payload.get("delivery_id") or payload.get("source_event_id") or raw)
            if identity in seen:
                continue  # a restarted follower replays its pending mailbox
            seen.add(identity)
            if args.human:
                key = (payload.get("session_id"), payload.get("document_index"))
                if payload.get("kind") in ("draft", "revised"):
                    drafts[key] = payload
                    continue
                flush(key)
                print(human_line(payload, channel), flush=True)
            else:
                emit(line)
        if args.human:
            flush()

    if args.once:
        if not source.is_file():
            sys.stderr.write(f"bus-demux: watch source missing: {source}\n")
            return 1
        pump(iter_new_lines(source, 0)[0])
        return 0
    offset = 0
    if not args.from_start:
        try:
            offset = source.stat().st_size
        except FileNotFoundError:
            offset = 0
    sys.stderr.write(f"bus-demux: watching {source}\n")
    trigger = BusEventTrigger(source, args.interval)
    try:
        while True:
            entries, offset = iter_new_lines(source, offset)
            pump(entries)
            trigger.wait(timeout=1.0)
    except KeyboardInterrupt:
        return 130
    finally:
        trigger.close()


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
        "--print-agent-turn-lease-path",
        action="store_true",
        help="print the runtime-equivalent agent-turn lease path and exit",
    )
    authority.add_argument(
        "--assert-install-idle",
        action="store_true",
        help="exit zero only when the whole canonical Bus proves installation-safe",
    )
    parser.add_argument(
        "--name",
        default=None,
        help="bound name; exact or unique bounded opening-name match",
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
        "--coalesce",
        action="store_true",
        help="newest draft/revision replaces its predecessors in the mailbox; "
        "seals stay separate envelopes",
    )
    parser.add_argument(
        "--on-seal",
        dest="on_seal",
        default=None,
        help="shell hook fired once per freshly queued seal "
        "(CODESCRIBE_SEAL_DELIVERY_ID/_SESSION_ID/_TEXT in its environment)",
    )
    parser.add_argument(
        "--attach",
        action="store_true",
        help="bind --channel to this provider session, ensure one coalescing "
        "follower, print an attach receipt, and exit",
    )
    parser.add_argument(
        "--channel", default=None, help="agent channel digit (1-9) for --attach"
    )
    parser.add_argument(
        "--status",
        action="store_true",
        help="print one truthful channel status (backlog = pending minus "
        "acknowledgment markers) and exit",
    )
    parser.add_argument(
        "--watch",
        action="store_true",
        help="print one compact JSON line per seal, refused take, state-changing "
        "or routing-ambiguity envelope from this session's follower events "
        "(line-buffered; --once reads the events and exits)",
    )
    parser.add_argument("--human", action="store_true", help="--watch as readable one-line envelopes")
    parser.add_argument(
        "--from-file",
        dest="from_file",
        type=Path,
        default=None,
        help="--watch source instead of the session's follower events",
    )
    parser.add_argument("--follower-events", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--follower-channel", help=argparse.SUPPRESS)
    parser.add_argument(
        "--provider", help="client id, for example codex or claude-code"
    )
    parser.add_argument(
        "--session",
        help="stable provider-session id used for cursor recovery; with "
        "--provider claude-code it defaults to $CLAUDE_CODE_SESSION_ID",
    )
    parser.add_argument("--lease", help="reattach to an explicit lease id")
    parser.add_argument(
        "--ack",
        nargs="+",
        metavar="DELIVERY_ID",
        help="acknowledge one or more deliveries for --provider/--session "
        "(all or nothing)",
    )
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
    parser.add_argument(
        "--say",
        help="append an agent reply to the canonical Bus and speak it through "
        "vendor TTS; --name defaults to the name on this session's lease",
    )
    parser.add_argument(
        "--tts-vendor",
        choices=("xai", "openai"),
        default=None,
        help="TTS speaker; defaults to the name's profile provider in voices.json, "
        "then xai; with --attach it is stored in the profile",
    )
    parser.add_argument(
        "--voice",
        default=None,
        help="TTS voice id; defaults to the name's profile in "
        "<bridge-home>/voices.json; with --attach it is stored in the profile",
    )
    parser.add_argument(
        "--speed",
        type=float,
        default=None,
        help="TTS speed; defaults to the name's profile in voices.json, "
        f"then {DEFAULT_SPEECH_SPEED}; with --attach it is stored in the profile",
    )
    args = parser.parse_args()
    args.bus_overridden = args.bus is not None
    if args.bus is None:
        args.bus = bus_path()
    if args.print_bus_path:
        print(args.bus)
        return 0
    if args.print_install_interlock_path:
        print(install_interlock_path())
        return 0
    if args.print_agent_turn_lease_path:
        print(agent_turn_lease_path())
        return 0
    if args.bridge_home is None:
        args.bridge_home = bridge_home()
    if args.assert_install_idle:
        return 0 if installation_idle(args.bus, bridge_root=args.bridge_home) else 2
    if args.provider and not args.session:
        args.session = provider_session_from_env(args.provider)
    if bool(args.provider) != bool(args.session):
        parser.error(
            "--provider and --session must be supplied together "
            "(only claude-code falls back to $CLAUDE_CODE_SESSION_ID)"
        )
    if args.lease and not args.provider:
        parser.error("--lease requires --provider and --session")
    if args.speed is not None and args.speed <= 0:
        parser.error("--speed must be positive")
    if args.voice is not None and not args.voice.strip():
        parser.error("--voice needs a voice id")
    if args.human and not args.watch:
        parser.error("--human travels with --watch")
    if args.watch or args.from_file is not None:
        if not args.watch:
            parser.error("--from-file travels with --watch")
        if (
            args.ack
            or args.attach
            or args.channel is not None
            or args.status
            or args.say is not None
            or args.active_names
        ):
            parser.error("--watch combines with no other command")
        if args.follow or args.all or args.become or args.lease:
            parser.error("--watch takes only --once or --from-start")
        if not args.provider and args.from_file is None:
            parser.error("--watch requires --provider/--session or --from-file")
        return watch_command(args)
    if args.ack:
        if not args.provider or args.follow or args.once or args.from_start:
            parser.error("--ack requires --provider/--session and no read mode")
        try:
            return acknowledge_delivery(args)
        except (OSError, ValueError) as error:
            sys.stderr.write(f"bus-demux: acknowledgment refused: {error}\n")
            return 3
    if args.attach or args.channel is not None:
        if not (args.attach and args.channel is not None):
            parser.error("--attach and --channel travel together")
        if not args.provider or not args.name:
            parser.error("--attach requires --provider/--session and --name")
        if args.follow or args.once or args.from_start or args.all or args.become:
            parser.error("--attach takes no read mode")
        if args.say is not None or args.status:
            parser.error("--attach combines with no other command")
        if not re.fullmatch(r"[1-9]", str(args.channel)):
            parser.error("--channel must be a single digit 1-9")
        try:
            return attach_command(args)
        except OSError as error:
            sys.stderr.write(f"bus-demux: attach refused: {error}\n")
            return 3
    if args.status:
        if not args.provider:
            parser.error("--status requires --provider and --session")
        if args.follow or args.once or args.from_start or args.say is not None:
            parser.error("--status combines with no other command")
        return status_command(args)
    if args.say is not None:
        if not args.provider:
            parser.error("--say requires --provider/--session")
        if args.follow or args.once or args.from_start or args.all or args.become:
            parser.error("--say takes no read mode")
        if not args.say.strip():
            parser.error("--say needs non-empty text")
        if not args.name:
            # The lease already knows the name --status reports; an explicit
            # --name still wins.
            args.name = lease_name(args.bridge_home, args.provider, args.session)
            if not args.name:
                parser.error("--say needs --name, or an attached lease carrying one")
        try:
            return say_reply(args)
        except OSError as error:
            sys.stderr.write(f"bus-demux: reply refused: {error}\n")
            return 3
    if args.lease_ttl <= 0:
        parser.error("--lease-ttl must be positive")
    if args.active_names:
        leases = active_leases(args.bridge_home, args.lease_ttl)
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
