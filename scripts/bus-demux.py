#!/usr/bin/env python3
"""Named, session-aware follower for the clean Codescribe Transcript Bus.

The helper never opens audio. It reads ``codescribe.transcript.v1`` NDJSON and
emits small agent-bridge envelopes. Product installs run it from the stable
path below, not from a source checkout::

  cs-bus \
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

Table of contents — grep for the ``§ N.`` banner to jump to a section:

  § 1.  Schemas and constants
  § 2.  Runtime paths and environment
  § 3.  Generational bus journal
  § 4.  Installation interlock and CLI session activity
  § 5.  Names, audiences and recipient resolution
  § 6.  Event decoding and evidence normalization
  § 7.  Follower emission and delivery admission
  § 8.  Session lease, persistence and the delivery mailbox
  § 9.  Native queue wakeup
  § 10. Follower run loop
  § 11. Speech: credentials, voice profiles, TTS and playback
  § 12. Reply publication and spoken replies
  § 13. Messages: Founder typed text and agent peer text
  § 14. Channel bindings and session handover
  § 15. Status, watch and the CLI entrypoint
"""

from __future__ import annotations

import argparse
import base64
import contextlib
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
from stat import S_ISREG
from typing import Any, Iterator
from types import SimpleNamespace

# =============================================================================
# § 1. Schemas and constants
# =============================================================================

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
AGENT_USER_MESSAGE_SCHEMA = "codescribe.agent-user-message.v1"
AGENT_REPLY_SCHEMA = "codescribe.agent-reply.v1"
AGENT_REPLY_PLAYBACK_SCHEMA = "codescribe.agent-reply-playback.v1"
REPLY_SOURCE_SCHEMA = "codescribe.agent-reply-source.v1"
REPLY_CONTROL_SCHEMA = "codescribe.agent-reply-control.v1"
PLAYBACK_MUTE_SCHEMA = "codescribe.agent-playback-mute.v1"
REPLY_READ_LIMIT = 32 << 20
AUDIENCE_BINDING_SCHEMA = "vc.agent-audience-binding.v1"
AUDIENCE_BINDING_FILENAME = "vc.agent-audience-binding.v1.json"
ATTACH_RECEIPT_SCHEMA = "codescribe.agent-bridge.attach-receipt.v1"
DETACH_RECEIPT_SCHEMA = "codescribe.agent-bridge.detach-receipt.v1"
TAKEOVER_RECEIPT_SCHEMA = "codescribe.agent-bridge.takeover-receipt.v1"
#: Follower states a handover reports. Only these two authorize rebinding a
#: channel; the other two mean the previous reader is still sitting on it.
HANDOVER_CLEAR_STATES = ("stopped", "not_running")
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


# =============================================================================
# § 2. Runtime paths and environment
# =============================================================================


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


# =============================================================================
# § 3. Generational bus journal
# =============================================================================


def generation_sources(path: Path) -> tuple[list[dict[str, Any]], int, int, float | None]:
    """Read the single generation owner's receipt; broken links never look idle."""
    path = Path(os.path.abspath(path))
    receipt = Path(str(path) + ".generations.json")
    current = path.stat()
    if not receipt.exists():
        return ([{"path": str(path), "start": 0, "length": current.st_size,
                  "dev": current.st_dev, "ino": current.st_ino}], current.st_dev, current.st_ino, getattr(current, "st_birthtime", None))
    with receipt.open("rb") as handle:
        raw = handle.read((4 << 20) + 1)
    if len(raw) > 4 << 20:
        raise ValueError("oversized generation receipt")
    value = json.loads(raw)
    if set(value) != {"schema", "root", "stream_id", "stream_inode", "stream_dev", "stream_birthtime", "segments", "active", "pending"} or value.get("schema") != "codescribe.bus-generations.v1" or value.get("root") != str(path):
        raise ValueError("unknown generation receipt")
    segments = list(value["segments"])
    active = dict(value["active"])
    pending = value.get("pending")
    if pending is not None:
        next_generation = pending["next"]
        if (current.st_dev, current.st_ino) == (next_generation["dev"], next_generation["ino"]):
            segments.append(pending["closed"])
            active = dict(next_generation)
        elif (current.st_dev, current.st_ino) != (active["dev"], active["ino"]):
            raise ValueError("broken generation transaction")
    expected = 0
    events = path.parent / "events"
    for segment in segments:
        if set(segment) != {"id", "path", "start", "length", "dev", "ino", "day", "compressed", "sha256", "superseded"}:
            raise ValueError("unknown generation segment")
        physical = Path(segment["path"])
        if not physical.is_relative_to(events) or ".." in physical.parts:
            raise ValueError("generation outside selected storage")
        meta = physical.stat()
        if (segment["start"], segment["length"], segment["dev"], segment["ino"]) != (
            expected, meta.st_size, meta.st_dev, meta.st_ino
        ):
            raise ValueError("changed closed generation")
        expected += meta.st_size
    if active["start"] != expected or (active["dev"], active["ino"]) != (current.st_dev, current.st_ino):
        raise ValueError("broken active generation")
    active.update(path=str(path), length=current.st_size)
    segments.append(active)
    return segments, value["stream_dev"], value["stream_inode"], value["stream_birthtime"]


def generation_metadata(path: Path) -> Any:
    segments, dev, ino, birth = generation_sources(path)
    current = path.stat()
    last = next((s for s in reversed(segments) if s["length"]), segments[-1])
    modified = Path(last["path"]).stat()
    return SimpleNamespace(st_dev=dev, st_ino=ino,
        st_size=segments[-1]["start"] + segments[-1]["length"],
        st_mode=current.st_mode, st_mtime_ns=modified.st_mtime_ns,
        st_birthtime=birth)


class GenerationFile:
    """Bounded ordinary open/read/seek over verified, ordered journal files."""
    def __init__(self, path: Path) -> None:
        self.path = path
        self.segments, self.dev, self.ino, _ = generation_sources(path)
        self.position = 0
        self.size = self.segments[-1]["start"] + self.segments[-1]["length"]

    def __enter__(self) -> GenerationFile:
        return self

    def __exit__(self, *_: Any) -> None:
        pass

    def metadata(self, refresh: bool = False) -> Any:
        return generation_metadata(self.path)

    def tell(self) -> int:
        return self.position

    def seek(self, position: int, whence: int = 0) -> int:
        position = position if whence == 0 else (self.position if whence == 1 else self.size) + position
        if position < 0:
            raise ValueError("negative generation cursor")
        self.position = position
        return position

    def read(self, width: int) -> bytes:
        if width < 0:
            raise ValueError("unbounded generation read refused")
        out = bytearray()
        for segment in self.segments:
            end = segment["start"] + segment["length"]
            if self.position >= end or not width:
                continue
            if self.position < segment["start"]:
                raise ValueError("generation gap")
            try:
                descriptor = Path(segment["path"]).open("rb")
            except FileNotFoundError:
                refreshed, _, _, _ = generation_sources(self.path)
                current = next((s for s in refreshed if s.get("id") == segment.get("id") and s["start"] == segment["start"] and s["length"] == segment["length"]), None)
                if current is None:
                    raise ValueError("lost generation")
                segment = current
                descriptor = Path(segment["path"]).open("rb")
            with descriptor as handle:
                meta = os.fstat(handle.fileno())
                if (meta.st_dev, meta.st_ino) != (segment["dev"], segment["ino"]) or meta.st_size < segment["length"]:
                    raise ValueError("changed generation descriptor")
                handle.seek(self.position - segment["start"])
                raw = handle.read(min(width, end - self.position))
            if not raw:
                raise ValueError("short generation")
            out.extend(raw)
            self.position += len(raw)
            width -= len(raw)
        return bytes(out)

    def readline(self, limit: int = 1024 * 1024 + 1) -> bytes:
        out = bytearray()
        while len(out) < limit:
            raw = self.read(min(4096, limit - len(out)))
            if not raw:
                break
            newline = raw.find(b"\n")
            if newline >= 0:
                out.extend(raw[:newline + 1])
                self.position -= len(raw) - newline - 1
                break
            out.extend(raw)
        return bytes(out)


# =============================================================================
# § 4. Installation interlock and CLI session activity
# =============================================================================


def installation_idle(
    path: Path,
    *,
    sealed_is_idle: bool = True,
    cursor: dict[str, Any] | None = None,
    bridge_root: Path | None = None,
    deadline: float | None = None,
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
    A supplied monotonic deadline limits each scan to one polling slice.
    Budget expiry retains the processed prefix and refuses idle; the caller
    can resume that cursor without skipping any unread lifecycle rows.
    """
    scan_started = time.monotonic()
    scan_deadline = (
        min(deadline, scan_started + PLAYBACK_POLL_SECONDS)
        if deadline is not None
        else scan_started + TAKE_WAIT_SECONDS
    )
    root = bridge_root if bridge_root is not None else bridge_home()
    cursors = cursor if cursor is not None else {}
    channel_cursors = cursors.setdefault("channel_buses", {})
    try:
        path = path.expanduser().resolve(strict=False)
        root = root.expanduser().resolve(strict=False)
        paths = {path, *channel_cursors}
        discovered = set()
        for channel_bus in (root / "buses").glob("channel-*.jsonl"):
            if time.monotonic() >= scan_deadline:
                return False
            discovered.add(channel_bus)
        paths.update(discovered)
        binding_path = root / AUDIENCE_BINDING_FILENAME
        binding_identity = None
        try:
            descriptor = os.open(binding_path, os.O_RDONLY | os.O_NONBLOCK)
            with os.fdopen(descriptor, "rb") as handle:
                metadata = os.fstat(handle.fileno())
                if not S_ISREG(metadata.st_mode) or metadata.st_size > 1024 * 1024:
                    return False
                binding_identity = (
                    metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns,
                )
                raw_binding = handle.read(1024 * 1024 + 1)
                if len(raw_binding) > 1024 * 1024:
                    return False
                binding = json.loads(raw_binding)
        except FileNotFoundError:
            binding = {"schema": AUDIENCE_BINDING_SCHEMA, "bindings": {}}
        if (
            not isinstance(binding, dict)
            or binding.get("schema") != AUDIENCE_BINDING_SCHEMA
            or not isinstance(binding.get("bindings"), dict)
        ):
            return False
        for entry in binding["bindings"].values():
            if time.monotonic() >= scan_deadline:
                return False
            if not isinstance(entry, dict):
                return False
            raw_bus = entry.get("bus")
            if raw_bus is not None:
                if not isinstance(raw_bus, str) or not raw_bus.strip():
                    return False
                paths.add(Path(os.path.expanduser(raw_bus.strip())).resolve(strict=False))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return False
    idle = True
    observed_sources = set()
    for source in sorted(paths):
        if time.monotonic() >= scan_deadline:
            return False
        state = cursors if source == path else channel_cursors.setdefault(source, {})
        if not source.exists():
            # A disappearing known bus is no terminal receipt. Preserve its
            # CLI/channel starts until their timestamp proves abandonment.
            if (
                state.get("live_app") is not None
                or any(
                    not _cli_session_abandoned(activity, time.time())
                    for activity in state.get("open_cli", {}).values()
                )
                or any(
                    not _cli_session_abandoned(activity, time.time())
                    for _, activity in state.get("open_channels", {}).values()
                )
            ):
                idle = False
            continue
        if not source.is_file():
            return False
        observed_sources.add(source)
        open_cli: dict[str, str | None] = {}
        open_channels: dict[str, tuple[str, str | None]] = {}
        live_app: str | None = None
        try:
            with GenerationFile(source) as handle:
                stat = handle.metadata()
                if not S_ISREG(stat.st_mode):
                    return False
                identity = (
                    stat.st_dev,
                    stat.st_ino,
                    sealed_is_idle,
                    getattr(stat, "st_birthtime", None),
                )

                def fingerprint(position: int) -> tuple[str, str]:
                    handle.seek(0)
                    head = hashlib.sha256(handle.read(min(position, 256))).hexdigest()
                    handle.seek(max(0, position - 256))
                    edge = hashlib.sha256(handle.read(min(position, 256))).hexdigest()
                    return head, edge

                initial_head, _ = fingerprint(stat.st_size)
                # Appends may change mtime; a same-size rewrite, shrink,
                # replacement or changed processed boundary requires replay.
                # Retain unresolved starts across that replay: rotation is no
                # terminal receipt for a previously observed live take.
                if state.get("identity", (None, None, None))[2] == sealed_is_idle:
                    open_cli = dict(state.get("open_cli", {}))
                    live_app = state.get("live_app")
                    open_channels = dict(state.get("open_channels", {}))
                if (
                    state
                    and state.get("identity") == identity
                    and 0 <= state["offset"] <= state["size"] <= stat.st_size
                    and (
                        state["size"] < stat.st_size
                        or state["mtime_ns"] == stat.st_mtime_ns
                    )
                    and fingerprint(state["offset"]) == (state["head"], state["edge"])
                ):
                    handle.seek(state["offset"])
                else:
                    handle.seek(0)
                caught_up = False
                storage = ChunkDecoder()
                storage_start = handle.tell()
                offset = handle.tell()
                try:
                    # Byte and time bounds keep one malformed row or a cold
                    # history from monopolizing a synchronous playback probe.
                    while True:
                        offset = handle.tell()
                        if time.monotonic() >= scan_deadline:
                            return False
                        raw = handle.readline(1024 * 1024 + 1)
                        if not raw:
                            caught_up = True
                            break
                        if len(raw) > 1024 * 1024 or not raw.endswith(b"\n"):
                            raise ValueError("incomplete or oversized bus row")
                        raw = raw.decode("utf-8", errors="strict").strip()
                        if not raw:
                            continue
                        event = json.loads(raw)
                        if not isinstance(event, dict):
                            raise ValueError("bus row is not an object")
                        if not storage.pending:
                            storage_start = offset
                        event = storage.feed(event)
                        if event is None:
                            continue
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
                finally:
                    if storage.pending:
                        offset = storage_start
                        caught_up = False
                    # Only complete, successfully handled rows advance offset.
                    # Save progress on budget expiry and retry malformed tails.
                    snapshot = handle.metadata(refresh=True)
                    current_head, _ = fingerprint(stat.st_size)
                    if (
                        snapshot.st_size < stat.st_size
                        or initial_head != current_head
                        or (
                            snapshot.st_size == stat.st_size
                            and snapshot.st_mtime_ns != stat.st_mtime_ns
                        )
                    ):
                        offset = 0
                        caught_up = False
                    head, edge = fingerprint(offset)
                    caught_up = caught_up and offset == snapshot.st_size
                    state.update(
                        identity=identity,
                        offset=offset,
                        size=snapshot.st_size,
                        mtime_ns=snapshot.st_mtime_ns,
                        head=head,
                        edge=edge,
                        open_cli=open_cli,
                        live_app=live_app,
                        open_channels=open_channels,
                        caught_up=caught_up,
                    )
        except (OSError, ValueError, KeyError, TypeError, IndexError):
            return False
        if not state.get("caught_up") or time.monotonic() >= scan_deadline:
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
    if not idle:
        return False
    try:
        # Another bus or binding may change while a cold sibling is read.
        # Such changes require another pass, never an idle certificate.
        current_discovered = set()
        for channel_bus in (root / "buses").glob("channel-*.jsonl"):
            if time.monotonic() >= scan_deadline:
                return False
            current_discovered.add(channel_bus)
        if current_discovered != discovered:
            return False
        try:
            metadata = binding_path.stat()
            current_binding = (
                metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns,
            )
        except FileNotFoundError:
            current_binding = None
        if current_binding != binding_identity:
            return False
        for source in paths:
            if time.monotonic() >= scan_deadline:
                return False
            state = cursors if source == path else channel_cursors[source]
            try:
                metadata = generation_metadata(source)
            except FileNotFoundError:
                if source in observed_sources:
                    return False
                continue
            if (
                state.get("identity") != (
                    metadata.st_dev, metadata.st_ino, sealed_is_idle,
                    getattr(metadata, "st_birthtime", None),
                )
                or state.get("offset") != metadata.st_size
                or state.get("mtime_ns") != metadata.st_mtime_ns
            ):
                return False
    except (OSError, ValueError, KeyError, TypeError, IndexError):
        return False
    return time.monotonic() < scan_deadline


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


# =============================================================================
# § 5. Names, audiences and recipient resolution
# =============================================================================


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


# A preview of a document the reducer is still building.
DRAFT_KINDS = ("draft", "revised")
# The envelope that closes a message: an agent acknowledges one of these, never
# a draft. A channel take and a coverage-refused flush both seal; typed Founder
# messages arrive as `message`.
TERMINAL_KINDS = ("seal", "message")


def draft_key(payload: dict[str, Any]) -> tuple[Any, Any]:
    """The message a draft previews, and that its terminal envelope closes.

    Channel takes carry `message_id` (`channel_message_identity`); named
    utterances are keyed by the reducer's `document_index` instead.
    """
    return payload.get("session_id"), payload.get("message_id") or payload.get("document_index")


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


# =============================================================================
# § 6. Event decoding and evidence normalization
# =============================================================================


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
    for key in ("recipients", "broadcast_id", "channel", "provider", "provider_session_id", "lease_id"):
        if key in event:
            payload[key] = event[key]
    for key in ("message_id", "occurrences"):
        if key in event:
            payload[key] = event[key]
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


class ChunkDecoder:
    """Hash bounded transport records; guards need metadata, never document bytes."""
    def __init__(self, assemble: bool = True) -> None:
        self.assemble = assemble
        self.pending = False
        self.identity = ""
        self.next = 0
        self.parts = 0
        self.length = 0
        self.count = 0
        self.header: dict[str, Any] = {}
        self.hasher = hashlib.sha256()
        self.payload = bytearray()

    def feed(self, row: dict[str, Any]) -> dict[str, Any] | None:
        if row.get("schema") != "codescribe.bus-chunk.v1":
            if self.pending:
                raise ValueError("interrupted storage record")
            return row
        part, parts, length = row.get("part"), row.get("parts"), row.get("length")
        if any(type(v) is not int or v < 0 for v in (part, parts, length)):
            raise ValueError("unknown storage record")
        if not parts or parts != (length + 32767) // 32768:
            raise ValueError("invalid storage length")
        if self.assemble and length > 256 << 20:
            raise ValueError("storage document exceeds reconstruction budget")
        header = row.get("event")
        if not isinstance(header, dict):
            raise ValueError("missing storage metadata")
        if not self.assemble and header.get("schema") != EVIDENCE_SCHEMA:
            raise ValueError("guard refuses chunked non-evidence control record")
        if part == 0:
            if self.pending:
                raise ValueError("overlapping storage records")
            self.pending = True
            self.identity = str(row.get("id"))
            self.parts, self.length, self.next, self.count = parts, length, 0, 0
            self.header = header
            self.hasher = hashlib.sha256()
            self.payload = bytearray()
        if (row.get("id"), part, parts, length, header) != (
            self.identity, self.next, self.parts, self.length, self.header
        ):
            raise ValueError("broken storage ordering")
        block = base64.b64decode(row.get("payload", ""), validate=True)
        if len(block) > 32768 or self.count + len(block) > length:
            raise ValueError("oversized storage part")
        self.hasher.update(block)
        self.count += len(block)
        self.next += 1
        if self.assemble:
            self.payload.extend(block)
        if self.next != parts:
            return None
        if self.count != length or self.hasher.hexdigest() != self.identity:
            raise ValueError("unverified storage payload")
        result = json.loads(self.payload) if self.assemble else dict(self.header)
        if self.assemble and any(result.get(k) != v for k, v in self.header.items()):
            raise ValueError("changed storage metadata")
        self.pending = False
        self.payload.clear()
        return result


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
        AGENT_USER_MESSAGE_SCHEMA,
        # Replies enter the follower stream only for the peer text lane;
        # admission drops every reply that does not name a peer recipient.
        AGENT_REPLY_SCHEMA,
        "codescribe.bus-chunk.v1",
    ):
        return None
    if "persistence_encoding" in event:
        if (
            event.get("schema") != EVIDENCE_SCHEMA
            or evidence_revision_rows(event) is None
        ):
            return None
    return event


def evidence_revision_rows(event: dict[str, Any]) -> list[dict[str, Any]] | None:
    """Expand persisted occurrence metadata without deriving text or authority."""
    if "persistence_encoding" not in event:
        return [event]
    if event.get("persistence_encoding") != "shared-revision.v1":
        return None
    occurrences = event.get("occurrence_rows")
    if not isinstance(occurrences, list):
        return None
    coordinates = (
        "sequence",
        "capture_epoch",
        "sample_start",
        "sample_end",
        "document_index",
    )
    labels = ("emitted_at", "occurrence_session_id", "label")
    allowed = {*coordinates, *labels, "acoustic_receipts"}
    if not isinstance(event.get("rendered_text"), str):
        return None
    first = dict(event)
    first.pop("persistence_encoding", None)
    first.pop("occurrence_rows", None)
    rows = [first]
    for occurrence in occurrences:
        if not isinstance(occurrence, dict) or set(occurrence) != allowed:
            return None
        if any(
            type(occurrence.get(key)) is not int or occurrence[key] < 0
            for key in coordinates
        ):
            return None
        if any(not isinstance(occurrence.get(key), str) for key in labels):
            return None
        if not isinstance(occurrence.get("acoustic_receipts"), list):
            return None
        row = dict(first)
        row.update(occurrence)
        rows.append(row)
    return rows


def normalized_revision_events(
    raw: str, normalizer: EvidenceNormalizer
) -> list[dict[str, Any]]:
    """One durable row may observe several original publication coordinates."""
    parsed = parse_line(raw)
    if parsed is None:
        if normalizer.storage.pending or "persistence_encoding" in raw or "codescribe.bus-chunk" in raw:
            raise ValueError("unverified storage encoding")
        return []
    parsed = normalizer.storage.feed(parsed)
    if parsed is None:
        return []
    rows = evidence_revision_rows(parsed)
    if rows is None:
        raise ValueError("unverified revision encoding")
    if (parsed.get("schema") == EVIDENCE_SCHEMA
            and channel_of_session(str(parsed.get("session_id") or "")) is not None):
        # The storage inventory describes one complete reducer snapshot. It
        # does not turn its physical entries into separate spoken messages.
        snapshot = dict(rows[0])
        snapshot["occurrences"] = [
            {key: row.get(key) for key in (
                "sequence", "emitted_at", "occurrence_session_id", "capture_epoch",
                "sample_start", "sample_end", "document_index", "label", "acoustic_receipts",
            )}
            for row in rows
        ]
        rows = [snapshot]
    events: list[dict[str, Any]] = []
    for row in rows:
        event = normalizer.normalize(row)
        events.extend(normalizer.pop_flushes())
        if event is not None:
            events.append(event)
    return events


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


def channel_message_identity(session: str) -> str:
    """One manual channel take; PCM entries remain evidence inside it."""
    return _identity(("channel-message", session))


def channel_message_phase_identity(event: dict[str, Any]) -> str:
    session = event.get("session_id")
    if event.get("status") == SEALED:
        return _identity(("channel-message-seal", session))
    return _identity(("channel-message-revision", session,
                      event.get("reducer_revision"), event.get("reducer_action")))


class EvidenceNormalizer:
    """Translate ``transcript-evidence.v1`` rows into the shape the bridge speaks.

    ``rendered_text`` is an immutable full snapshot from the reducer.  The
    bridge forwards it verbatim; it never infers a delta, ordering, revision,
    or finality from characters.  Terminal rows are coalesced only by the
    reducer's stable terminal phase identity, because one terminal receipt can
    project once per document entry.

    Channel safety net: a ledger that refuses terminal finality emits no
    terminal seal, so an addressed utterance would vanish without a trace.
    The normalizer keeps the latest whole snapshot of each unsealed
    channel session and, when the session is over — a ``channel-session``
    receipt shows the channel moved on (any non-open state for the session,
    or a fresh open of the same channel), or the session's own
    ``session_ended`` lifecycle row marks a hang-up — flushes one message via
    :meth:`pop_flushes`. A ledger terminal receipt carries certification;
    without it the envelope keeps ``coverage: "refused"``. Physical occurrence
    coordinates and receipts accompany the full render, never split its text.
    """

    #: Unsealed channel sessions retained for the flush net. The quiet
    #: contract reopens a channel within seconds of every real utterance, so
    #: anything beyond a handful of sessions is an abandoned bus replay.
    MAX_TRACKED_SESSIONS = 16

    def __init__(self) -> None:
        self.storage = ChunkDecoder()
        self._terminal_seals: set[str] = set()
        # Sessions whose delivery is settled: a terminal seal was reported or
        # the refused flush already carried their words. A late row for such
        # a session must not re-arm the net and deliver the take twice.
        self._settled_sessions: set[str] = set()
        self._session_docs: dict[str, dict[str, Any]] = {}
        self._channel_open_times: dict[str, tuple[str, float]] = {}
        self._flushes: list[dict[str, Any]] = []

    def pop_flushes(self) -> list[dict[str, Any]]:
        """Final channel envelopes triggered by the last normalized row."""
        flushes, self._flushes = self._flushes, []
        return flushes

    def restore_channel_documents(self, documents: list[dict[str, Any]]) -> None:
        """Resume observed snapshots, including a queued row before cursor commit."""
        for document in documents:
            session = str(document.get("session_id") or "")
            if (channel_of_session(session) is None
                    or document.get("message_id") != channel_message_identity(session)):
                continue
            if document.get("status") == SEALED:
                self._settled_sessions.add(session)
                self._session_docs.pop(session, None)
            elif session not in self._settled_sessions:
                self._remember_channel_document(document, dict(document))

    def channel_documents(self) -> dict[str, dict[str, Any]]:
        return {session: dict(document) for session, document in self._session_docs.items()}

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
        if event.get("reducer_action") == SESSION_ENDED:
            self._flush_session(str(event.get("session_id") or ""), event.get("emitted_at"))
            return None
        document = event.get("rendered_text")
        if not isinstance(document, str):
            return None
        session = str(event.get("session_id") or "")
        if event.get("audience") and channel_of_session(session) is not None:
            if session in self._settled_sessions:
                return None
            clean = self._as_clean(event, LIVE_STATUSES[1], document)
            clean["message_id"] = channel_message_identity(session)
            if not self._remember_channel_document(event, clean):
                return None
            # Even a ledger terminal phase is a preview until capture ends.
            return clean
        if str(event.get("reducer_action") or "") == TERMINAL_SEAL:
            self._settled_sessions.add(session)
            self._session_docs.pop(session, None)
            seal_id = terminal_seal_identity(event)
            if seal_id in self._terminal_seals:
                return None
            self._terminal_seals.add(seal_id)
            return self._as_clean(event, SEALED, document)
        clean = self._as_clean(event, LIVE_STATUSES[1], document)
        return clean

    def _remember_channel_document(
        self, event: dict[str, Any], clean: dict[str, Any]
    ) -> bool:
        session = str(event.get("session_id") or "")
        if not clean.get("audience") or channel_of_session(session) is None:
            return False
        if session in self._settled_sessions:
            return False
        previous = self._session_docs.get(session)
        incoming_revision = event.get("reducer_revision")
        older_snapshot = (previous is not None and type(incoming_revision) is int
                and type(previous.get("reducer_revision")) is int
                and incoming_revision < previous["reducer_revision"])
        inventory = {
            (item.get("occurrence_session_id"), item.get("capture_epoch"),
             item.get("sample_start"), item.get("sample_end")): item
            for item in (previous or {}).get("occurrences", [])
        }
        for item in event.get("occurrences", [
            {key: event.get(key) for key in (
                "sequence", "emitted_at", "occurrence_session_id", "capture_epoch",
                "sample_start", "sample_end", "document_index", "label", "acoustic_receipts",
            )}
        ]):
            key = (item.get("occurrence_session_id"), item.get("capture_epoch"),
                   item.get("sample_start"), item.get("sample_end"))
            if not older_snapshot or key not in inventory:
                inventory[key] = item
        clean["occurrences"] = list(inventory.values())
        if older_snapshot:
            previous["occurrences"] = clean["occurrences"]
            return False
        same_phase = previous is not None and (
            previous.get("reducer_revision"), previous.get("reducer_action")
        ) == (incoming_revision, event.get("reducer_action"))
        if same_phase:
            previous["occurrences"] = clean["occurrences"]
        else:
            self._session_docs[session] = clean
        while len(self._session_docs) > self.MAX_TRACKED_SESSIONS:
            self._session_docs.pop(next(iter(self._session_docs)))
        return not same_phase

    def _consume_channel_row(self, row: dict[str, Any]) -> None:
        channel = str(row.get("channel") or "")
        session = str(row.get("session_id") or "")
        if not session or channel_of_session(session) != channel:
            return
        # A delayed predecessor close is evidence only for that named capture.
        # It cannot terminate documents belonging to a newer admitted take.
        if str(row.get("state") or "") != "open":
            self._flush_session(session, row.get("emitted_at"))
            return
        opened_at = row.get("opened_at")
        if not isinstance(opened_at, str):
            return
        try:
            opened = datetime.datetime.fromisoformat(opened_at.replace("Z", "+00:00"))
            if opened.tzinfo is None:
                return
            timestamp = opened.timestamp()
        except (ValueError, OverflowError):
            return
        identity = (channel, timestamp)
        previous = self._channel_open_times.get(session)
        if previous is not None and previous != identity:
            return
        self._channel_open_times[session] = identity
        for cached in list(self._session_docs):
            known = self._channel_open_times.get(cached)
            if (cached != session and known is not None
                    and known[0] == channel and known[1] < timestamp):
                self._flush_session(cached, row.get("emitted_at"))
        while len(self._channel_open_times) > self.MAX_TRACKED_SESSIONS:
            oldest = min(self._channel_open_times,
                         key=lambda key: self._channel_open_times[key][1])
            del self._channel_open_times[oldest]

    def _flush_session(self, session: str, emitted_at: Any) -> None:
        self._channel_open_times.pop(session, None)
        clean = self._session_docs.pop(session, None)
        if not clean or session in self._settled_sessions:
            return
        self._settled_sessions.add(session)
        if not (clean.get("text") or "").strip():
            return
        final = dict(clean)
        final.update(status=SEALED, emitted_at=emitted_at or clean.get("emitted_at"))
        if clean.get("reducer_action") != TERMINAL_SEAL:
            final["coverage"] = COVERAGE_REFUSED
        self._flushes.append(final)

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
        for key in ("recipients", "broadcast_id", "channel", "provider", "provider_session_id", "lease_id"):
            if key in event:
                clean[key] = event[key]
        return clean


# =============================================================================
# § 7. Follower emission and delivery admission
# =============================================================================


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
    if event.get("schema") == AGENT_REPLY_SCHEMA:
        # Agent-to-agent text lane. Only a reply that explicitly names a peer
        # recipient is a delivery; ordinary spoken replies carry no `peer_to`
        # and never re-enter any mailbox, so replying cannot echo-loop.
        peer_to = event.get("peer_to")
        sender = event.get("sender")
        if (event.get("kind") != "agent_reply" or not isinstance(peer_to, str)
                or not isinstance(event.get("text"), str) or not event["text"].strip()
                or not isinstance(event.get("reply_id"), str)
                or not re.fullmatch(r"[0-9a-f]{24}", event["reply_id"])
                or event.get("message_id") != event["reply_id"]
                or event.get("source_event_id") != event["reply_id"]
                or not isinstance(sender, dict)
                or any(not isinstance(sender.get(key), str) or not sender[key]
                       for key in ("name", "provider", "provider_session_id", "lease_id"))):
            return None
        if not name or peer_to.casefold() != name.casefold():
            return None
        # Peer text is coordination between agents, never Founder authority:
        # it must not inherit the typed lane's state_change_allowed=True.
        return {**event, "schema": EVENT_SCHEMA, "kind": "message",
                "producer_schema": AGENT_REPLY_SCHEMA, "state_change_allowed": False,
                "routing_match": "audience"}
    if event.get("schema") == AGENT_USER_MESSAGE_SCHEMA:
        if (event.get("kind") != "agent_user_message" or event.get("source") != "typed"
                or not isinstance(event.get("text"), str) or not event["text"].strip()
                or not isinstance(event.get("message_id"), str)
                or not re.fullmatch(r"[0-9a-f]{24}", event["message_id"])
                or event.get("source_event_id") != event["message_id"]):
            return None
        audience = event.get("audience")
        if not isinstance(audience, str) or not name or audience.casefold() != name.casefold():
            return None
        return {**event, "schema": EVENT_SCHEMA, "kind": "message",
                "producer_schema": AGENT_USER_MESSAGE_SCHEMA, "state_change_allowed": True,
                "routing_match": "audience"}
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
        size = generation_metadata(path).st_size
    except FileNotFoundError:
        return [], offset
    if size < offset:
        # Rotation/truncation is an authority boundary. Replaying the new file
        # from byte zero could disclose sealed commands that predate this
        # provider lease, so resume at the new EOF and wait for fresh events.
        return [], size
    entries: list[tuple[str, int]] = []
    with GenerationFile(path) as handle:
        handle.seek(offset)
        while True:
            raw = handle.readline()
            if not raw:
                break
            if not raw.endswith(b"\n"):
                break
            entries.append((raw.decode("utf-8", errors="strict"), handle.tell()))
            if handle.tell() - offset >= 64 << 20:
                break
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
        with GenerationFile(path) as handle:
            while True:
                raw = handle.readline()
                if not raw:
                    return
                if not raw.endswith(b"\n"):
                    raise ValueError("incomplete or oversized journal row")
                yield raw.decode("utf-8", errors="strict")
    except FileNotFoundError:
        return


# =============================================================================
# § 8. Session lease, persistence and the delivery mailbox
# =============================================================================


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def atomic_json(path: Path, payload: dict[str, Any]) -> None:
    encoded = (json.dumps(payload, ensure_ascii=False, sort_keys=True) + "\n").encode(
        "utf-8"
    )
    atomic_bytes(path, encoded)


def atomic_bytes(path: Path, encoded: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        path.parent.chmod(0o700)
    except OSError:
        pass
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{time.time_ns()}.tmp")
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


def process_identity(pid: int) -> dict[str, str] | None:
    """Observe one process incarnation and its complete invocation, never TTL."""
    import subprocess

    try:
        result = subprocess.run(
            ["/bin/ps", "-ww", "-p", str(pid), "-o", "lstart=", "-o", "state=", "-o", "command="],
            capture_output=True, text=True, check=False, timeout=2,
            env={**os.environ, "LC_ALL": "C"},
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    fields = result.stdout.strip().split(None, 6)
    if result.returncode or len(fields) != 7 or fields[5].startswith(("T", "Z", "X")):
        return None
    return {"started": " ".join(fields[:5]), "command": fields[6]}


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
        self.process_identity = process_identity(os.getpid())
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
            self.wakeup_configuration = previous.get("wakeup_configuration") if previous else None
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
            self.unclosed_channel_messages: dict[str, dict[str, Any]] = {}
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
                try:
                    extent = generation_metadata(Path(self.bus)).st_size
                except OSError:
                    extent = None
                if extent is not None and self.cursor > extent:
                    # Compare logical positions across all journal generations.
                    # Rotation keeps the stream; only a genuinely shorter
                    # journal invalidates the recovered cursor and documents.
                    self.cursor = 0
                    previous["unclosed_channel_messages"] = {}
                self.last_sequence = previous.get("last_sequence")
                # The follower's requested name outranks the recovered lease
                # name: a channel rename (detach + attach under a new name)
                # must take effect on resume, or the lease pins its first
                # name forever and direct routing to the new name goes deaf
                # while broadcasts still arrive.
                self.name = self.name or previous.get("name")
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
                documents = previous.get("unclosed_channel_messages", {})
                if (not isinstance(documents, dict) or len(documents) > EvidenceNormalizer.MAX_TRACKED_SESSIONS
                        or any(not isinstance(document, dict)
                            or document.get("session_id") != session
                            or channel_of_session(session) is None
                            or document.get("message_id") != channel_message_identity(session)
                            or document.get("status") not in LIVE_STATUSES
                            or not isinstance(document.get("text"), str)
                            or not isinstance(document.get("occurrences"), list)
                            or any(not isinstance(item, dict) for item in document["occurrences"])
                            or document.get("producer_schema") != EVIDENCE_SCHEMA
                            or not self.admits_channel_event(document)
                            for session, document in documents.items())):
                    raise ValueError("invalid unclosed channel messages; recovery state preserved")
                self.unclosed_channel_messages = documents
                self.resumed = True
            elif follow_from_end:
                try:
                    self.cursor = generation_metadata(bus).st_size
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
                "unclosed_channel_messages": self.unclosed_channel_messages,
                "active": active,
                "pid": os.getpid(),
                "process_identity": self.process_identity,
                "heartbeat_unix": time.time(),
                "updated_at": utc_now(),
                **({"wakeup_configuration": self.wakeup_configuration} if self.wakeup_configuration else {}),
            },
        )

    def queue_delivery(self, payload: dict[str, Any]) -> bool:
        delivery_id = payload["delivery_id"]
        if delivery_acknowledged(self.root, self.lease_id, delivery_id, payload):
            return False
        if delivery_id in self.pending:
            return False
        if self.coalesce and payload.get("kind") in DRAFT_KINDS:
            # A reducer storm re-states one document ~250 times per sentence.
            # Under coalescing the newest revision replaces its predecessors
            # in the mailbox instead of stacking toward the 256 cap; the seal
            # stays a separate envelope so terminal delivery is never merged.
            for queued_id in self._drafts_of(draft_key(payload)):
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

    def _drafts_of(self, key: tuple[Any, Any]) -> list[str]:
        return [
            queued_id
            for queued_id, item in self.pending.items()
            if item.get("kind") in DRAFT_KINDS and draft_key(item) == key
        ]

    def prune_settled_drafts(self) -> None:
        """Apply the same rule across runs: an acknowledged terminal envelope
        settles the drafts of its message.

        A lease can hold previews whose terminal envelope was acknowledged and
        left the mailbox in an earlier process. The acknowledgment marker
        keeps the terminal envelope's causal coordinates (`receipt_envelope`
        drops only transcript-bearing fields), so a settled message is provable
        from the marker store rather than guessed.
        """
        if not self.pending:
            return
        settled: set[tuple[Any, Any]] = set()
        try:
            entries = os.listdir(self.root / "acknowledgments" / self.lease_id)
        except OSError:
            return
        for entry in entries:
            if not entry.endswith(".json"):
                continue
            delivery_id = entry[: -len(".json")]
            if not re.fullmatch(r"[0-9a-f]{24}", delivery_id):
                continue
            if not delivery_acknowledged(self.root, self.lease_id, delivery_id):
                continue
            envelope = (read_json(self.root / "acknowledgments" / self.lease_id / entry) or {}).get("envelope")
            if isinstance(envelope, dict) and envelope.get("kind") in TERMINAL_KINDS:
                settled.add(draft_key(envelope))
        orphans = [
            queued_id
            for queued_id, item in self.pending.items()
            if item.get("kind") in DRAFT_KINDS and draft_key(item) in settled
        ]
        if orphans:
            for queued_id in orphans:
                del self.pending[queued_id]
            self.persist(active=True)

    def collect_acknowledgments(self, native: Any = None) -> None:
        completed = [
            delivery_id
            for delivery_id in self.pending
            if delivery_acknowledged(self.root, self.lease_id, delivery_id, self.pending[delivery_id])
        ]
        settled = []
        for delivery_id in completed:
            receipt = read_json(self.root / "wakeups" / self.lease_id / f"{delivery_id}.json")
            if self.provider != "codex" or receipt is None or receipt.get("queue_disposition") in ("removed", "not_pending"):
                settled.append(delivery_id)
            elif native:
                native.enqueue_withdrawal(delivery_id)
        if settled:
            # An acknowledged terminal envelope settles the drafts of its
            # message: the consumer has seen the whole take, so its previews
            # can leave with it instead of holding the mailbox for the life of
            # the lease. Read the key before the envelope is deleted. A
            # terminal acknowledged but still awaiting a native queue
            # withdrawal is not in `settled`, so it retires nothing yet.
            retired = [
                queued_id
                for delivery_id in settled
                if self.pending[delivery_id].get("kind") in TERMINAL_KINDS
                for queued_id in self._drafts_of(draft_key(self.pending[delivery_id]))
            ]
            for delivery_id in dict.fromkeys(settled + retired):
                del self.pending[delivery_id]
            self.persist(active=True)

    def bind_name(self, name: str) -> None:
        self.name = name.casefold()
        self.persist(active=True)

    def admits_channel_event(self, event: dict[str, Any]) -> bool:
        frozen = event.get("recipients")
        expected = {"provider": self.provider, "provider_session_id": self.provider_session_id,
                    "lease_id": self.lease_id, "bus": self.bus}
        if isinstance(frozen, list):
            return any(isinstance(owner, dict) and all(owner.get(key) == value
                       for key, value in expected.items()) for owner in frozen)
        if channel_of_session(str(event.get("session_id") or "")) is not None:
            return all(event.get(key) == value for key, value in expected.items())
        return frozen is None

    def enrich(self, payload: dict[str, Any]) -> None:
        payload["lease_id"] = self.lease_id
        payload["provider"] = self.provider
        payload["provider_session_id"] = self.provider_session_id
        payload["bus"] = self.bus
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
        if (payload.get("message_id") == channel_message_identity(str(payload.get("session_id") or ""))
                and channel_of_session(str(payload.get("session_id") or "")) is not None):
            phase_id = channel_message_phase_identity(payload)
        elif (
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


def receipt_envelope(payload: dict[str, Any], bus: str) -> dict[str, Any]:
    """Retain causal coordinates without keeping transcript-bearing evidence."""
    result = {key: value for key, value in payload.items() if key not in ("text", "wav", "occurrences")}
    if "occurrences" in payload:
        result["occurrences"] = [
            {key: value for key, value in item.items() if key not in ("label", "acoustic_receipts")}
            for item in payload["occurrences"]
        ]
    result["bus"] = bus
    return result


def delivery_acknowledged(root: Path, lease_id: str, delivery_id: str,
                          expected_payload: dict[str, Any] | None = None) -> bool:
    path = root / "acknowledgments" / lease_id / f"{delivery_id}.json"
    try:
        metadata = path.lstat()
        if not S_ISREG(metadata.st_mode) or metadata.st_size > 1 << 20:
            return False
        receipt = read_reply_json(path, 1 << 20)
    except (OSError, ValueError, UnicodeError):
        return False
    if receipt.get("lease_id") != lease_id or receipt.get("delivery_id") != delivery_id:
        return False
    if set(receipt) == {"lease_id", "delivery_id"}:
        return True
    state = read_json(root / "leases" / f"{lease_id}.json")
    if (not state or state.get("schema") != LEASE_SCHEMA
            or state.get("lease_id") != lease_id
            or not all(isinstance(state.get(key), str) and state[key]
                       for key in ("provider", "provider_session_id", "bus"))
            or lease_identifier(state["provider"], state["provider_session_id"]) != lease_id):
        return False
    owner = {key: state[key] for key in ("lease_id", "provider", "provider_session_id", "bus")}
    matches = lambda value: isinstance(value, dict) and all(value.get(k) == v for k, v in owner.items())
    envelope = receipt.get("envelope")
    if not matches(receipt) or not matches(envelope) or envelope.get("delivery_id") != delivery_id:
        return False
    frozen = envelope.get("recipients")
    if frozen is not None and (not isinstance(frozen, list)
            or any(not isinstance(item, dict) for item in frozen)
            or not any(matches(item) for item in frozen)):
        return False
    pending = state.get("pending", [])
    original = expected_payload
    if original is None and isinstance(pending, list):
        original = next((item for item in pending if isinstance(item, dict)
                         and item.get("delivery_id") == delivery_id), None)
    return original is None or envelope == receipt_envelope(original, state["bus"])


def native_submission_id(receipt: dict[str, Any]) -> str | None:
    """Admit only the exact provider receipt for this thread, never message text."""
    session = receipt.get("provider_session_id")
    match = re.fullmatch(
        r"Queued message ([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}) for thread "
        + re.escape(str(session)) + r"\.", str(receipt.get("provider_receipt", "")),
    )
    identity = match.group(1) if match else None
    recorded = receipt.get("queued_submission_id")
    return identity if recorded is None or recorded == identity else None


def delete_native_queue_submission(root: Path, session: str, submission: str) -> bool:
    """Use the installed Rust transport and the running provider's own socket."""
    import shutil
    import subprocess

    executable = shutil.which("codex")
    if not executable:
        raise ValueError("native provider unavailable")
    version = subprocess.run([executable, "app-server", "daemon", "version"],
                             stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=5)
    if version.returncode != 0 or len(version.stdout) > 65536:
        raise ValueError("native provider status unavailable")
    status = json.loads(version.stdout)
    socket_path = status.get("socketPath")
    if status.get("status") != "running" or not isinstance(socket_path, str) or not Path(socket_path).is_absolute():
        raise ValueError("running native provider socket unavailable")
    result = subprocess.run(
        [reply_publisher_command(root), "bus", "withdraw-queued-message", "--socket", socket_path,
         "--thread", session, "--submission", submission],
        stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=12,
    )
    if result.returncode != 0 or len(result.stdout) > 65536:
        raise ValueError("native queue withdrawal unavailable")
    value = json.loads(result.stdout)
    if not isinstance(value, dict) or type(value.get("deleted")) is not bool:
        raise ValueError("invalid native queue withdrawal receipt")
    return value["deleted"]


def withdraw_acknowledged_queue(root: Path, lease_id: str, identity: str, *,
                               retry: bool = False, locked: bool = False) -> bool:
    """Cancel this owned pending submission after ACK; preserve immutable history.

    A busy sender owns the same lock and checks ACK after publishing its receipt.
    The existing follower retries failures without blocking its bus reader.
    """
    import subprocess

    if not re.fullmatch(r"[0-9a-f]{32}", lease_id) or not re.fullmatch(r"[0-9a-f]{24}", identity):
        return False
    if not delivery_acknowledged(root, lease_id, identity):
        return False
    directory = root / "wakeups" / lease_id
    if not directory.exists():
        return True
    if not locked:
        with (directory / f"{identity}.lock").open("a") as lock:
            os.chmod(lock.name, 0o600)
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return False
            return withdraw_acknowledged_queue(root, lease_id, identity, retry=retry, locked=True)
    path = directory / f"{identity}.json"
    receipt = read_json(path)
    if receipt is None:
        return not path.exists()
    state = read_json(root / "leases" / f"{lease_id}.json") or {}
    expected = {"lease_id": lease_id, "provider": "codex", "provider_session_id": state.get("provider_session_id"),
                "delivery_id": identity}
    if (state.get("provider") != "codex" or lease_identifier("codex", str(expected["provider_session_id"])) != lease_id
            or receipt.get("schema") != "codescribe.native-queue.receipt.v1"
            or any(receipt.get(k) != v for k, v in expected.items())):
        return False
    if receipt.get("queue_disposition") in ("removed", "not_pending"):
        return True
    if receipt.get("disposition") in ("rejected", "unavailable"):
        receipt["queue_disposition"] = "not_pending"
        atomic_json(path, receipt)
        return True
    submission = native_submission_id(receipt)
    if receipt.get("disposition") != "provider_accepted" or submission is None:
        receipt["queue_disposition"] = "unresolved"
        atomic_json(path, receipt)
        return False
    last_attempt = receipt.get("withdrawal_attempt_at", 0)
    if not retry and isinstance(last_attempt, (int, float)) and time.time() - last_attempt < 30:
        return False
    receipt.update(queued_submission_id=submission, queue_disposition="pending",
                   withdrawal_attempt_at=time.time())
    atomic_json(path, receipt)
    try:
        deleted = delete_native_queue_submission(root, expected["provider_session_id"], submission)
        receipt.update(queue_disposition="removed" if deleted else "not_pending",
                       withdrawal_completed_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
    except (OSError, ValueError, subprocess.SubprocessError):
        receipt["withdrawal_reason"] = "provider withdrawal unavailable; retry retained"
    atomic_json(path, receipt)
    return receipt["queue_disposition"] in ("removed", "not_pending")


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
    if not isinstance(pending, list) or any(not isinstance(item, dict) for item in pending):
        raise ValueError("invalid pending mailbox; nothing acknowledged")
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
        payload = next((item for item in pending if isinstance(item, dict)
                        and item.get("delivery_id") == delivery_id), None)
        if (delivery_id not in pending_ids or not payload
                or payload.get("lease_id") != lease_id
                or payload.get("provider") != args.provider.casefold()
                or payload.get("provider_session_id") != args.session
                or payload.get("bus", state["bus"]) != state["bus"]):
            raise ValueError(
                f"delivery {delivery_id} is not pending for this provider session; "
                "nothing acknowledged"
            )
    for delivery_id in unread:
        atomic_json(
            args.bridge_home / "acknowledgments" / lease_id / f"{delivery_id}.json",
            {"lease_id": lease_id, "delivery_id": delivery_id,
             "provider": args.provider.casefold(), "provider_session_id": args.session,
             "read_at": utc_now(),
             "bus": state["bus"],
             "envelope": receipt_envelope(next(payload for payload in pending
                                if payload.get("delivery_id") == delivery_id), state["bus"])},
        )
    for delivery_id in delivery_ids:
        withdrawn = withdraw_acknowledged_queue(args.bridge_home, lease_id, delivery_id, retry=True)
        emit({"kind": "acknowledged", "lease_id": lease_id, "delivery_id": delivery_id,
              "native_queue_settled": withdrawn})
    return 0


def read_pending_command(args: argparse.Namespace) -> int:
    """Read a bounded, complete snapshot. Only the conversation may ACK it."""
    lease_id = lease_identifier(args.provider, args.session)
    state = read_json(args.bridge_home / "leases" / f"{lease_id}.json") or {}
    if (state.get("schema") != LEASE_SCHEMA or state.get("lease_id") != lease_id
            or state.get("provider") != args.provider
            or state.get("provider_session_id") != args.session
            or not isinstance(state.get("bus"), str)
            or args.bus_overridden and state["bus"] != str(args.bus.expanduser().resolve(strict=False))):
        raise ValueError("mailbox does not belong to this provider session and bus")
    pending = state.get("pending")
    if not isinstance(pending, list) or any(not isinstance(row, dict) for row in pending):
        raise ValueError("invalid pending mailbox; nothing read")
    rows = []
    for row in pending:
        identity = row.get("delivery_id")
        if row.get("kind") in DRAFT_KINDS:
            continue
        if (not isinstance(identity, str) or not re.fullmatch(r"[0-9a-f]{24}", identity)
                or row.get("lease_id") != lease_id or row.get("provider") != args.provider
                or row.get("provider_session_id") != args.session
                or row.get("bus", state["bus"]) != state["bus"]):
            raise ValueError("foreign or invalid pending envelope; nothing read")
        owner = row.get("delivery_owner")
        if not isinstance(owner, dict) or any(owner.get(key) != row[key] for key in (
                "lease_id", "provider", "provider_session_id")):
            raise ValueError("foreign pending delivery owner; nothing read")
        if not delivery_acknowledged(args.bridge_home, lease_id, identity, row):
            rows.append(row)
    if not 1 <= args.read_limit <= 256 or not 1 <= args.read_bytes <= 16 * 1024 * 1024:
        raise ValueError("invalid --read-limit or --read-bytes")
    result = {"kind": "pending_read", "lease_id": lease_id, "provider": args.provider,
              "provider_session_id": args.session, "snapshot_cursor": state.get("cursor"),
              "deliveries": [], "read_delivery_ids": [], "remaining": len(rows)}
    if len(json.dumps(result, ensure_ascii=False).encode("utf-8")) > args.read_bytes:
        raise ValueError("snapshot metadata exceeds --read-bytes; increase the limit")
    for row in rows[:args.read_limit]:
        result["deliveries"].append(row)
        result["read_delivery_ids"].append(row["delivery_id"])
        result["remaining"] -= 1
        if len(json.dumps(result, ensure_ascii=False).encode("utf-8")) > args.read_bytes:
            result["deliveries"].pop()
            result["read_delivery_ids"].pop()
            result["remaining"] += 1
            if not result["deliveries"]:
                raise ValueError("complete envelope exceeds --read-bytes; increase the limit, then read before ACK")
            break
    emit(result)
    return 0


# =============================================================================
# § 9. Native queue wakeup
# =============================================================================


def effective_wakeup(args: argparse.Namespace) -> str:
    requested = getattr(args, "wakeup", "auto")
    if requested != "auto":
        return requested
    managed_follower = getattr(args, "attach", False) or getattr(args, "follower_channel", None)
    return "codex-queue" if args.provider == "codex" and managed_follower and not args.on_seal else "off"


def wakeup_configuration(args: argparse.Namespace) -> dict[str, Any]:
    return {
        "wakeup": effective_wakeup(args), "on_seal": args.on_seal,
        "helper_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    }


class NativeQueueWakeup:
    """Ordered provider submission, independent of microphone and agent ACK.

    Each delivery has a durable transport receipt. A provider timeout or an
    interrupted submission is ambiguous: never replay it automatically.
    Only the conversation can acknowledge reading the original envelope.
    """

    def __init__(self, root: Path, session: str, channel: str | None):
        from concurrent.futures import ThreadPoolExecutor

        self.root = root
        self.session = session
        self.channel = channel
        self.lease_id = lease_identifier("codex", session)
        self.executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="cs-native-queue")
        import threading
        self.withdrawals: set[str] = set()
        self.withdrawal_lock = threading.Lock()

    def enqueue_withdrawal(self, identity: str) -> None:
        receipt = read_json(self.root / "wakeups" / self.lease_id / f"{identity}.json") or {}
        last_attempt = receipt.get("withdrawal_attempt_at", 0)
        if (receipt.get("queue_disposition") == "unresolved"
                or isinstance(last_attempt, (int, float)) and time.time() - last_attempt < 30):
            return
        with self.withdrawal_lock:
            if identity in self.withdrawals:
                return
            self.withdrawals.add(identity)
        def complete(future: Any) -> None:
            with self.withdrawal_lock:
                self.withdrawals.discard(identity)
            self._report_error(future)
        self.executor.submit(withdraw_acknowledged_queue, self.root, self.lease_id, identity).add_done_callback(complete)

    def enqueue(self, payload: dict[str, Any], *, retry: bool = False) -> None:
        if payload.get("kind") not in TERMINAL_KINDS:
            return
        self.executor.submit(self._deliver, dict(payload), retry).add_done_callback(self._report_error)

    @staticmethod
    def _report_error(future: Any) -> None:
        if not future.cancelled() and future.exception() is not None:
            sys.stderr.write("cs-bus: native wakeup failed; delivery remains in its mailbox\n")

    def close(self, *, wait: bool = False) -> None:
        self.executor.shutdown(wait=wait, cancel_futures=not wait)

    def _deliver(self, payload: dict[str, Any], retry: bool) -> None:
        import shutil
        import subprocess

        identity = payload.get("delivery_id")
        owner = payload.get("delivery_owner")
        if not isinstance(identity, str) or not re.fullmatch(r"[0-9a-f]{24}", identity):
            return
        expected = {"lease_id": self.lease_id, "provider": "codex", "provider_session_id": self.session}
        if not isinstance(owner, dict) or any(owner.get(k) != v or payload.get(k) != v for k, v in expected.items()):
            return
        directory = self.root / "wakeups" / self.lease_id
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        receipt_path = directory / f"{identity}.json"
        with (directory / f"{identity}.lock").open("a") as lock:
            os.chmod(lock.name, 0o600)
            fcntl.flock(lock, fcntl.LOCK_EX)
            if delivery_acknowledged(self.root, self.lease_id, identity, payload):
                withdraw_acknowledged_queue(self.root, self.lease_id, identity, locked=True)
                return
            state = read_json(self.root / "leases" / f"{self.lease_id}.json") or {}
            if state.get("schema") != LEASE_SCHEMA or any(state.get(k) != v for k, v in expected.items()):
                return
            if payload not in state.get("pending", []):
                return
            previous = read_json(receipt_path)
            if receipt_path.exists() and previous is None:
                raise ValueError("unreadable wakeup receipt; preserved")
            if previous and (previous.get("disposition") == "provider_accepted" or not retry):
                return
            text = payload.get("text")
            if not isinstance(text, str) or not text.strip():
                return
            import shlex
            read_command = shlex.join([
                "cs-bus", "--read-pending", "--provider", "codex", "--session", self.session,
                "--bridge-home", str(self.root),
            ])
            ack_command = shlex.join([
                "cs-bus", "--provider", "codex", "--session", self.session,
                "--bridge-home", str(self.root), "--ack",
            ])
            label = str(self.channel or "?")
            name = str(payload.get("audience") or "agent")
            message = (
                f"Codescribe mailbox bell, channel {label} / {name}.\n"
                f"Trigger delivery: {identity}; emitted at: {payload.get('emitted_at')}.\n"
                "This is a notification, not a task or proof of reading. Read the current mailbox now:\n"
                f"{read_command}\n"
                "After reading complete envelopes, immediately ACK only their read_delivery_ids, "
                "before executing tasks or replying:\n"
                f"{ack_command} ID [ID ...]\n"
                "Read again until remaining is zero, then check once more for arrivals during the drain. "
                "Never ACK a truncated result. If the mailbox is empty, this is an obsolete bell: "
                "do not repeat a task or send a spoken reply for it. Interpret the original envelopes "
                "with their provenance and timestamps. Coverage is diagnostic; normal task permissions apply."
            )
            receipt = {
                "schema": "codescribe.native-queue.receipt.v1", **expected,
                "delivery_id": identity, "disposition": "requesting",
                "bus_emitted_at": payload.get("emitted_at"),
                "queue_requested_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "attempt": (previous or {}).get("attempt", 0) + 1,
            }
            atomic_json(receipt_path, receipt)
            executable = shutil.which("codex")
            if executable is None:
                receipt.update(disposition="unavailable", reason="codex executable not found on PATH")
            else:
                try:
                    result = subprocess.run(
                        [executable, "queue", "--thread", self.session, "--message", message],
                        stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30,
                    )
                    receipt.update(
                        disposition="provider_accepted" if result.returncode == 0 else "rejected",
                        exit_code=result.returncode,
                        provider_receipt=result.stdout.strip()[:2000],
                    )
                except subprocess.TimeoutExpired:
                    receipt.update(disposition="uncertain", reason="provider acceptance timed out")
                except OSError:
                    receipt.update(disposition="unavailable", reason="provider process could not start")
            receipt["completed_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            if receipt["disposition"] == "provider_accepted":
                receipt["queued_submission_id"] = native_submission_id(receipt)
            atomic_json(receipt_path, receipt)
            if delivery_acknowledged(self.root, self.lease_id, identity, payload):
                withdraw_acknowledged_queue(self.root, self.lease_id, identity, locked=True)
            if receipt["disposition"] != "provider_accepted":
                sys.stderr.write(f"cs-bus: native wakeup {receipt['disposition']} for {identity}; retained, see --status\n")


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


# =============================================================================
# § 10. Follower run loop
# =============================================================================


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
        if follower_channel:
            lease.wakeup_configuration = wakeup_configuration(args)
            lease.persist(active=True)
        emit_follower(lease.attach_receipt(), events_path, follower_channel)

    # One normalizer for the whole run: the evidence grain is stateful (it
    # remembers each session's document and whether its seal was reported), and
    # a fresh one per line would re-emit the entire document every time.
    normalizer = EvidenceNormalizer()
    if lease:
        # Prune first: a draft left over from a message that was sealed and
        # acknowledged in an earlier run must not be restored as an unclosed
        # document for a session that is already settled.
        lease.prune_settled_drafts()
        normalizer.restore_channel_documents(list(lease.unclosed_channel_messages.values()))
        # Mailbox drafts are receipt history, not evidence of an open capture.
        # A final queued before cursor commit still suppresses its replay.
        normalizer.restore_channel_documents([
            payload for payload in lease.pending.values() if payload.get("status") == SEALED
        ])
    native = (
        NativeQueueWakeup(args.bridge_home, args.session, follower_channel)
        if lease and args.follow and args.provider == "codex"
        else None
    )
    event_trigger: BusEventTrigger | None = None
    deferred: tuple[list[dict[str, Any]], int | None] | None = None
    recipients: set[str] | None = None
    human_drafts: dict[tuple[Any, Any], dict[str, Any]] = {}

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
            if payload.get("kind") in DRAFT_KINDS:
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
                    if native and effective_wakeup(args) == "codex-queue":
                        native.enqueue(payload)
                    if args.on_seal and payload.get("kind") in TERMINAL_KINDS:
                        fire_seal_hook(args.on_seal, payload)
                remaining.pop(0)
        except BufferError:
            # Preserve the already normalized envelopes. Re-normalizing their
            # raw evidence rows would suppress a terminal phase — or a
            # coverage-refused flush — on retry.
            deferred = (remaining, next_cursor)
            raise
        if lease and next_cursor is not None:
            lease.unclosed_channel_messages = {
                session: document for session, document in normalizer.channel_documents().items()
                if lease.admits_channel_event(document)
            }
            lease.persist(
                active=True,
                cursor=next_cursor,
                sequence=payloads[-1].get("sequence") if payloads else None,
            )
        deferred = None

    def handle(raw: str, next_cursor: int | None = None) -> None:
        nonlocal name, hear_all
        # Coverage-refused flushes precede the row that triggered them: they
        # carry utterances older than the channel receipt on this line.
        events = normalized_revision_events(raw, normalizer)
        if normalizer.storage.pending:
            return
        payloads: list[dict[str, Any]] = []
        for event in events:
            if lease and not lease.admits_channel_event(event):
                continue
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
                lease.unclosed_channel_messages = {
                    session: document for session, document in normalizer.channel_documents().items()
                    if lease.admits_channel_event(document)
                }
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
            lease.collect_acknowledgments(native)
            for payload in lease.pending.values():
                if delivery_acknowledged(lease.root, lease.lease_id, payload["delivery_id"], payload):
                    continue
                publish(payload)
                if native and effective_wakeup(args) == "codex-queue":
                    native.enqueue(payload)
            flush_human_drafts()
        if args.once:
            last = None
            recipients = registered_recipients(args.bridge_home, path)
            for raw in replay(path):
                events = normalized_revision_events(raw, normalizer)
                for event in events:
                    if lease and not lease.admits_channel_event(event):
                        continue
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
            if normalizer.storage.pending:
                raise ValueError("incomplete storage record")
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
                offset = generation_metadata(path).st_size
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
                lease.collect_acknowledgments(native)
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
                if normalizer.storage.pending:
                    raise ValueError("incomplete storage record")
                return 0
            if lease and time.monotonic() - last_heartbeat >= 1.0:
                lease.persist(active=True, cursor=lease.cursor if normalizer.storage.pending else offset)
                last_heartbeat = time.monotonic()
            assert event_trigger is not None
            event_trigger.wait(timeout=1.0)
    except KeyboardInterrupt:
        return 130
    except BufferError as error:
        sys.stderr.write(f"bus-demux: {error}\n")
        return 4
    finally:
        if native:
            native.close()
        if event_trigger:
            event_trigger.close()
        if lease:
            lease.close()


# =============================================================================
# § 11. Speech: credentials, voice profiles, TTS and playback
# =============================================================================


def _xai_speech_key() -> str | None:
    """Explicit Keychain key first, then the Grok CLI's xAI OIDC session."""
    import subprocess

    probe = subprocess.run(
        ["/usr/bin/security", "find-generic-password", "-s",
         "com.vetcoders.codescribe", "-a", "LLM_XAI_API_KEY", "-w"],
        capture_output=True, text=True,
    )
    key = probe.stdout.strip() if probe.returncode == 0 else ""
    if key:
        return key
    try:
        data = json.loads((Path.home() / ".grok" / "auth.json").read_text())
        for endpoint, value in data.items():
            if (
                str(endpoint).split("::", 1)[0].rstrip("/") == "https://auth.x.ai"
                and
                isinstance(value, dict)
                and value.get("auth_mode") == "oidc"
                and value.get("key")
            ):
                return str(value["key"])
    except (OSError, ValueError):
        pass
    return None


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
    control: ReplyPlaybackControl | None = None,
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
    return _speak_pcm(*_tts_exchange(request), playback_root=playback_root, bus=bus, control=control)


def _speak_pcm(
    pcm: bytes | None,
    error: str | None,
    reason: str | None,
    *,
    playback_root: Path | None = None,
    bus: Path | None = None,
    control: ReplyPlaybackControl | None = None,
) -> tuple[bool, str | None, str | None]:
    if pcm is None:
        return False, error, reason
    return _play_pcm_24k(pcm, playback_root=playback_root, bus=bus, control=control)


def _acquire_playback_lock(descriptor: int, control: ReplyPlaybackControl | None = None) -> bool:
    """Bound the wait with a monotonic clock; the kernel owns exclusivity."""
    deadline = time.monotonic() + PLAYBACK_WAIT_SECONDS
    while True:
        if control is not None and control.stopped():
            return False
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except BlockingIOError:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            time.sleep(min(0.05, remaining))


def _lifecycle_checkpoint(bus: Path, root: Path) -> Path:
    # Playback's existing flock serializes cursor writers. This stores only
    # derived parser progress; every admission still reads the canonical buses.
    key = str(bus.expanduser().resolve(strict=False)).encode("utf-8")
    return root / "runtime" / f"lifecycle-{hashlib.sha256(key).hexdigest()}.json"


def _load_lifecycle_cursor(bus: Path, root: Path, deadline: float) -> dict[str, Any]:
    import stat

    path = _lifecycle_checkpoint(bus, root)
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as handle:
            metadata = os.fstat(handle.fileno())
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.getuid()
                or metadata.st_mode & 0o077
                or metadata.st_size > 1024 * 1024
                or time.monotonic() >= deadline
            ):
                return {}
            payload = json.loads(handle.read(1024 * 1024 + 1))
        canonical = str(bus.expanduser().resolve(strict=False))
        if (
            time.monotonic() >= deadline
            or not isinstance(payload, dict)
            or payload.get("schema") != "codescribe.lifecycle-cursor.v1"
            or payload.get("bus") != canonical
            or payload.get("sealed_is_idle") is not False
            or not isinstance(payload.get("sources"), dict)
        ):
            return {}
        cursor: dict[str, Any] = {"channel_buses": {}}
        for source, saved in payload["sources"].items():
            if time.monotonic() >= deadline:
                return {}
            if not isinstance(source, str) or not Path(source).is_absolute():
                return {}
            if not isinstance(saved, dict) or set(saved) != {
                "identity", "offset", "size", "mtime_ns", "head", "edge",
                "caught_up", "open_cli", "live_app", "open_channels",
            }:
                return {}
            identity = saved.get("identity")
            if (
                not isinstance(identity, list)
                or len(identity) != 4
                or any(type(value) is not int or value < 0 for value in identity[:2])
                or identity[2] is not False
                or (
                    identity[3] is not None
                    and (
                        type(identity[3]) not in (int, float)
                        or not 0 <= identity[3] < float("inf")
                    )
                )
                or any(
                    type(saved.get(key)) is not int or saved[key] < 0
                    for key in ("offset", "size", "mtime_ns")
                )
                or saved["offset"] > saved["size"]
                or any(
                    not isinstance(saved.get(key), str)
                    or re.fullmatch(r"[0-9a-f]{64}", saved[key]) is None
                    for key in ("head", "edge")
                )
                or type(saved.get("caught_up")) is not bool
                or not isinstance(saved.get("open_cli"), dict)
                or not isinstance(saved.get("open_channels"), dict)
            ):
                return {}
            live_app = saved.get("live_app")
            if live_app is not None and (not isinstance(live_app, str) or not live_app):
                return {}
            if any(
                not isinstance(sid, str) or not sid
                or (activity is not None and not isinstance(activity, str))
                for sid, activity in saved["open_cli"].items()
            ):
                return {}
            channels = saved["open_channels"]
            if any(
                not isinstance(sid, str) or not sid
                or not isinstance(value, list) or len(value) != 2
                or not isinstance(value[0], str) or not value[0]
                or (value[1] is not None and not isinstance(value[1], str))
                for sid, value in channels.items()
            ):
                return {}
            state = dict(saved)
            state["identity"] = tuple(identity)
            state["open_channels"] = {sid: tuple(value) for sid, value in channels.items()}
            if source == canonical:
                cursor.update(state)
            else:
                cursor["channel_buses"][Path(source)] = state
        return cursor
    except (OSError, ValueError, TypeError):
        # Invalid storage grants nothing: rebuild from the canonical history.
        return {}


def _save_lifecycle_cursor(bus: Path, root: Path, cursor: dict[str, Any]) -> None:
    canonical = str(bus.expanduser().resolve(strict=False))
    sources = {
        str(source): state
        for source, state in cursor.get("channel_buses", {}).items()
        if state.get("identity") is not None
    }
    if cursor.get("identity") is not None:
        sources[canonical] = {
            key: value for key, value in cursor.items() if key != "channel_buses"
        }
    try:
        atomic_json(_lifecycle_checkpoint(bus, root), {
            "schema": "codescribe.lifecycle-cursor.v1",
            "bus": canonical,
            "sealed_is_idle": False,
            "sources": sources,
        })
    except OSError:
        # The current parser verdict is independent of checkpoint availability.
        # A later process must replay when progress could not be persisted.
        pass


def installation_idle_with_checkpoint(bus: Path, *, bridge_root: Path) -> bool:
    """Reuse the existing lifecycle observer, then verify its current suffix.

    Its session-ended predicate is sufficient for installation. A busy or
    incomplete retained cursor grants nothing; keep its open identities.
    """
    deadline = time.monotonic() + TAKE_WAIT_SECONDS
    cursor = _load_lifecycle_cursor(bus, bridge_root, deadline)
    if cursor:
        idle = installation_idle(
            bus, sealed_is_idle=False, cursor=cursor, bridge_root=bridge_root,
            deadline=deadline,
        )
        _save_lifecycle_cursor(bus, bridge_root, cursor)
        return idle
    return installation_idle(bus, bridge_root=bridge_root)


def _wait_for_take_end(
    bus: Path, cursor: dict[str, Any], *, bridge_root: Path | None = None,
    control: ReplyPlaybackControl | None = None,
) -> bool:
    deadline = time.monotonic() + TAKE_WAIT_SECONDS
    root = bridge_root if bridge_root is not None else bridge_home()
    if not cursor:
        cursor.update(_load_lifecycle_cursor(bus, root, deadline))
    while time.monotonic() < deadline:
        if control is not None and control.stopped():
            return False
        previous = (cursor.get("offset"), {
            source: state.get("offset")
            for source, state in cursor.get("channel_buses", {}).items()
        })
        idle = installation_idle(
            bus, sealed_is_idle=False, cursor=cursor, bridge_root=root,
            deadline=deadline,
        )
        _save_lifecycle_cursor(bus, root, cursor)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        if idle:
            return True
        current = (cursor.get("offset"), {
            source: state.get("offset")
            for source, state in cursor.get("channel_buses", {}).items()
        })
        # Continue a cold scan without adding a poll delay to each slice.
        # A live take or a stalled malformed row uses the normal poll cadence.
        if current != previous:
            continue
        time.sleep(min(PLAYBACK_POLL_SECONDS, remaining))
    return False


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
    control: ReplyPlaybackControl | None = None,
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
        if not _acquire_playback_lock(lock_descriptor, control):
            return (False, "playback stopped", "stopped") if control and control.stopped() else (False, "playback wait timed out", "playback_busy")
        # A take may have started during synthesis or the lock wait.
        speech_bus = bus if bus is not None else bus_path()
        cursor: dict[str, Any] = {}
        if not _wait_for_take_end(speech_bus, cursor, bridge_root=root, control=control):
            return (False, "playback stopped", "stopped") if control and control.stopped() else (False, "live take wait timed out", "take_live")
        if control is not None:
            if control.stopped():
                return False, "playback stopped", "stopped"
            control.publish("playing")
        player = subprocess.Popen(
            ["afplay", wav_path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )
        while True:
            if control is not None and control.stopped():
                _stop_playback(player)
                return False, "playback stopped", "stopped"
            idle = installation_idle(
                speech_bus, sealed_is_idle=False, cursor=cursor, bridge_root=root,
                deadline=time.monotonic() + PLAYBACK_POLL_SECONDS,
            )
            _save_lifecycle_cursor(speech_bus, root, cursor)
            if not idle:
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
    control: ReplyPlaybackControl | None = None,
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
    return _speak_pcm(*_tts_exchange(request), playback_root=playback_root, bus=bus, control=control)


# =============================================================================
# § 12. Reply publication and spoken replies
# =============================================================================


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


def read_reply_json(path: Path, limit: int = 16 << 20) -> dict[str, Any]:
    """Bounded private receipt reads; never deserialize the live journal here."""
    with path.open("rb") as handle:
        raw = handle.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("oversized reply receipt")
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("invalid reply receipt")
    return value


def reply_publisher_command(root: Path) -> str:
    """Find the existing Rust command without depending on a GUI shell's PATH."""
    import shutil

    runtime = root / "runtime"
    bundled = runtime / "bin" / "codescribe"
    manifest_path = runtime / "manifest.json"
    bundle_present = any(path.exists() or path.is_symlink() for path in (bundled, manifest_path))
    if bundle_present and (not bundled.is_file() or not manifest_path.is_file() or manifest_path.is_symlink()):
        raise ValueError("bundled canonical publisher or manifest is missing or invalid")
    if bundled.is_file() and manifest_path.is_file():
        manifest = read_reply_json(manifest_path, 4 << 20)
        entries = manifest.get("files", [])
        entry = next((item for item in entries if isinstance(item, dict)
                      and item.get("path") == "bin/codescribe"), None) if isinstance(entries, list) else None
        if manifest.get("schema") == "codescribe.agent-bridge.bundle.v1" and entry:
            metadata = bundled.stat()
            size = entry.get("bytes")
            if (type(size) is not int or not 0 < size <= 256 << 20
                    or metadata.st_size != size or bundled.is_symlink()
                    or not os.access(bundled, os.X_OK)):
                raise ValueError("bundled canonical publisher ownership is invalid")
            digest = hashlib.sha256()
            with bundled.open("rb") as handle:
                remaining = size
                while remaining:
                    block = handle.read(min(1 << 20, remaining))
                    if not block:
                        raise ValueError("bundled canonical publisher is incomplete")
                    digest.update(block)
                    remaining -= len(block)
                if handle.read(1):
                    raise ValueError("bundled canonical publisher changed during verification")
            if digest.hexdigest() != entry.get("sha256"):
                raise ValueError("bundled canonical publisher digest does not match its manifest")
            return str(bundled.resolve(strict=True))
        raise ValueError("bundled canonical publisher manifest schema or entry is invalid")
    discovered = shutil.which("codescribe")
    if discovered:
        return discovered
    for candidate in (Path.home() / ".cargo" / "bin" / "codescribe",
                      Path.home() / ".local" / "bin" / "codescribe"):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    raise OSError("canonical codescribe bus publisher is unavailable")


def publish_reply_event(bus: Path, event: dict[str, Any], *, bridge_root: Path | None = None) -> dict[str, Any]:
    """The Rust generation/chunk/private append owner is the only bus writer."""
    import subprocess

    executable = reply_publisher_command(bridge_root if bridge_root is not None else bridge_home())
    try:
        result = subprocess.run(
            [executable, "bus", "append-event", "--bus", str(bus.resolve(strict=False))],
            input=json.dumps(event, ensure_ascii=False), capture_output=True,
            text=True, timeout=30, check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise OSError("canonical bus publication timed out; speech did not start") from error
    if result.returncode != 0:
        raise OSError("canonical bus publication refused; speech did not start")
    try:
        receipt = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ValueError("canonical bus publication has no receipt") from error
    if not isinstance(receipt, dict):
        raise ValueError("canonical bus publication has invalid receipt")
    return receipt


def reply_delivery_envelope(args: argparse.Namespace) -> tuple[Path, dict[str, Any] | None]:
    lease_id = lease_identifier(args.provider, args.session)
    lease_path = args.bridge_home / "leases" / f"{lease_id}.json"
    state = read_reply_json(lease_path) if lease_path.exists() else {}
    if state and (state.get("schema") != LEASE_SCHEMA
                  or state.get("lease_id") != lease_id
                  or state.get("provider") != args.provider.casefold()
                  or state.get("provider_session_id") != args.session):
        raise ValueError("reply lease does not belong to this provider session")
    if state and (not isinstance(state.get("bus"), str) or not state["bus"].strip()):
        raise ValueError("reply lease has an invalid bus path")
    pending = state.get("pending", [])
    if not isinstance(pending, list) or any(not isinstance(item, dict) for item in pending):
        raise ValueError("reply lease has an invalid pending mailbox")
    bus = (args.bus if args.bus_overridden or not state.get("bus")
           else Path(state["bus"])).expanduser().resolve(strict=False)
    if state and state.get("bus") != str(bus):
        raise ValueError("reply bus does not belong to this provider session")
    identity = args.reply_to
    if identity is None:
        return bus, None
    envelope = next((item for item in pending if item.get("delivery_id") == identity), None)
    if envelope is None:
        marker = args.bridge_home / "acknowledgments" / lease_id / f"{identity}.json"
        receipt = read_reply_json(marker) if marker.exists() else {}
        if (receipt.get("lease_id") != lease_id or receipt.get("delivery_id") != identity
                or receipt.get("provider") != args.provider.casefold()
                or receipt.get("provider_session_id") != args.session
                or receipt.get("bus") != str(bus)):
            raise ValueError("reply delivery has no owned envelope")
        envelope = receipt.get("envelope")
    if (not isinstance(envelope, dict) or envelope.get("delivery_id") != identity
            or envelope.get("lease_id") != lease_id
            or envelope.get("provider") != args.provider.casefold()
            or envelope.get("provider_session_id") != args.session
            or envelope.get("bus", state.get("bus")) != str(bus)):
        raise ValueError("reply delivery does not belong to this provider session and bus")
    frozen = envelope.get("recipients")
    if frozen is not None and (not isinstance(frozen, list)
            or any(not isinstance(owner, dict) for owner in frozen) or not any(
            owner.get("provider") == args.provider.casefold()
            and owner.get("provider_session_id") == args.session
            and owner.get("lease_id") == lease_id and owner.get("bus") == str(bus)
            for owner in frozen)):
        raise ValueError("reply owner was not admitted for this channel utterance")
    return bus, envelope


def load_published_reply(args: argparse.Namespace, identity: str) -> tuple[Path, dict[str, Any]]:
    source = read_reply_json(args.bridge_home / "runtime" / "reply-sources" / f"{identity}.json")
    bus = Path(source.get("bus", ""))
    lease_id = lease_identifier(args.provider, args.session)
    if (source.get("schema") != REPLY_SOURCE_SCHEMA or source.get("reply_id") != identity
            or source.get("lease_id") != lease_id
            or source.get("provider") != args.provider.casefold()
            or source.get("provider_session_id") != args.session or not bus.is_absolute()
            or (args.bus_overridden and bus != args.bus.expanduser().resolve(strict=False))):
        raise ValueError("reply source does not belong to this provider session and bus")
    receipt = source.get("source", {})
    offset, length = receipt.get("offset"), receipt.get("length")
    if type(offset) is not int or offset < 0 or type(length) is not int or not 0 < length <= REPLY_READ_LIMIT:
        raise ValueError("invalid reply source coordinates")
    with GenerationFile(bus) as handle:
        manifest = read_reply_json(Path(str(bus) + ".generations.json"), 4 << 20)
        if (manifest.get("schema") != "codescribe.bus-generations.v1"
                or manifest.get("root") != str(bus)
                or not isinstance(receipt.get("stream_id"), str) or not receipt["stream_id"]
                or manifest.get("stream_id") != receipt["stream_id"]
                or (handle.dev, handle.ino) != (receipt.get("stream_dev"), receipt.get("stream_inode"))):
            raise ValueError("reply source stream was replaced")
        handle.seek(offset)
        raw = handle.read(length)
    if len(raw) != length or not raw.endswith(b"\n"):
        raise ValueError("reply source is incomplete")
    decoder = ChunkDecoder()
    events = []
    for line in raw.splitlines():
        value = decoder.feed(json.loads(line))
        if value is not None:
            events.append(value)
    if decoder.pending or len(events) != 1:
        raise ValueError("reply source is not one complete event")
    reply = events[0]
    if (reply.get("schema") != AGENT_REPLY_SCHEMA or reply.get("kind") != "agent_reply"
            or reply.get("reply_id") != identity or reply.get("lease_id") != lease_id
            or reply.get("provider") != args.provider.casefold()
            or reply.get("provider_session_id") != args.session):
        raise ValueError("canonical reply does not match its source owner")
    return bus, reply


def playback_mute_path(root: Path, provider: str, session: str, bus: Path) -> Path:
    identity = "\0".join((provider.casefold(), session, str(bus.resolve(strict=False))))
    key = hashlib.sha256(identity.encode("utf-8")).hexdigest()[:24]
    return root / "runtime" / "playback-mutes" / f"{key}.json"


def agent_playback_muted(root: Path, provider: str, session: str, bus: Path) -> bool:
    path = playback_mute_path(root, provider, session, bus)
    try:
        row = read_reply_json(path, 65536)
        if (row.get("schema") != PLAYBACK_MUTE_SCHEMA
                or row.get("provider") != provider.casefold()
                or row.get("provider_session_id") != session
                or row.get("lease_id") != lease_identifier(provider, session)
                or row.get("bus") != str(bus.resolve(strict=False))
                or type(row.get("muted")) is not bool):
            return True
        return row["muted"]
    except FileNotFoundError:
        return False
    except (OSError, ValueError):
        # An unreadable mute receipt never gives permission to speak.
        return True


def set_agent_muted(args: argparse.Namespace) -> int:
    bus = args.bus.expanduser().resolve(strict=False)
    lease_id = lease_identifier(args.provider, args.session)
    lease = read_reply_json(args.bridge_home / "leases" / f"{lease_id}.json")
    if (lease.get("schema") != LEASE_SCHEMA or lease.get("lease_id") != lease_id
            or lease.get("provider") != args.provider.casefold()
            or lease.get("provider_session_id") != args.session
            or lease.get("bus") != str(bus)):
        raise ValueError("playback mute does not belong to this provider session and bus")
    row = {"schema": PLAYBACK_MUTE_SCHEMA, "provider": args.provider.casefold(),
           "provider_session_id": args.session, "lease_id": lease_id,
           "bus": str(bus), "muted": args.mute_agent, "emitted_at": utc_now()}
    atomic_json(playback_mute_path(args.bridge_home, args.provider, args.session, bus), row)
    emit(row)
    return 0


class ReplyPlaybackControl:
    """One request ticket controls only its own child of the serialized player."""
    def __init__(self, args: argparse.Namespace, bus: Path, reply: dict[str, Any], ticket: str,
                 *, automatic: bool = False):
        self.bus, self.reply, self.ticket = bus, reply, ticket
        self.automatic = automatic
        self.root = args.bridge_home
        self.path = self.root / "runtime" / "reply-playback" / f"{reply['reply_id']}.{ticket}.json"
        self.stop_path = self.path.with_suffix(".stop.json")
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.descriptor = os.open(self.path.with_suffix(".lock"), os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(self.descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if self.path.exists():
                raise ValueError("playback ticket was already admitted; use a new ticket")
        except BaseException:
            os.close(self.descriptor)
            raise

    def owner(self) -> dict[str, Any]:
        return {key: self.reply[key] for key in
                ("reply_id", "provider", "provider_session_id", "lease_id")}

    def stopped(self) -> bool:
        if self.muted():
            return True
        if not self.stop_path.exists():
            return False
        stop = read_reply_json(self.stop_path, 65536)
        return (stop.get("schema") == REPLY_CONTROL_SCHEMA
                and stop.get("playback_ticket") == self.ticket
                and all(stop.get(key) == value for key, value in self.owner().items()))

    def muted(self) -> bool:
        return self.automatic and agent_playback_muted(
            self.root, self.reply["provider"], self.reply["provider_session_id"], self.bus)

    def publish(self, state: str, *, error: str | None = None, reason: str | None = None) -> None:
        event = {"schema": AGENT_REPLY_PLAYBACK_SCHEMA, "kind": "agent_reply_playback",
                 "emitted_at": utc_now(), **self.owner(), "playback_ticket": self.ticket,
                 "state": state, "spoken": state == "spoken"}
        if error:
            event["tts_error"] = error
        if reason:
            event["reason"] = reason
        publish_reply_event(self.bus, event, bridge_root=self.root)
        atomic_json(self.path, {**event, "schema": REPLY_CONTROL_SCHEMA, "bus": str(self.bus)})
        emit(event)

    def close(self) -> None:
        os.close(self.descriptor)


def speak_published_reply(args: argparse.Namespace, bus: Path, reply: dict[str, Any], ticket: str,
                          *, automatic: bool = False) -> int:
    control = ReplyPlaybackControl(args, bus, reply, ticket, automatic=automatic)
    try:
        control.publish("waiting")
        if control.stopped():
            spoken, error, reason = False, "playback stopped", "stopped"
        else:
            speaker = _speak_openai if reply["tts_vendor"] == "openai" else _speak_xai
            spoken, error, reason = speaker(reply["text"], reply["voice"], reply["speed"],
                                            playback_root=args.bridge_home, bus=bus_path(), control=control)
            if control.stopped() and not spoken:
                error, reason = "playback stopped", "stopped"
        if not spoken and control.muted():
            error, reason = None, "muted"
        state = ("spoken" if spoken else "stopped" if reason == "stopped" else
                 "refused" if reason in ("take_live", "take_started", "playback_busy", "muted") else "failed")
        control.publish(state, error=error, reason=reason)
        return 0 if spoken or reason == "muted" else 5
    except Exception as error:
        control.publish("failed", error=f"speech failed ({error.__class__.__name__})",
                        reason="speech_exception")
        raise
    finally:
        control.close()


def play_reply_command(args: argparse.Namespace) -> int:
    bus, reply = load_published_reply(args, args.play_reply)
    return speak_published_reply(args, bus, reply, args.playback_ticket)


def stop_reply_command(args: argparse.Namespace) -> int:
    identity, ticket = args.stop_reply, args.playback_ticket
    path = args.bridge_home / "runtime" / "reply-playback" / f"{identity}.{ticket}.json"
    control = read_reply_json(path, 65536)
    owner = {"reply_id": identity, "provider": args.provider.casefold(),
             "provider_session_id": args.session,
             "lease_id": lease_identifier(args.provider, args.session)}
    bus = Path(control.get("bus", ""))
    if (control.get("schema") != REPLY_CONTROL_SCHEMA or not bus.is_absolute()
            or (args.bus_overridden and bus != args.bus.expanduser().resolve(strict=False))
            or control.get("playback_ticket") != ticket
            or any(control.get(key) != value for key, value in owner.items())):
        raise ValueError("stop ticket does not belong to this reply playback owner")
    if control.get("state") not in ("waiting", "playing"):
        raise ValueError("playback ticket is already terminal")
    with path.with_suffix(".lock").open("rb") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            pass
        else:
            raise ValueError("playback ticket has no running owner")
    atomic_json(path.with_suffix(".stop.json"),
                {"schema": REPLY_CONTROL_SCHEMA, **owner, "playback_ticket": ticket})
    emit({"kind": "stop_requested", **owner, "playback_ticket": ticket})
    return 0


def say_reply(args: argparse.Namespace) -> int:
    """Durably publish reply text before attempting synthesis or playback."""
    bus, envelope = reply_delivery_envelope(args)
    lease_id = lease_identifier(args.provider, args.session)
    owner = next((item for item in envelope.get("recipients", [])
                  if item.get("provider") == args.provider.casefold()
                  and item.get("provider_session_id") == args.session
                  and item.get("lease_id") == lease_id
                  and item.get("bus") == str(bus)), None) if envelope else None
    historical_name = ((owner.get("name") or owner.get("audience")) if owner
                       else envelope.get("audience")) if envelope else None
    name = historical_name if historical_name and historical_name != "*" else args.name
    profile = voice_profile(args.bridge_home, name)
    voice = args.voice or str(profile["voice"])
    speed = args.speed if args.speed is not None else float(profile["speed"])
    vendor = args.tts_vendor or str(profile.get("provider") or "xai")
    reply: dict[str, Any] = {
        "schema": AGENT_REPLY_SCHEMA, "kind": "agent_reply", "emitted_at": utc_now(),
        "reply_id": os.urandom(12).hex(), "name": name,
        "provider": args.provider.casefold(), "provider_session_id": args.session,
        "lease_id": lease_identifier(args.provider, args.session), "text": args.say,
        "voice": voice, "speed": speed, "tts_vendor": vendor, "spoken": False,
        "association": "addressed" if envelope else "unsolicited",
        "delivery_id": args.reply_to,
    }
    if envelope:
        if owner:
            reply["channel"] = owner.get("channel")
        for key in ("source_event_id", "utterance_id", "session_id", "occurrence_session_id",
                    "capture_epoch", "sample_start", "sample_end", "document_index",
                    "audience", "broadcast_id", "recipients", "message_id"):
            if key in envelope:
                reply[key] = envelope[key]
    receipt = publish_reply_event(bus, reply, bridge_root=args.bridge_home)
    if (any(type(receipt.get(key)) is not int or receipt[key] < 0
            for key in ("stream_dev", "stream_inode", "offset", "length"))
            or not 0 < receipt["length"] <= REPLY_READ_LIMIT
            or not isinstance(receipt.get("stream_id"), str) or not receipt["stream_id"]):
        raise ValueError("canonical reply publication has invalid source coordinates")
    atomic_json(args.bridge_home / "runtime" / "reply-sources" / f"{reply['reply_id']}.json",
                {"schema": REPLY_SOURCE_SCHEMA, "bus": str(bus), "source": receipt,
                 **{key: reply[key] for key in ("reply_id", "provider", "provider_session_id", "lease_id")}})
    emit(reply)
    return speak_published_reply(args, bus, reply, os.urandom(12).hex(), automatic=True)


# =============================================================================
# § 13. Messages: Founder typed text and agent peer text
# =============================================================================


def send_text_command(args: argparse.Namespace) -> int:
    """Publish explicit user text to the selected immutable channel owner."""
    root = args.bridge_home
    channel = str(args.channel)
    bus = args.bus.expanduser().resolve(strict=False)
    lease_id = lease_identifier(args.provider, args.session)
    if args.lease != lease_id:
        raise ValueError("selected conversation owner no longer matches")
    path = root / AUDIENCE_BINDING_FILENAME
    with path.with_suffix(".lock").open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_SH)
        state = read_json(path) or {}
        binding = state.get("bindings", {}).get(channel)
        lease = read_json(root / "leases" / f"{lease_id}.json") or {}
        if (state.get("schema") != AUDIENCE_BINDING_SCHEMA or not isinstance(binding, dict)
                or binding.get("provider") != args.provider.casefold()
                or binding.get("provider_session_id") != args.session
                or Path(binding.get("bus") or "").resolve(strict=False) != bus
                or lease.get("schema") != LEASE_SCHEMA or lease.get("lease_id") != lease_id
                or lease.get("provider") != args.provider.casefold()
                or lease.get("provider_session_id") != args.session
                or lease.get("bus") != str(bus)
                or not live_follower_pid(root, lease_id)):
            raise ValueError("channel was rebound or its agent is not listening; draft retained")
        text = sys.stdin.buffer.read(65537)
        if len(text) > 65536:
            raise ValueError("message exceeds 64 KiB; draft retained")
        text = text.decode("utf-8")
        if not text.strip():
            raise ValueError("empty message")
        audience = binding.get("audience")
        if not isinstance(audience, str) or not audience:
            raise ValueError("channel has no audience")
        identity = os.urandom(12).hex()
        owner = {"provider": args.provider.casefold(), "provider_session_id": args.session,
                 "lease_id": lease_id, "channel": channel, "audience": audience,
                 "name": audience, "bus": str(bus)}
        event = {"schema": AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
                 **owner, "message_id": identity, "source_event_id": identity,
                 "source": "typed", "text": text, "emitted_at": utc_now(),
                 "recipients": [owner]}
        receipt = publish_reply_event(bus, event, bridge_root=root)
        if (any(type(receipt.get(key)) is not int or receipt[key] < 0
                for key in ("stream_dev", "stream_inode", "offset", "length"))
                or not 0 < receipt["length"] <= REPLY_READ_LIMIT
                or not isinstance(receipt.get("stream_id"), str) or not receipt["stream_id"]):
            raise ValueError("publication has no durable receipt; do not automatically resend")
    emit({"kind": "message_published", "message_id": identity, "source": receipt})
    return 0


def send_peer_command(args: argparse.Namespace) -> int:
    """Publish one agent-authored text message to a peer mailbox or channel 0.

    The wire shape is an agent reply — the canonical publisher's only
    agent-authored text lane — extended with explicit peer routing: `peer_to`
    names the one recipient of each per-bus copy, `sender` carries the
    authoring lease, and the top-level `channel` records the origin ("0" for
    a broadcast, the target's digit for a direct). Followers admit it as a
    "message" delivery with state_change_allowed=False.

    A broadcast (`--to 0`) writes one copy per bound peer bus, excluding the
    sender's own lease; each copy shares one message identity, so a reader
    bound to several channels still dedupes it within its lease.
    """
    root = args.bridge_home
    lease_id = lease_identifier(args.provider, args.session)
    lease = read_json(root / "leases" / f"{lease_id}.json") or {}
    if (lease.get("schema") != LEASE_SCHEMA or lease.get("lease_id") != lease_id
            or lease.get("provider") != args.provider.casefold()
            or lease.get("provider_session_id") != args.session):
        raise ValueError("sender has no lease here; attach before sending")
    sender_name = lease.get("name")
    if not isinstance(sender_name, str) or not sender_name:
        raise ValueError("sender lease has no attached name")
    text = args.send
    if not text.strip():
        raise ValueError("empty message")
    if len(text.encode("utf-8")) > 65536:
        raise ValueError("message exceeds 64 KiB")
    target = args.to
    path = root / AUDIENCE_BINDING_FILENAME
    with path.with_suffix(".lock").open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_SH)
        state = read_json(path) or {}
        bindings = (state.get("bindings")
                    if state.get("schema") == AUDIENCE_BINDING_SCHEMA else None)
        if not isinstance(bindings, dict) or not bindings:
            raise ValueError("no channel bindings exist")
        rows: list[tuple[str, dict[str, Any], str]] = []
        sender_channel = None
        for channel, binding in sorted(bindings.items()):
            if not isinstance(binding, dict):
                continue
            owner_lease = lease_identifier(str(binding.get("provider") or ""),
                                           str(binding.get("provider_session_id") or ""))
            if owner_lease == lease_id:
                sender_channel = str(channel)
            rows.append((str(channel), binding, owner_lease))
        if target == "0":
            selected = [(channel, binding) for channel, binding, owner_lease in rows
                        if owner_lease != lease_id]
            if not selected:
                raise ValueError("no other agent is bound to any channel")
            origin = "0"
        else:
            selected = [(channel, binding) for channel, binding, _ in rows
                        if str(binding.get("audience") or "").casefold() == target.casefold()]
            if not selected:
                raise ValueError(f"no channel is bound to the name {target!r}")
            selected = selected[:1]
            origin = selected[0][0]
        identity = os.urandom(12).hex()
        sender = {"name": sender_name, "provider": args.provider.casefold(),
                  "provider_session_id": args.session, "lease_id": lease_id,
                  "channel": sender_channel}
        deliveries: list[dict[str, Any]] = []
        for channel, binding in selected:
            owner_lease = lease_identifier(str(binding.get("provider") or ""),
                                           str(binding.get("provider_session_id") or ""))
            audience = binding.get("audience")
            if not isinstance(audience, str) or not audience:
                raise ValueError(f"channel {channel} has no audience")
            bus = Path(str(binding.get("bus") or "")).expanduser().resolve(strict=False)
            owner = {"provider": str(binding.get("provider") or "").casefold(),
                     "provider_session_id": binding.get("provider_session_id"),
                     "lease_id": owner_lease, "channel": channel,
                     "audience": audience, "name": audience, "bus": str(bus)}
            event = {"schema": AGENT_REPLY_SCHEMA, "kind": "agent_reply",
                     "emitted_at": utc_now(), "reply_id": identity,
                     # source_event_id keys the delivery phase (enrich), so two
                     # peer messages never collapse into one delivery identity.
                     "message_id": identity, "source_event_id": identity,
                     "name": sender_name,
                     "provider": args.provider.casefold(),
                     "provider_session_id": args.session, "lease_id": lease_id,
                     "text": text, "spoken": False, "association": "unsolicited",
                     "delivery_id": None, "peer_to": audience,
                     "channel": origin, "sender": sender, "recipients": [owner]}
            receipt = publish_reply_event(bus, event, bridge_root=root)
            if (any(type(receipt.get(key)) is not int or receipt[key] < 0
                    for key in ("stream_dev", "stream_inode", "offset", "length"))
                    or not 0 < receipt["length"] <= REPLY_READ_LIMIT
                    or not isinstance(receipt.get("stream_id"), str) or not receipt["stream_id"]):
                raise ValueError("publication has no durable receipt; do not automatically resend")
            deliveries.append({"peer": audience, "channel": channel,
                               "follower_live": bool(live_follower_pid(root, owner_lease)),
                               "source": receipt})
    emit({"kind": "peer_message_published", "message_id": identity,
          "origin_channel": origin, "sender": sender_name,
          "recipients": deliveries})
    return 0


# =============================================================================
# § 14. Channel bindings and session handover
# =============================================================================


def follower_pidfile(root: Path, lease_id: str) -> Path:
    return root / "runtime" / "followers" / f"{lease_id}.pid"


def live_follower_pid(root: Path, lease_id: str) -> int | None:
    """Live follower pid for a lease; heartbeat freshness never proves death.

    A manually started follower has no pidfile but still owns the lease lock;
    spawning next to it would only produce a child that loses the lock and
    dies. Its recorded live process counts even when its heartbeat is stale.
    """
    value = read_json(follower_pidfile(root, lease_id))
    pid = value.get("pid") if isinstance(value, dict) else None
    if isinstance(pid, int) and process_is_alive(pid):
        return pid
    state = read_json(root / "leases" / f"{lease_id}.json")
    if isinstance(state, dict):
        lease_pid = state.get("pid")
        if (
            isinstance(lease_pid, int)
            and process_is_alive(lease_pid)
        ):
            return lease_pid
    return None


@contextlib.contextmanager
def channel_bindings(root: Path) -> Iterator[dict[str, Any]]:
    """The binding file's only writer: one lock, one validation, one write.

    Yields the mutable bindings map under an exclusive lock and writes it back
    atomically when, and only when, the caller actually changed it. Every
    operation on the file — claim, handover, release — runs inside this block,
    so verification and rewrite never straddle a window another session can
    use.
    """
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = root / AUDIENCE_BINDING_FILENAME
    # Lock a stable sibling: atomic_json replaces the data file's inode.
    with open(path.with_suffix(".lock"), "a+b") as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        state = read_json(path)
        original = path.read_bytes() if path.exists() else None
        if path.exists() and (
            not isinstance(state, dict)
            or state.get("schema") != AUDIENCE_BINDING_SCHEMA
            or not isinstance(state.get("bindings"), dict)
        ):
            raise OSError("channel bindings are unreadable or invalid; nothing changed")
        bindings = state["bindings"] if state else {}
        before = json.dumps(bindings, sort_keys=True)
        yield bindings
        if json.dumps(bindings, sort_keys=True) != before:
            attempted = {"schema": AUDIENCE_BINDING_SCHEMA, "bindings": bindings}
            try:
                atomic_json(path, attempted)
            except OSError as error:
                # Replacement can succeed before a durability operation fails.
                # Restore only our exact attempted document, under the same lock.
                if read_json(path) == attempted:
                    try:
                        if original is None:
                            path.unlink()
                        else:
                            atomic_bytes(path, original)
                    except OSError:
                        pass
                restored = (
                    not path.exists() if original is None
                    else path.exists() and path.read_bytes() == original
                )
                raise OSError(
                    "binding write failed; "
                    + ("original binding restored" if restored else "binding state uncertain")
                ) from error


def occupied_refusal(
    channel: str, current: Any, bindings: dict[str, Any], same_name: bool = False
) -> str:
    """The one wording for a channel another session already owns."""
    owner = (
        current.get("audience", "unknown") if isinstance(current, dict) else "unknown"
    )
    free = ", ".join(str(slot) for slot in range(1, 10) if str(slot) not in bindings)
    message = (
        f"channel {channel} is occupied by {owner}; "
        f"free channels: {free or 'none'}; nothing changed"
    )
    if same_name:
        message += (
            "; --takeover claims it for the same name once that session has ended"
        )
    return message


def write_channel_binding(
    root: Path,
    channel: str,
    name: str,
    provider: str,
    provider_session_id: str,
    bus: str | None = None,
) -> Path:
    """Claim a channel without replacing another session's routing."""
    path = root / AUDIENCE_BINDING_FILENAME
    with channel_bindings(root) as bindings:
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
                raise OSError(
                    occupied_refusal(
                        channel,
                        current,
                        bindings,
                        same_name=isinstance(current, dict)
                        and current.get("audience") == requested["audience"],
                    )
                )
            if not bus or current.get("bus") == bus:
                return path
        bindings[str(channel)] = entry
    return path


def source_closes_retained_documents(bus: Path, documents: dict[str, Any]) -> bool:
    """Resolve stale observer snapshots through the canonical source normalizer."""
    remaining = set(documents)
    normalizer = EvidenceNormalizer()
    for raw in replay(bus):
        for event in normalized_revision_events(raw, normalizer):
            if event.get("status") == SEALED:
                remaining.discard(event.get("session_id"))
        if not remaining:
            return True
    return not remaining


def require_drained_lease(
    root: Path, lease_id: str, *, lenient_catch_up: bool = False
) -> None:
    """A saved cursor must cover the source before its reader can be retired.

    ``lenient_catch_up`` is only for a SAME-SESSION detach, where the lease
    (cursor, pending, unclosed documents) is inherited by the successor: a
    live reader gets a moment to catch up to the extent measured at entry,
    and unclosed documents never veto. A cross-session takeover keeps the
    strict contract - its new lease inherits nothing, so every row and every
    open document of the old owner must be settled first.
    """
    path = root / "leases" / f"{lease_id}.json"
    state = read_json(path)
    if (not isinstance(state, dict) or state.get("schema") != LEASE_SCHEMA
            or state.get("lease_id") != lease_id
            or not isinstance(state.get("provider"), str)
            or not isinstance(state.get("provider_session_id"), str)
            or lease_identifier(state["provider"], state["provider_session_id"]) != lease_id
            or type(state.get("cursor")) is not int
            or not isinstance(state.get("bus"), str)
            or not Path(state["bus"]).is_absolute()
            or not isinstance(state.get("pending"), list)
            or any(not isinstance(item, dict) for item in state["pending"])
            or not isinstance(state.get("unclosed_channel_messages", {}), dict)):
        raise OSError("unreadable retirement cursor; owner retained")
    try:
        end = generation_metadata(Path(state["bus"])).st_size
    except (OSError, ValueError, TypeError, KeyError) as error:
        raise OSError("source extent unavailable; owner retained") from error
    if state["cursor"] > end:
        # The saved cursor sits past the journal's logical extent: the bus
        # was replaced underneath its reader. Nothing of the old extent can
        # be drained any more, so replacement never blocks retirement;
        # pending envelopes stay in the lease.
        return
    if lenient_catch_up and state["cursor"] < end:
        # A live producer appends while retirement is being decided, so
        # exact cursor == end never holds on an active channel (observed
        # live 2026-10-07: the app's evidence stream made detach refuse
        # forever). Rows are only lost if the READER is behind: give a
        # live follower a moment to catch up to the extent measured at
        # entry; rows appended after that snapshot are inherited through
        # the lease cursor by the same-session successor.
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            time.sleep(0.15)
            state = read_json(path)
            if not isinstance(state, dict) or type(state.get("cursor")) is not int:
                break
            if state["cursor"] >= end:
                break
    if (not isinstance(state, dict) or type(state.get("cursor")) is not int
            or state["cursor"] < end
            or (not lenient_catch_up and (
                state["cursor"] != end))):
        raise OSError("old source is undrained; resume its reader before retiring it")
    documents = state.get("unclosed_channel_messages", {})
    if not lenient_catch_up and documents and not source_closes_retained_documents(Path(state["bus"]), documents):
        raise OSError("old source has an unfinished capture; owner retained")


def verified_follower(root: Path, lease_id: str, session: str, pid: int) -> bool:
    """Bind a live invocation to the incarnation recorded by the lease owner."""
    import shlex

    state = read_json(root / "leases" / f"{lease_id}.json") or {}
    observed = process_identity(pid)
    caller = process_identity(os.getpid())
    if (observed is None or caller is None or state.get("process_identity") != observed
            or state.get("schema") != LEASE_SCHEMA or state.get("lease_id") != lease_id
            or state.get("pid") != pid or state.get("provider_session_id") != session
            or not isinstance(state.get("provider"), str)
            or lease_identifier(state["provider"], session) != lease_id):
        return False
    try:
        words = shlex.split(observed["command"])
        caller_words = shlex.split(caller["command"])
        def argument(flag: str) -> str | None:
            positions = [i for i, word in enumerate(words) if word == flag]
            if len(positions) != 1 or positions[0] + 1 >= len(words):
                return None
            return words[positions[0] + 1]
        return (
            # macOS Python can execute a framework binary behind its launcher.
            # Compare the actual running interpreters, not the launcher path.
            len(words) >= 2 and bool(caller_words)
            and Path(words[0]).resolve() == Path(caller_words[0]).resolve()
            and Path(words[1]).resolve() == Path(__file__).resolve()
            and "--follow" in words and argument("--session") == session
            and argument("--provider") == state["provider"]
            and argument("--bridge-home") is not None
            and Path(argument("--bridge-home")).resolve() == root.resolve()
            and argument("--bus") is not None
            and Path(argument("--bus")).resolve() == Path(state["bus"]).resolve()
        )
    except (ValueError, OSError, TypeError, KeyError):
        return False


@contextlib.contextmanager
def retirement_guard(
    root: Path, lease_id: str, session: str, *, bound: bool = True,
    lenient_catch_up: bool = False,
) -> Iterator[tuple[int | None, str]]:
    """Keep the existing lease lock through a verified, drained handover."""
    import signal

    path = root / "leases" / f"{lease_id}.lock"
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    held = False
    pid = live_follower_pid(root, lease_id)
    stopped = False
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            held = True
        except BlockingIOError:
            pass
        if pid is not None:
            if held or not verified_follower(root, lease_id, session, pid):
                yield pid, "unverified_retained"
                return
            try:
                require_drained_lease(root, lease_id, lenient_catch_up=lenient_catch_up)
            except OSError:
                yield pid, "undrained_retained"
                return
            # Observe the recorded incarnation again immediately before signaling.
            if not verified_follower(root, lease_id, session, pid):
                yield pid, "unverified_retained"
                return
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                if not process_is_alive(pid):
                    try:
                        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        held = True
                        stopped = True
                        break
                    except BlockingIOError:
                        pass
                time.sleep(0.1)
            if not held:
                yield pid, "did_not_exit"
                return
        elif not held:
            yield None, "unverified_retained"
            return
        elif not bound and not (root / "leases" / f"{lease_id}.json").exists():
            yield None, "not_running"
            return
        try:
            require_drained_lease(root, lease_id, lenient_catch_up=lenient_catch_up)
        except OSError:
            yield pid, "undrained_stopped" if stopped else "undrained_retained"
            return
        yield pid, "stopped" if stopped else "not_running"
    finally:
        if held:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        os.close(descriptor)


def lease_backlog(
    root: Path, lease_id: str
) -> tuple[list[Any], set[str], list[dict[str, Any]]]:
    """Pending envelopes, acknowledgment markers and the unacknowledged rest.

    The lease file keeps acknowledged envelopes until the follower's next
    sweep, so its raw pending length overstates the backlog; the marker store
    is the receipt truth and the backlog is pending minus markers.
    """
    state = read_json(root / "leases" / f"{lease_id}.json")
    pending = state.get("pending") if isinstance(state, dict) else None
    if not isinstance(pending, list):
        pending = []
    try:
        markers = {
            entry[: -len(".json")]
            for entry in os.listdir(root / "acknowledgments" / lease_id)
            if entry.endswith(".json")
            and re.fullmatch(r"[0-9a-f]{24}", entry[:-len(".json")])
            and delivery_acknowledged(root, lease_id, entry[:-len(".json")])
        }
    except OSError:
        markers = set()
    unacked = [
        item
        for item in pending
        if isinstance(item, dict) and item.get("delivery_id") not in markers
    ]
    return pending, markers, unacked


def retired_owner_report(root: Path, entry: dict[str, Any]) -> dict[str, Any]:
    """Identity and backlog of the session a channel entry still points at.

    The backlog named here is the one ``--status`` calls ``unacked_seals``:
    sealed takes and typed messages nobody acknowledged. A draft revision is
    not a delivery waiting for a reader, so it is never named to the Founder.
    """
    provider = str(entry.get("provider") or "")
    session = str(entry.get("provider_session_id") or "")
    lease_id = lease_identifier(provider, session)
    unacked = [
        str(item.get("delivery_id"))
        for item in lease_backlog(root, lease_id)[2]
        if item.get("kind") in TERMINAL_KINDS
        and isinstance(item.get("delivery_id"), str)
    ]
    return {
        "provider": provider,
        "provider_session_id": session,
        "lease_id": lease_id,
        "audience": entry.get("audience"),
        "follower_pid": None,
        # Keep uncertainty explicit if inspection itself fails; never report
        # a reader as stopped merely because retirement was requested.
        "follower_state": "unknown",
        "unacked_deliveries": len(unacked),
        "unacked_delivery_ids": unacked,
    }


def detach_command(args: argparse.Namespace) -> int:
    """Release a drained session under its binding and lease ownership locks."""
    root: Path = args.bridge_home
    lease_id = lease_identifier(args.provider, args.session)
    released: list[str] = []
    original_entries: dict[str, Any] = {}
    pid: int | None = None
    follower_state = "unknown"
    failure: str | None = None
    binding_changed: bool | None = False
    try:
        with contextlib.ExitStack() as retirements:
            with channel_bindings(root) as bindings:
                original_entries = {
                    slot: entry for slot, entry in bindings.items()
                    if isinstance(entry, dict)
                    and entry.get("provider") == args.provider.casefold()
                    and entry.get("provider_session_id") == args.session
                }
                pid, follower_state = retirements.enter_context(
                    retirement_guard(root, lease_id, args.session, bound=bool(original_entries), lenient_catch_up=True)
                )
                if follower_state in HANDOVER_CLEAR_STATES:
                    if original_entries or (root / "leases" / f"{lease_id}.json").exists():
                        require_drained_lease(root, lease_id, lenient_catch_up=True)
                    released = sorted(original_entries)
                    for slot in released:
                        del bindings[slot]
            binding_changed = bool(released)
    except OSError as error:
        if follower_state == "unknown":
            raise
        failure = str(error)
        current = read_json(root / AUDIENCE_BINDING_FILENAME)
        current_bindings = current.get("bindings") if isinstance(current, dict) else None
        unchanged = (isinstance(current_bindings, dict)
                     and all(current_bindings.get(slot) == entry
                             for slot, entry in original_entries.items()))
        binding_changed = False if unchanged else None
        released = []
    unacked = [item for item in lease_backlog(root, lease_id)[2]
               if item.get("kind") in TERMINAL_KINDS]
    emit({
        "schema": DETACH_RECEIPT_SCHEMA, "kind": "detach_receipt",
        "provider": args.provider.casefold(), "provider_session_id": args.session,
        "lease_id": lease_id, "released_channels": released,
        "follower_pid": pid, "follower_state": follower_state,
        "binding_changed": binding_changed,
        "binding_state": "uncertain" if binding_changed is None else "changed" if binding_changed else "unchanged",
        "was_attached": bool(original_entries) or follower_state != "not_running",
        "unacked_deliveries": len(unacked),
        "unacked_delivery_ids": [str(item.get("delivery_id")) for item in unacked
                                 if isinstance(item.get("delivery_id"), str)],
        "binding_path": str(root / AUDIENCE_BINDING_FILENAME),
    })
    if failure is not None or follower_state not in HANDOVER_CLEAR_STATES:
        reason = failure or f"follower {follower_state}; channel ownership retained"
        sys.stderr.write(f"bus-demux: detach failed: {reason}\n")
        return 3
    return 0


def read_inherited_delivery(args: argparse.Namespace) -> int:
    """Read one envelope from a handed-over lease without touching it.

    Authorization is current ownership: the caller's session holds a channel
    bound to the same spoken name as the named lease. Nothing is
    acknowledged, no cursor moves, and the retired lease stays exactly as its
    own session left it.
    """
    root: Path = args.bridge_home
    state = read_json(root / "leases" / f"{args.lease}.json") or {}
    name = state.get("name")
    if (
        state.get("schema") != LEASE_SCHEMA
        or state.get("lease_id") != args.lease
        or not isinstance(name, str)
        or not name
    ):
        raise ValueError("named lease is unreadable or carries no name")
    bindings = (read_json(root / AUDIENCE_BINDING_FILENAME) or {}).get("bindings")
    if not isinstance(bindings, dict) or not any(
        isinstance(entry, dict)
        and entry.get("provider") == args.provider.casefold()
        and entry.get("provider_session_id") == args.session
        and entry.get("audience") == name.casefold()
        for entry in bindings.values()
    ):
        raise ValueError(
            f"this session owns no channel bound to {name.casefold()}; "
            "the inherited mailbox stays closed"
        )
    payload = next(
        (
            item
            for item in state.get("pending", [])
            if isinstance(item, dict) and item.get("delivery_id") == args.read_delivery
        ),
        None,
    )
    if payload is None:
        raise ValueError("delivery is not pending in the inherited mailbox")
    emit(
        {
            **payload,
            "inherited_from": {
                "provider": state.get("provider"),
                "provider_session_id": state.get("provider_session_id"),
                "lease_id": args.lease,
                "name": name,
                "acknowledgment": "not_available",
            },
        }
    )
    return 0


def attach_command(args: argparse.Namespace) -> int:
    """Publish a channel owner only after its verified reader is ready."""
    import subprocess

    root: Path = args.bridge_home
    name = args.name.casefold()
    persist_voice = bool(args.voice or args.speed is not None or args.tts_vendor)
    voice_source = voice_profile_with_source(root, name)[1]
    if persist_voice:
        _voice_store(root / VOICES_FILENAME)
    if args.voice:
        voice_source = "flag"
    if not getattr(args, "bus_overridden", False):
        channel_bus = root / "buses" / f"channel-{args.channel}.jsonl"
        channel_bus.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor = os.open(channel_bus, os.O_WRONLY | os.O_CREAT, 0o600)
        os.close(descriptor)
        args.bus = channel_bus
    resolved_bus = str(Path(args.bus).expanduser().resolve(strict=False))
    binding_path = root / AUDIENCE_BINDING_FILENAME
    requested = {"audience": name, "provider": args.provider.casefold(),
                 "provider_session_id": args.session}
    claimed = {**requested, "bus": resolved_bus}
    previous: dict[str, Any] | None = None
    original_entry: Any = None
    child: Any = None
    child_state = "not_started"
    lease_id = lease_identifier(args.provider, args.session)
    lease_path = root / "leases" / f"{lease_id}.json"
    resumed = lease_path.exists()
    configuration = wakeup_configuration(args)
    log_path, events_path = follower_paths(root, lease_id)
    errors_path = log_path.with_suffix(".errors.log")
    try:
        # Old lease locks outlive the binding commit; the child owns its own
        # distinct lease. No competing attach can observe a half-started owner.
        with contextlib.ExitStack() as retirements:
            with channel_bindings(root) as bindings:
                original_entry = bindings.get(str(args.channel))
                if str(args.channel) in bindings and (
                    not isinstance(original_entry, dict)
                    or any(original_entry.get(key) != value for key, value in requested.items())
                ):
                    same_name = (isinstance(original_entry, dict)
                                 and original_entry.get("audience") == name)
                    if not getattr(args, "takeover", False) or not same_name:
                        raise OSError(occupied_refusal(args.channel, original_entry, bindings, same_name))
                    previous = retired_owner_report(root, original_entry)
                    old_pid, disposition = retirements.enter_context(retirement_guard(
                        root, previous["lease_id"], previous["provider_session_id"]
                    ))
                    previous = {**retired_owner_report(root, original_entry),
                                "follower_pid": old_pid, "follower_state": disposition}
                    if disposition not in HANDOVER_CLEAR_STATES:
                        raise OSError(f"previous follower {disposition}; owner retained")

                pid = live_follower_pid(root, lease_id)
                lease_state = read_json(lease_path)
                pid_record = read_json(follower_pidfile(root, lease_id)) or {}
                installed = pid_record.get("configuration") or (lease_state or {}).get("wakeup_configuration")
                migrating = (isinstance(lease_state, dict)
                             and lease_state.get("bus") not in (None, resolved_bus))
                if pid is not None and not verified_follower(root, lease_id, args.session, pid):
                    raise OSError("existing follower identity could not be verified; retained")
                if migrating or pid is not None and installed != configuration:
                    with retirement_guard(root, lease_id, args.session) as (_, disposition):
                        if disposition not in HANDOVER_CLEAR_STATES:
                            raise OSError(f"existing follower {disposition}; retained")
                        if migrating:
                            lease_state = read_json(lease_path)
                            if lease_state is None:
                                raise OSError("lease became unreadable; retained")
                            lease_state.update(bus=resolved_bus, cursor=0, active=False)
                            atomic_json(lease_path, lease_state)
                    pid = None
                if pid is None:
                    command = [sys.executable, os.path.abspath(__file__),
                               "--bus", resolved_bus, "--bridge-home", str(root.resolve()),
                               "--provider", args.provider, "--session", args.session,
                               "--name", name, "--drafts", "--follow", "--coalesce",
                               "--follower-events", str(events_path),
                               "--follower-channel", str(args.channel),
                               "--wakeup", configuration["wakeup"]]
                    if args.on_seal:
                        command += ["--on-seal", args.on_seal]
                    log_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                    with contextlib.ExitStack() as outputs:
                        for path in (log_path, errors_path):
                            descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
                            handle = outputs.enter_context(os.fdopen(descriptor, "ab"))
                            os.fchmod(handle.fileno(), 0o600)
                            if path == log_path:
                                log = handle
                            else:
                                errors = handle
                        child = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                                 stdout=log, stderr=errors, start_new_session=True)
                    pid = child.pid
                    child_state = "starting"
                    atomic_json(follower_pidfile(root, lease_id), {
                        "lease_id": lease_id, "pid": pid, "started_at": utc_now(),
                        "configuration": configuration,
                    })
                    deadline = time.monotonic() + 5.0
                    while time.monotonic() < deadline:
                        if child.poll() is not None:
                            child_state = "exited"
                            raise OSError(f"follower exited during startup; see {errors_path}")
                        state = read_json(lease_path) or {}
                        if (state.get("schema") == LEASE_SCHEMA and state.get("pid") == pid
                                and state.get("lease_id") == lease_id
                                and state.get("active") is True and state.get("bus") == resolved_bus
                                and events_path.exists()
                                and verified_follower(root, lease_id, args.session, pid)
                                and child.poll() is None):
                            child_state = "ready"
                            break
                        time.sleep(0.1)
                    else:
                        raise OSError(f"follower readiness timed out; see {errors_path}")
                if persist_voice:
                    write_voice_profile(root, name, voice=args.voice, speed=args.speed, vendor=args.tts_vendor)
                if previous is not None:
                    # A producer may have completed another old-owner row while
                    # its reader was exiting. Never hide that row at cutover.
                    require_drained_lease(root, previous["lease_id"])
                bindings[str(args.channel)] = claimed
            # channel_bindings commits here, while retirement locks still hold.
    except (OSError, ValueError, RuntimeError) as error:
        if child is not None:
            try:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=5)
                child_state = "stopped"
            except (OSError, subprocess.TimeoutExpired):
                child_state = "did_not_exit" if child.poll() is None else "exited"
        actual = read_json(binding_path)
        actual_entry = (actual.get("bindings", {}).get(str(args.channel))
                        if isinstance(actual, dict) and isinstance(actual.get("bindings"), dict)
                        else None)
        unchanged = ((actual is not None or not binding_path.exists())
                     and actual_entry == original_entry)
        if previous is not None:
            emit({"schema": TAKEOVER_RECEIPT_SCHEMA, "kind": "takeover_receipt",
                  "channel": str(args.channel), "audience": name, "provider": args.provider.casefold(),
                  "provider_session_id": args.session,
                  "binding_changed": False if unchanged else None,
                  "binding_state": "unchanged" if unchanged else "uncertain",
                  "previous": previous, "new_follower_state": child_state,
                  "binding_path": str(binding_path)})
        suffix = "unchanged" if unchanged else "state uncertain"
        sys.stderr.write(f"bus-demux: attach failed: {error}; channel {args.channel} {suffix}\n")
        return 3
    state = read_json(lease_path) or {}
    emit({
        "schema": ATTACH_RECEIPT_SCHEMA, "kind": "attach_receipt",
        "channel": str(args.channel), "audience": name, "provider": args.provider.casefold(),
        "provider_session_id": args.session, "lease_id": lease_id,
        "cursor": state.get("cursor"), "resumed": resumed, "follower_pid": pid,
        "follower_spawned": child is not None, "follower_log": str(log_path),
        "follower_events": str(events_path), "coalesce_requested": True,
        "on_seal_hook": bool(args.on_seal), "wakeup": configuration["wakeup"],
        "wakeup_receipts": str(root / "wakeups" / lease_id),
        "voice": voice_profile(root, name), "voice_source": voice_source,
        "voices_file": "present" if (root / VOICES_FILENAME).exists() else "missing",
        "binding_path": str(binding_path), "binding_changed": previous is not None,
        "previous": previous,
    })
    return 0


# =============================================================================
# § 15. Status, watch and the CLI entrypoint
# =============================================================================


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
    pending, markers, unacked = lease_backlog(root, lease_id)
    seals = [item for item in unacked if item.get("kind") in TERMINAL_KINDS]
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
            "wakeup": (state or {}).get("wakeup_configuration", {}).get("wakeup", "unrecorded"),
            "wakeup_receipts": str(root / "wakeups" / lease_id),
            "pending_wakeups": [
                {"delivery_id": p["delivery_id"], "disposition": (
                    read_json(root / "wakeups" / lease_id / f"{p['delivery_id']}.json") or {}
                ).get("disposition", "not_submitted")}
                for p in seals
            ],
            "lease_id": lease_id,
            "follower_log": str(log_path),
            "follower_events": str(events_path),
            "attached": state is not None,
            "channel": channel,
            "name": name,
            "follower_alive": follower_alive,
            "follower_pid": lease_pid if follower_alive else None,
            "pending_native_withdrawals": [
                {"delivery_id": item["delivery_id"],
                 "queue_disposition": (read_json(root / "wakeups" / lease_id / f"{item['delivery_id']}.json") or {}).get("queue_disposition", "pending")}
                for item in pending if isinstance(item, dict) and item.get("delivery_id") in markers
                and (read_json(root / "wakeups" / lease_id / f"{item['delivery_id']}.json") or {}).get("disposition") in ("provider_accepted", "requesting", "uncertain")
                and (read_json(root / "wakeups" / lease_id / f"{item['delivery_id']}.json") or {}).get("queue_disposition") not in ("removed", "not_pending")
            ],
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
                key = draft_key(payload)
                if payload.get("kind") in DRAFT_KINDS:
                    drafts[key] = payload
                    continue
                flush(key)
                print(human_line(payload, channel), flush=True)
            else:
                emit(line if getattr(args, "full", False) else {
                    "delivery_id": identity, "kind": line.get("kind"),
                    "notice": "Codescribe mailbox has a new delivery",
                })
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
    parser = argparse.ArgumentParser(
        prog="cs-bus", usage="%(prog)s <operation> [options]",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="Codescribe messages, live notifications and voice replies for your agent.\n\n"
               "Quick start:\n"
               "  cs-bus --attach --channel 2 --name lena --provider codex --session THREAD\n"
               "  cs-bus --attach --channel 2 --name lena --provider codex --session THREAD --takeover\n"
               "  cs-bus --detach --provider codex --session THREAD\n"
               "  cs-bus --read-delivery ID --lease PREVIOUS_LEASE --provider codex --session THREAD\n"
               "  cs-bus --watch --provider codex --session THREAD\n"
               "  cs-bus --read-delivery ID --provider codex --session THREAD\n"
               "  cs-bus --ack ID --provider codex --session THREAD\n"
               "  cs-say 'Gotowe.' --provider codex --session THREAD\n"
               "  cs-say auth --help\n\n"
               "Every attachment needs an output-notifying watch. Its default is a short bell;\n"
               "read the full envelope before ACK. Codex native queue also wakes the next turn.",
    )
    parser.add_argument("--version", action="store_true", help="installed helper version and source commit slug")
    parser.add_argument("--entrypoint", choices=("cs-bus", "cs-say"), default="cs-bus", help=argparse.SUPPRESS)
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
        "--wakeup", choices=("auto", "codex-queue", "off"), default="auto",
        help="native wakeup: auto uses codex queue for attached Codex sessions; off keeps monitor-only delivery",
    )
    parser.add_argument(
        "--retry-wakeup", metavar="DELIVERY_ID",
        help="explicitly retry one retained Codex seal after a failed/uncertain queue submission",
    )
    parser.add_argument("--read-delivery", metavar="DELIVERY_ID", help="read one complete pending envelope without acknowledging or resubmitting it")
    parser.add_argument("--read-pending", action="store_true", help="read complete unread non-draft envelopes as a bounded batch; never ACK automatically")
    parser.add_argument("--read-limit", type=int, default=None, help="with --read-pending: maximum envelopes, default 8 (1..256)")
    parser.add_argument("--read-bytes", type=int, default=None, help="with --read-pending: maximum UTF-8 output bytes, default 65536 (up to 16 MiB)")
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
        "--takeover",
        action="store_true",
        help="with --attach: claim the channel from an ended session of the "
        "same name, stopping its leftover follower first",
    )
    parser.add_argument(
        "--detach",
        action="store_true",
        help="release this session's channels, stop its own follower, print a "
        "detach receipt, and exit; the lease and its backlog stay",
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
        help="live notifications for this mailbox; short bell by default "
        "(--once reads existing events and exits)",
    )
    watch_format = parser.add_mutually_exclusive_group()
    watch_format.add_argument("--human", action="store_true", help="diagnostic --watch as readable one-line envelopes")
    watch_format.add_argument("--bell", action="store_true", help="explicit default --watch format: delivery id and notice only")
    watch_format.add_argument("--full", action="store_true", help="diagnostic --watch with transcript text and receipt fields")
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
    parser.add_argument("--send-text", action="store_true", help="send user text from stdin to an exact channel owner; --channel, --lease and --bus required")
    parser.add_argument("--send", metavar="TEXT", help="agent-authored text message; requires --to and an attached --provider/--session sender")
    parser.add_argument("--to", metavar="NAME|0", help="recipient agent name for --send, or 0 to broadcast to every other bound agent")
    parser.add_argument("--reply-to", metavar="DELIVERY_ID", help="associate --say with this owned delivery envelope")
    playback = parser.add_mutually_exclusive_group()
    playback.add_argument("--play-reply", metavar="REPLY_ID", help="explicitly play one durable reply")
    playback.add_argument("--stop-reply", metavar="REPLY_ID", help="request stop from that reply's real player")
    mute = parser.add_mutually_exclusive_group()
    mute.add_argument("--mute-agent", action="store_true", help="mute automatic replies for this provider session and bus")
    mute.add_argument("--unmute-agent", action="store_true", help="unmute future automatic replies for this provider session and bus")
    parser.add_argument("--playback-ticket", metavar="TICKET", help="24 lowercase hex identifying one playback request")
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
    if (args.read_limit is not None or args.read_bytes is not None) and not args.read_pending:
        parser.error("--read-limit and --read-bytes require --read-pending")
    if args.read_pending and any((
        args.version, args.print_bus_path, args.print_install_interlock_path,
        args.print_agent_turn_lease_path, args.assert_install_idle,
        args.ack, args.attach, args.detach, args.takeover, args.channel is not None,
        args.status, args.watch, args.follow, args.once, args.from_start, args.from_file,
        args.read_delivery, args.retry_wakeup, args.say is not None, args.send_text,
        args.send is not None, args.to is not None, args.lease,
        args.play_reply, args.stop_reply, args.playback_ticket, args.reply_to,
        args.all, args.become, args.active_names, args.drafts, args.coalesce,
        args.mute_agent, args.unmute_agent, args.on_seal, args.voice,
        args.speed is not None, args.tts_vendor,
    )):
        parser.error("--read-pending combines with no other command")
    if (args.mute_agent or args.unmute_agent) and any((
        args.version, args.print_bus_path, args.print_install_interlock_path,
        args.print_agent_turn_lease_path, args.assert_install_idle,
    )):
        parser.error("agent mute combines with no inspection command")
    # Refuse contradictory commands before any early publication/playback or
    # inspection dispatch can return without reaching the detach branch.
    if args.detach and any((
        args.attach, args.takeover, args.channel is not None, args.status, args.watch,
        args.follow, args.once, args.from_start, args.ack, args.lease,
        args.from_file is not None, args.say is not None, args.send_text,
        args.send is not None, args.to is not None,
        args.read_delivery, args.retry_wakeup, args.play_reply, args.stop_reply,
        args.playback_ticket, args.reply_to, args.all, args.become, args.active_names,
        args.mute_agent, args.unmute_agent,
        args.version, args.print_bus_path, args.print_install_interlock_path,
        args.print_agent_turn_lease_path, args.assert_install_idle,
    )):
        parser.error("--detach combines with no other command")
    if args.version:
        manifest = {}
        # Direct installs carry their generation with the executable, independent
        # of any older app replacing the runtime receipt or payload directory.
        with Path(__file__).open(encoding="utf-8") as helper:
            helper.readline()
            generation = helper.readline().rstrip("\n")
        marker = "# codescribe-managed-command: "
        if generation.startswith(marker):
            try:
                value = json.loads(generation[len(marker):])
                if isinstance(value, dict):
                    manifest = value
            except json.JSONDecodeError:
                pass
        if not manifest:
            manifest = read_json(Path(__file__).resolve().parent.parent / "manifest.json") or {}
        version = manifest.get("helper_version") or manifest.get("bundle_version") or "source"
        commit = manifest.get("source_commit")
        slug = f"+g{commit[:8]}" if isinstance(commit, str) and re.fullmatch(r"[0-9a-f]{40,64}", commit) else ""
        dirty = ".dirty" if manifest.get("source_dirty") is True else ""
        print(f"{args.entrypoint} {version}{slug}{dirty}")
        return 0
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
        return 0 if installation_idle_with_checkpoint(args.bus, bridge_root=args.bridge_home) else 2
    if args.provider and not args.session:
        args.session = provider_session_from_env(args.provider)
    if bool(args.provider) != bool(args.session):
        parser.error(
            "--provider and --session must be supplied together "
            "(only claude-code falls back to $CLAUDE_CODE_SESSION_ID)"
        )
    if args.lease and not args.provider:
        parser.error("--lease requires --provider and --session")
    if args.read_pending:
        if not args.provider:
            parser.error("--read-pending requires --provider/--session")
        args.read_limit = 8 if args.read_limit is None else args.read_limit
        args.read_bytes = 65536 if args.read_bytes is None else args.read_bytes
        try:
            return read_pending_command(args)
        except (OSError, ValueError) as error:
            sys.stderr.write(f"cs-bus: pending read refused: {error}\n")
            return 3
    if args.takeover and not args.attach:
        parser.error("--takeover requires --attach")
    if args.mute_agent or args.unmute_agent:
        if (not args.provider or not args.bus_overridden
                or any((args.send_text, args.say is not None, args.ack, args.attach,
                        args.detach, args.takeover, args.channel is not None, args.lease,
                        args.status, args.watch, args.follow, args.once, args.from_start,
                        args.from_file, args.read_delivery, args.retry_wakeup,
                        args.play_reply, args.stop_reply, args.playback_ticket, args.reply_to,
                        args.all, args.become, args.active_names, args.voice,
                        args.speed is not None, args.tts_vendor))):
            parser.error("agent mute requires only --provider/--session/--bus/--bridge-home")
        try:
            return set_agent_muted(args)
        except (OSError, ValueError) as error:
            sys.stderr.write(f"cs-bus: playback mute refused: {error}\n")
            return 3
    if args.send_text:
        if (not args.provider or args.channel not in tuple(str(n) for n in range(1, 10))
                or not args.lease or not args.bus_overridden
                or any((args.say is not None, args.send is not None, args.to is not None,
                        args.ack, args.attach, args.status, args.watch,
                        args.follow, args.once, args.read_delivery, args.retry_wakeup,
                        args.play_reply, args.stop_reply, args.reply_to))):
            parser.error("--send-text requires an exact --provider/--session/--lease/--channel/--bus owner")
        try:
            return send_text_command(args)
        except (OSError, ValueError, RuntimeError) as error:
            sys.stderr.write(f"cs-bus: message publication refused: {error}\n")
            return 3
    if args.send is not None or args.to is not None:
        if (args.send is None or args.to is None or not args.provider
                or any((args.say is not None, args.ack, args.attach, args.status, args.watch,
                        args.follow, args.once, args.read_delivery, args.retry_wakeup,
                        args.play_reply, args.stop_reply, args.reply_to, args.lease))):
            parser.error("--send requires --to <name|0> and an attached --provider/--session sender")
        try:
            return send_peer_command(args)
        except (OSError, ValueError, RuntimeError) as error:
            sys.stderr.write(f"cs-bus: peer message refused: {error}\n")
            return 3
    if args.reply_to and (args.say is None or not re.fullmatch(r"[0-9a-f]{24}", args.reply_to)):
        parser.error("--reply-to requires --say and a delivery id")
    if args.play_reply or args.stop_reply:
        identity = args.play_reply or args.stop_reply
        if (not args.provider or not re.fullmatch(r"[0-9a-f]{24}", identity)
                or not args.playback_ticket or not re.fullmatch(r"[0-9a-f]{24}", args.playback_ticket)):
            parser.error("reply control requires --provider/--session, a reply id and a playback ticket")
        if any((args.say is not None, args.ack, args.attach, args.channel is not None,
                args.status, args.watch, args.follow, args.once, args.from_start, args.from_file,
                args.read_delivery, args.retry_wakeup, args.all, args.become, args.active_names,
                args.lease, args.voice, args.speed is not None, args.tts_vendor)):
            parser.error("reply control combines with no other command")
        try:
            return play_reply_command(args) if args.play_reply else stop_reply_command(args)
        except (OSError, ValueError, RuntimeError) as error:
            sys.stderr.write(f"bus-demux: reply control refused: {error}\n")
            return 3
    if args.playback_ticket:
        parser.error("--playback-ticket requires --play-reply or --stop-reply")
    if args.read_delivery:
        if not args.provider or not re.fullmatch(r"[0-9a-f]{24}", args.read_delivery):
            parser.error("--read-delivery requires --provider/--session and a delivery id")
        if any((args.ack, args.attach, args.detach, args.status, args.watch, args.follow, args.once, args.retry_wakeup, args.say is not None)):
            parser.error("--read-delivery combines with no other command")
        if args.lease and args.lease != lease_identifier(args.provider, args.session):
            # An inherited mailbox is read, never consumed: the retired lease
            # keeps its envelopes, markers and cursor.
            if not re.fullmatch(r"[0-9a-f]{32}", args.lease):
                parser.error("--lease takes a lease id")
            try:
                return read_inherited_delivery(args)
            except (OSError, ValueError) as error:
                sys.stderr.write(f"bus-demux: inherited read refused: {error}\n")
                return 3
        lease_id = lease_identifier(args.provider, args.session)
        state = read_json(args.bridge_home / "leases" / f"{lease_id}.json") or {}
        if state.get("lease_id") != lease_id or state.get("provider") != args.provider or state.get("provider_session_id") != args.session:
            parser.error("mailbox does not belong to this provider session")
        payload = next((p for p in state.get("pending", []) if p.get("delivery_id") == args.read_delivery), None)
        if payload is None or delivery_acknowledged(args.bridge_home, lease_id, args.read_delivery, payload):
            parser.error("delivery is not pending in this mailbox")
        emit(payload)
        return 0
    if args.wakeup == "codex-queue" and (args.provider != "codex" or args.on_seal):
        parser.error("codex-queue requires --provider codex and no separate --on-seal hook")
    if args.retry_wakeup:
        if args.provider != "codex" or not re.fullmatch(r"[0-9a-f]{24}", args.retry_wakeup):
            parser.error("--retry-wakeup requires --provider codex, --session and a delivery id")
        if any((args.ack, args.attach, args.detach, args.status, args.watch, args.follow, args.once, args.say is not None)):
            parser.error("--retry-wakeup combines with no other command")
        lease_id = lease_identifier(args.provider, args.session)
        state = read_json(args.bridge_home / "leases" / f"{lease_id}.json") or {}
        payload = next((p for p in state.get("pending", []) if p.get("delivery_id") == args.retry_wakeup), None)
        if payload is None or payload.get("kind") not in TERMINAL_KINDS:
            parser.error("delivery is not a pending seal in this mailbox")
        native = NativeQueueWakeup(args.bridge_home, args.session, None)
        native.enqueue(payload, retry=True)
        native.close(wait=True)
        receipt = read_json(args.bridge_home / "wakeups" / lease_id / f"{args.retry_wakeup}.json") or {}
        emit(receipt)
        return 0 if receipt.get("disposition") == "provider_accepted" else 3
    if args.speed is not None and args.speed <= 0:
        parser.error("--speed must be positive")
    if args.voice is not None and not args.voice.strip():
        parser.error("--voice needs a voice id")
    if args.human and not args.watch:
        parser.error("--human travels with --watch")
    if args.full and not args.watch:
        parser.error("--full requires --watch")
    if args.bell and (not args.watch or args.human):
        parser.error("--bell requires --watch and no --human")
    if args.detach:
        if not args.provider:
            parser.error("--detach requires --provider/--session")
        try:
            return detach_command(args)
        except OSError as error:
            sys.stderr.write(f"bus-demux: detach refused: {error}\n")
            return 3
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
        except (OSError, ValueError, RuntimeError) as error:
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
