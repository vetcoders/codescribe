#!/usr/bin/env -S python3 -E
"""Native MCP transport over the existing Codescribe mailbox, not an agent runner."""
from concurrent.futures import ThreadPoolExecutor
import contextlib
import datetime
import fcntl
import json
import os
from pathlib import Path
import queue
import shlex
import subprocess
import sys
import threading
import time
from types import SimpleNamespace

#: Hosts that load the Codescribe plugin, each with a wait window below its
#: client's tool-call deadline. Junie 26.10.5 aborts any tool call after
#: 180 s. Claude Code 2.1.296 aborts a stdio tool call that sends nothing for
#: 30 min, and any call after 1e8 ms. OMP 18.10.3 applies no client-side MCP
#: deadline unless `timeout`/`OMP_MCP_TIMEOUT_MS` is set; its window matches
#: Claude Code's because the hook, not the wait, is the idle wakeup there too.
#: A window that ends with an explicit idle receipt keeps server and client
#: agreeing on which wait is pending.
WAIT_SECONDS = {"claude-code": 900, "junie": 150, "omp": 900}

#: Hosts whose idle wakeup is a hook process holding the lease's wake lock:
#: Claude Code through its asyncRewake Stop hook, OMP through the bundled
#: extension running `cs-bus --omp-wake`. Junie has no idle wakeup.
NATIVE_WAKE_PROVIDERS = frozenset({"claude-code", "omp"})

#: Claude Code's asyncRewake hook wakes the model on exit code 2, which is
#: also argparse's usage-error code. The wake therefore exits WAKE_EXIT and the
#: plugin hook maps only that value to 2: a helper that cannot parse the hook
#: command stays a logged hook error instead of waking the model every turn.
WAKE_EXIT = 75
#: An idle wake re-arms the hook below its configured timeout (21660 s).
WAKE_SECONDS = 6 * 3600
#: How long the PostToolUse probe waits for the ledger lock before staying silent.
PROBE_LEDGER_WAIT_SECONDS = 2.0


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
    {"name": "codescribe_connect", "description": "Bind THIS conversation to a named Codescribe channel. Pass exactly this conversation's provider and real session_id from the Claude Code session hook or Junie's native /link. Never take over another conversation.",
     "inputSchema": schema({"provider": {"type": "string", "enum": sorted(WAIT_SECONDS)},
                            "session_id": {"type": "string"}, "name": {"type": "string"},
                            "channel": {"type": "string", "pattern": "^[1-9]$"},
                            "voice": {"type": "string"}, "tts_vendor": {"type": "string", "enum": ["xai", "openai"]}},
                           ["provider", "session_id", "name", "channel"])},
    {"name": "codescribe_wait", "description": "Wait for one complete owned message without ACKing. Ends with an explicit wait_idle receipt before the host's tool deadline (Junie 150 s, Claude Code 900 s); this is not indefinite idle wakeup. A newer wait supersedes this one; cancel to stop waiting. After read, ACK before acting.",
     "inputSchema": schema({"timeout_seconds": {"type": "integer", "minimum": 1}})},
    {"name": "codescribe_read", "description": "Read one complete pending message without ACKing. Drain before waiting; preserve sender and attachment pointers.", "inputSchema": schema()},
    {"name": "codescribe_ack", "description": "Acknowledge ONLY exact delivery IDs completely read through this MCP connection, immediately before work. ACK means read, not completed.",
     "inputSchema": schema({"delivery_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}}}, ["delivery_ids"])},
    {"name": "codescribe_reply", "description": "Save and speak one requested voice reply associated with an exact read and acknowledged delivery. Retains the existing voice, credentials and microphone/playback interlocks. Report spoken/reason truthfully.",
     "inputSchema": schema({"delivery_id": {"type": "string"}, "text": {"type": "string"}}, ["delivery_id", "text"])},
    {"name": "codescribe_status", "description": "Inspect this connection without claiming listening_verified. Only a fresh delivery and answer prove listening.", "inputSchema": schema()},
    {"name": "codescribe_detach", "description": "Release this conversation's channel on explicit stop; stops an active wait first. Unread messages can prevent release; never delete them.", "inputSchema": schema()},
]


def wake_paths(root, lease_id):
    directory = root / "runtime" / "native-wake"
    return directory / f"{lease_id}.lock", directory / f"{lease_id}.json"


def ledger_lock_path(root, lease_id):
    """The announced ledger's own lock. The waiter holds the arming lock for its
    whole life, so the probe cannot share that one; this one guards a single
    read-modify-write of the ledger on either side (CS160-F028)."""
    return root / "runtime" / "native-wake" / f"{lease_id}.ledger.lock"


def flock_within(descriptor, wait_seconds):
    """Take the exclusive lock; with a bound, give up after `wait_seconds`."""
    if wait_seconds is None:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        return True
    deadline = time.monotonic() + wait_seconds
    while True:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except BlockingIOError:
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.01)


@contextlib.contextmanager
def ledger_guard(path, *, wait_seconds=None):
    """Hold the ledger lock for one read-modify-write; closing the descriptor
    releases it. Yields whether the lock was taken: with `wait_seconds` the
    guard gives up after that long, so a PostToolUse probe never stalls a
    tool call behind a stuck waiter."""
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        yield flock_within(descriptor, wait_seconds)
    finally:
        os.close(descriptor)


def watch_lock_path(root, lease_id):
    """Held shared for the whole life of this lease's `--watch --until-event`
    task. While it is held the native waiter leaves deliveries to that task,
    so one message wakes the conversation once."""
    return root / "runtime" / "native-wake" / f"{lease_id}.watch.lock"


def lock_held(path):
    """True while another process holds `path`; observing never takes it."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return False
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return True
    finally:
        os.close(descriptor)
    return False


def wake_armed(root, lease_id):
    """True while a wake process holds this lease's lock; observing never arms."""
    return lock_held(wake_paths(root, lease_id)[0])


@contextlib.contextmanager
def until_event_watch(demux, root, lease_id):
    """Mark a live `--watch --until-event` task of this lease for its waiter.

    Yields `announce(ids)`. The task calls it after reporting deliveries and
    before it exits, so the ledger already holds them when the waiter takes
    over: the waiter wakes neither alongside the task nor after it for the
    same message. A task stopped before it reported anything leaves no entry,
    and the waiter announces those deliveries itself."""
    path = watch_lock_path(root, lease_id)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_SH)

        def announce(ids):
            _lock, record = wake_paths(root, lease_id)
            with ledger_guard(ledger_lock_path(root, lease_id)):
                saved = (demux.read_json(record) or {}).get("announced")
                seen = set(saved) if isinstance(saved, list) else set()
                demux.atomic_json(record, {"lease_id": lease_id, "announced": sorted(seen | set(ids))})

        yield announce
    finally:
        os.close(descriptor)


def bound_channel(demux, root, provider, session):
    bindings = (demux.read_json(root / demux.AUDIENCE_BINDING_FILENAME) or {}).get("bindings")
    if isinstance(bindings, dict):
        for slot, entry in bindings.items():
            if (isinstance(entry, dict) and entry.get("provider") == provider
                    and entry.get("provider_session_id") == session):
                return str(slot), entry.get("audience")
    return None, None


def local_clock(stamp):
    try:
        return datetime.datetime.fromisoformat(str(stamp).replace("Z", "+00:00")).astimezone().strftime("%H:%M:%S")
    except (TypeError, ValueError):
        return "--:--:--"


def addressing(row):
    """Paint the demux's address projection without inventing another address rule."""
    label = row.get("addressed_to")
    return f"channel {label}" if isinstance(label, str) and label else "an unrecorded address"


def describe_delivery(row):
    """One pending message as the lines an agent needs: who, where, when, the
    whole text, attachment paths and the id it must ACK. A Founder take carries
    no sender field; a peer message names its sender."""
    sender = row.get("sender")
    who = (sender.get("name") if isinstance(sender, dict) else sender) or "Founder"
    kind = "draft" if row.get("kind") == "revised" else str(row.get("kind") or "message")
    note = " · acoustic coverage refused (diagnostic, not a gate)" if row.get("coverage") == "refused" else ""
    lines = [f"Message from {who} on {addressing(row)} ({kind}, {local_clock(row.get('emitted_at'))}{note}):",
             str(row.get("text") or "")]
    for item in row.get("attachments") or []:
        if isinstance(item, dict):
            lines.append(f"[attachment] {item.get('name') or ''}: {item.get('path') or ''}")
    lines.append(f"delivery_id: {row.get('delivery_id')}")
    return "\n".join(lines)


def scalar_lines(result, skip=()):
    out = []
    for key, value in result.items():
        if key in skip or key in ("kind", "schema"):
            continue
        if isinstance(value, (str, int, float, bool)) or value is None:
            out.append(f"{key}: {value}")
        elif isinstance(value, list) and all(isinstance(v, (str, int, float)) for v in value):
            out.append(f"{key}: {', '.join(str(v) for v in value) or '(none)'}")
    return out


def describe(result):
    """Prose for the conversation. Until 2026-10-12 every tool answered with
    json.dumps(result): the Founder watched envelopes scroll through the agent's
    turn (\"koperty z jsona, które się sypią przez wasze outputy\"). The dict is
    still returned as structuredContent for programs and tests."""
    if not isinstance(result, dict):
        return str(result)
    if "receipts" in result and isinstance(result["receipts"], list):
        parts = [describe(row) for row in result["receipts"]]
        if result.get("exit_code"):
            parts.append(f"exit code: {result['exit_code']}")
        return "\n".join(parts)
    kind = result.get("kind")
    if kind == "pending_read":
        rows = result.get("deliveries") or []
        remaining = result.get("remaining", 0)
        if not rows:
            return f"No pending message; remaining {remaining}. Nothing was acknowledged."
        body = "\n\n".join(describe_delivery(row) for row in rows if isinstance(row, dict))
        return (f"{body}\n\n{len(rows)} message(s) read, {remaining} remaining. "
                "ACK the delivery_id(s) above before acting; ACK means read, not done.")
    if kind == "wait_idle":
        return f"No message in {result.get('waited_seconds')} s. {result.get('instructions') or 'Nothing was read or acknowledged.'}"
    if kind == "attach_receipt":
        voice = result.get("voice") if isinstance(result.get("voice"), dict) else {}
        follower = "spawned" if result.get("follower_spawned") else "reused"
        return (f"Connected to channel {result.get('channel')} as {result.get('audience')} "
                f"({result.get('provider')} session {result.get('provider_session_id')}). "
                f"Lease {result.get('lease_id')}, follower pid {result.get('follower_pid')} ({follower}), "
                f"cursor {result.get('cursor')}, resumed {result.get('resumed')}, wakeup {result.get('wakeup')}, "
                f"voice {voice.get('voice')} via {voice.get('provider')} ({result.get('voice_source')}). "
                "State: attached_unverified until a fresh take is read and answered.")
    if kind == "acknowledged":
        settled = result.get("native_queue_settled")
        tail = "" if settled is None else f"; native queue settled: {'yes' if settled else 'no'}"
        return f"Acknowledged delivery {result.get('delivery_id')}{tail}. ACK means read, not done."
    if kind == "agent_reply":
        spoken = "spoken" if result.get("spoken") else "not spoken"
        reason = result.get("reason") or result.get("state")
        return (f"Reply saved to the bus and {spoken}" + (f" ({reason})" if reason and not result.get("spoken") else "")
                + f"; reply to delivery {result.get('reply_to') or result.get('delivery_id')}.")
    if kind == "status":
        attached = "attached" if result.get("attached") else "not attached"
        alive = "alive" if result.get("follower_alive") else "not running"
        return (f"Connection {attached}: channel {result.get('channel')}, name {result.get('name') or result.get('audience')}, "
                f"lease {result.get('lease_id')}, follower {alive} (pid {result.get('follower_pid')}), "
                f"backlog {result.get('backlog')}, cursor {result.get('cursor')}, wakeup {result.get('wakeup')}"
                + (f", native wake armed: {'yes' if result.get('native_wake_armed') else 'no'}"
                   if "native_wake_armed" in result else "") + ".")
    if kind == "detach_receipt":
        released = ", ".join(str(c) for c in result.get("released_channels") or []) or "none"
        unacked = result.get("unacked_delivery_ids") or []
        return (f"Detached: released channel(s) {released}; follower {result.get('follower_state')} "
                f"(pid {result.get('follower_pid')}); was attached: {result.get('was_attached')}; "
                f"{len(unacked)} unacknowledged delivery(ies)" + (": " + ", ".join(unacked) if unacked else "") + ".")
    head = [f"{kind}:"] if kind else []
    return "\n".join(head + scalar_lines(result)) or "(empty result)"


def describe_wake(notice, provider, session, channel, name):
    where = f"for {name}, connected on channel {channel} ({provider} session {session})"
    kind = notice.get("kind")
    if kind == "codescribe_listener_problem":
        return f"Codescribe: listener problem {notice.get('problem')} {where}. {notice.get('instructions')}"
    if kind == "codescribe_wait_idle":
        return f"Codescribe: no message {where} in {notice.get('waited_seconds')} s. {notice.get('instructions')}"
    ids = notice.get("new_delivery_ids") or []
    said = notice.get("addressed") or {}
    problem = f"; problem: {notice['problem']}" if notice.get("problem") else ""
    listed = ", ".join(f"{identity} [{said[identity]}]" if identity in said else identity for identity in ids)
    return (f"Codescribe: {len(ids)} new message(s) {where}; pending {notice.get('pending_count')}"
            f"{problem}; new delivery ids: {listed}. {notice.get('instructions')}")


def addressed(rows, ids, demux):
    """Each announced delivery's channel and audience, keyed by its id."""
    by_id = {row.get("delivery_id"): row for row in rows}
    return {
        identity: addressing({"addressed_to": demux.delivery_address_label(by_id[identity])})
        for identity in ids if identity in by_id
    }


def claude_code_wake(demux, root, bus, payload, environ, *, window=WAKE_SECONDS, interval=1.0):
    """Claude Code asyncRewake hook: return WAKE_EXIT once per unannounced delivery.

    The hook never reads, ACKs or replies. It wakes only the Claude Code
    conversation whose exported session id equals the hook payload, and only
    while that session owns a channel. One process per lease holds the lock;
    a later Stop or tool call finds it armed and returns at once. Claude Code
    runs no Stop hook after a user interrupt, and StopFailure instead of Stop
    after an API error, so the plugin also arms after every tool call: a turn
    the wake started cannot end without a waiter. Announced IDs are kept until
    acknowledged, so a message left unread does not wake every turn.
    A channel attached on another Mac (`cs-bus --remote HOST --attach`) is
    waited on there, by that Mac's own waiter, in parallel with this one.
    """
    session = payload.get("session_id") if isinstance(payload, dict) else None
    if not isinstance(session, str) or not session or environ.get("CLAUDE_CODE_SESSION_ID") != session:
        return 0
    if any(ord(character) < 32 for character in session):
        return 0
    hosts = demux.remote_hosts(root, demux.lease_identifier("claude-code", session))
    if hosts:
        return wake_here_and_remote(demux, root, bus, session, environ, hosts,
                                    window=window, interval=interval)
    return native_wake(demux, root, bus, "claude-code", session, environ.get("CLAUDE_PID", ""),
                       window=window, interval=interval)


#: ssh exits 255 when it cannot connect; the remote waiter is retried.
SSH_FAILED = 255
REMOTE_RETRY_SECONDS = 30.0


def remote_wake_command(session):
    """The remote half: that Mac's own `cs-bus --claude-code-wake` for this
    session (its lock, announced ledger and notice), plus a guard on ssh stdin.
    When this side goes away, stdin ends and the guard stops the waiter, so no
    orphan keeps the remote lock and announces a message nobody hears."""
    script = "\n".join([
        "exec 3<&0",
        "(",
        f"  CLAUDE_CODE_SESSION_ID={shlex.quote(session)}",
        "  export CLAUDE_CODE_SESSION_ID",
        "  exec \"$HOME/.local/bin/cs-bus\" --claude-code-wake 3<&- <<'CODESCRIBE_WAKE'",
        json.dumps({"session_id": session}),
        "CODESCRIBE_WAKE",
        ") &",
        "waiter=$!",
        '( cat <&3 >/dev/null 2>&1; kill -TERM "$waiter" 2>/dev/null ) >/dev/null 2>&1 &',
        "guard=$!",
        "exec 3<&-",
        'wait "$waiter"; rc=$?',
        'kill "$guard" 2>/dev/null',
        'exit "$rc"',
    ])
    return "/bin/sh -c " + shlex.quote(script)


def remote_wake(demux, host, session, environ, procs, *, window, alive):
    """Run HOST's waiter over ssh until it answers; retry while ssh cannot connect."""
    deadline = time.monotonic() + window
    while time.monotonic() < deadline and alive():
        try:
            proc = subprocess.Popen(demux.ssh_command(host, remote_wake_command(session), environ),
                                    stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.PIPE, text=True, env=dict(environ))
        except OSError:
            return 0, ""
        procs.append(proc)
        # Cleanup may have started while Popen was completing. Either it saw
        # this registered process, or this guard releases the late arrival.
        if not alive():
            release_remote(proc)
            return 0, ""
        text = proc.stderr.read()
        code = proc.wait()
        if code != SSH_FAILED:
            return code, text
        retry = time.monotonic() + REMOTE_RETRY_SECONDS
        while time.monotonic() < min(retry, deadline) and alive():
            time.sleep(1.0)
    return 0, ""


def release_remote(proc):
    """Closing ssh stdin ends the remote guard's read; it stops the waiter."""
    if proc.poll() is not None:
        return
    try:
        proc.stdin.close()
    except OSError:
        pass
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()


def describe_remote_wake(host, text):
    said = [line.strip() for line in text.splitlines() if line.strip().startswith("Codescribe:")]
    head = said[-1] if said else "Codescribe: the waiter answered without a notice."
    return (f"{head} [Mailbox on {host}, over ssh: read it with `cs-bus --remote {host} --read-pending "
            f"--provider claude-code` and ACK with `cs-bus --remote {host} --provider claude-code --ack ID`; "
            "codescribe_read reads only this Mac.]\n")


def wake_here_and_remote(demux, root, bus, session, environ, hosts, *, window, interval):
    """The first waiter to answer wakes the conversation; the others are released."""
    results = queue.Queue()
    procs = []
    workers = []
    noticed = set()
    stop = threading.Event()
    host_pid = environ.get("CLAUDE_PID", "")

    def alive():
        return not stop.is_set() and (
            not host_pid.isdigit() or demux.process_is_alive(int(host_pid)))

    sources = 0
    here = None
    if bound_channel(demux, root, "claude-code", session)[0] is not None:
        sources += 1
        here = threading.Thread(target=lambda: results.put(("", native_wake(
            demux, root, bus, "claude-code", session, host_pid, window=window, interval=interval,
            stop=stop), "")), daemon=True)
        here.start()
    for host in hosts:
        sources += 1
        worker = threading.Thread(target=lambda host=host: results.put((host, *remote_wake(
            demux, host, session, environ, procs, window=window, alive=alive))), daemon=True)
        workers.append(worker)
        worker.start()
    try:
        while sources:
            try:
                source, code, text = results.get(timeout=max(interval, 1.0))
            except queue.Empty:
                if not alive():
                    return 0
                continue
            sources -= 1
            if code == WAKE_EXIT:
                if source:
                    noticed.add(source)
                    sys.stderr.write(describe_remote_wake(source, text))
                return WAKE_EXIT
        return 0
    finally:
        stop.set()
        for proc in list(procs):
            release_remote(proc)
        for worker in workers:
            worker.join(timeout=6)
        # A remote waiter may already have marked a delivery announced when
        # the local waiter wins. Preserve its notice after releasing both.
        while True:
            try:
                source, code, text = results.get_nowait()
            except queue.Empty:
                break
            if source and source not in noticed and code == WAKE_EXIT:
                noticed.add(source)
                sys.stderr.write(describe_remote_wake(source, text))
        if here is not None:
            here.join(timeout=interval + 1.0)


def omp_wake(demux, root, bus, session, host_pid, *, window=WAKE_SECONDS, interval=1.0):
    """OMP extension waiter: `cs-bus --omp-wake --provider omp --session ID --host-pid PID`.

    OMP exports no session id to child processes, so the bundled extension
    passes the id it read from its own session manager and its host pid. The
    waiter has the same contract as the Claude Code hook: exit WAKE_EXIT once
    per unannounced delivery, never read, ACK or reply, one lock per lease,
    silent exit when the session owns no channel or the host process is gone.
    """
    if not isinstance(session, str) or not session.strip() or any(ord(c) < 32 for c in session):
        return 0
    return native_wake(demux, root, bus, "omp", session, "" if host_pid is None else str(host_pid),
                       window=window, interval=interval)


def native_wake(demux, root, bus, provider, session, host, *, window=WAKE_SECONDS, interval=1.0,
                stop=None):
    """Hold this lease's wake lock until one notice is due, then exit WAKE_EXIT.
    A set `stop` (another waiter answered first) ends the wait silently."""
    if bound_channel(demux, root, provider, session)[0] is None:
        return 0
    lease_id = demux.lease_identifier(provider, session)
    lock, record = wake_paths(root, lease_id)
    lock.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return 0
        saved = (demux.read_json(record) or {}).get("announced")
        seen = set(saved) if isinstance(saved, list) else set()
        args = SimpleNamespace(bridge_home=root, provider=provider, session=session,
                               bus=bus, bus_overridden=bus is not None)
        trigger = demux.BusEventTrigger(root / "leases" / f"{lease_id}.json", 0.1)
        ledger_lock = ledger_lock_path(root, lease_id)
        deadline = time.monotonic() + window
        try:
            while True:
                if stop is not None and stop.is_set():
                    return 0
                channel, name = bound_channel(demux, root, provider, session)
                if channel is None:
                    return 0
                if host.isdigit() and not demux.process_is_alive(int(host)):
                    return 0
                # One read-modify-write of the ledger per pass, under the lock
                # the PostToolUse probe shares: a delivery the probe surfaced is
                # in the record this pass reads, and the probe cannot write
                # between this mailbox read and this write (CS160-F028).
                with ledger_guard(ledger_lock):
                    try:
                        _state, rows = demux.unread_pending(args)
                        problem = None if demux.live_follower_pid(root, lease_id) else "follower_lost"
                    except (OSError, ValueError) as error:
                        rows, problem = [], f"mailbox_refused: {error}"
                    unread = [row["delivery_id"] for row in rows]
                    # A PostToolUse probe may have surfaced a delivery mid-turn; its
                    # ledger entry keeps this waiter from announcing it again.
                    probed = (demux.read_json(record) or {}).get("announced")
                    if isinstance(probed, list):
                        seen |= set(probed)
                    # A live `--watch --until-event` task of this lease reports its
                    # deliveries itself and records them before it exits; waking
                    # here as well would wake the conversation twice. The waiter
                    # stays armed and takes over once the task is gone.
                    watched = lock_held(watch_lock_path(root, lease_id))
                    fresh = [] if watched else [identity for identity in unread if identity not in seen]
                    # After this pass every unread delivery is announced; an ACKed
                    # delivery or a resolved problem leaves the set. A delivery the
                    # task has not reported yet stays unannounced.
                    prior = seen
                    seen = ((seen & set(unread)) if watched else set(unread)) | ({problem} if problem else set())
                    if fresh:
                        notice = {"kind": "codescribe_mailbox_ready", "pending_count": len(unread),
                                  "new_delivery_ids": fresh, "addressed": addressed(rows, fresh, demux),
                                  **({"problem": problem} if problem else {}),
                                  "instructions": f"Read every pending message (codescribe_read, or cs-bus --read-pending --provider {provider}), ACK exactly its read_delivery_ids before acting, answer, then end the turn. This hook re-arms itself."}
                    elif problem and problem not in prior:
                        notice = {"kind": "codescribe_listener_problem", "problem": problem,
                                  "instructions": "Nothing was read or acknowledged. Inspect codescribe_status; connect again with the same name and channel to restart a lost follower."}
                    elif time.monotonic() >= deadline:
                        if watched:
                            # The task keeps waking this conversation; an idle
                            # re-arm would only spend a model turn. The next turn
                            # end or tool call arms a fresh waiter.
                            return 0
                        notice = {"kind": "codescribe_wait_idle", "waited_seconds": window,
                                  "instructions": "No message arrived. Nothing was read or acknowledged; end the turn and this hook re-arms."}
                    else:
                        notice = None
                    if notice is not None:
                        demux.atomic_json(record, {"lease_id": lease_id, "announced": sorted(seen)})
                if notice is None:
                    trigger.wait(timeout=interval)
                    continue
                sys.stderr.write(describe_wake(notice, provider, session, channel, name) + "\n")
                return WAKE_EXIT
        finally:
            trigger.close()
    finally:
        os.close(descriptor)


def claude_code_probe(demux, root, bus, payload, environ):
    """Claude Code PostToolUse hook: surface an unannounced delivery mid-turn.

    Runs after every tool call of the conversation, so while the mailbox is
    silent it costs one lease read and prints nothing. It never reads, ACKs,
    replies or waits. A surfaced delivery joins the announced ledger the Stop
    waiter keeps, so the end of the turn does not announce it again; the
    ledger drops an ID once it is acknowledged. Mailbox problems stay with the
    waiter, which announces them once.
    """
    session = payload.get("session_id") if isinstance(payload, dict) else None
    if not isinstance(session, str) or not session or environ.get("CLAUDE_CODE_SESSION_ID") != session:
        return 0
    provider = "claude-code"
    channel, name = bound_channel(demux, root, provider, session)
    if channel is None:
        return 0
    lease_id = demux.lease_identifier(provider, session)
    _lock, record = wake_paths(root, lease_id)
    args = SimpleNamespace(bridge_home=root, provider=provider, session=session,
                           bus=bus, bus_overridden=bus is not None)
    # The ledger lock the Stop waiter takes around its own mailbox read and
    # ledger write (CS160-F028). Without it the waiter could overwrite this
    # probe's entry with its earlier snapshot and announce the same delivery
    # again at the turn end. A waiter pass takes milliseconds; a lock still
    # busy after PROBE_LEDGER_WAIT_SECONDS means the probe stays silent and
    # the waiter announces the delivery itself.
    with ledger_guard(ledger_lock_path(root, lease_id), wait_seconds=PROBE_LEDGER_WAIT_SECONDS) as held:
        if not held:
            return 0
        try:
            # The probe runs inside a turn the model is already spending, so a
            # quiet channel-0 delivery costs no wake here: surface it now rather
            # than leaving it for an explicit read. The Stop waiter above keeps
            # the default (quiet rows never start a turn).
            _state, rows = demux.unread_pending(args, include_quiet=True)
        except (OSError, ValueError):
            return 0
        unread = [row["delivery_id"] for row in rows]
        saved = (demux.read_json(record) or {}).get("announced")
        seen = set(saved) if isinstance(saved, list) else set()
        fresh = [identity for identity in unread if identity not in seen]
        if not fresh:
            return 0
        demux.atomic_json(record, {"lease_id": lease_id, "announced": sorted(seen | set(unread))})
    notice = {"kind": "codescribe_mailbox_ready", "pending_count": len(unread), "new_delivery_ids": fresh,
              "addressed": addressed(rows, fresh, demux),
              "instructions": "Read every pending message now (codescribe_read, or cs-bus --read-pending --provider claude-code), ACK exactly its read_delivery_ids, then act on it before continuing the current work."}
    sys.stdout.write(json.dumps({"hookSpecificOutput": {
        "hookEventName": "PostToolUse",
        "additionalContext": describe_wake(notice, provider, session, channel, name)}}) + "\n")
    return 0


class Mailbox:
    # Shared in-process contract for BUS clients shipped in the managed payload.
    # Additive receipt fields do not change this version; breaking changes do.
    api_version = 1

    def __init__(self, demux, root, bus=None):
        self.demux, self.root, self.bus = demux, root, bus
        self.owner = self.provider = None
        self.read_ids, self.acked_ids, self.replied_ids = set(), set(), set()
        self.lock = threading.RLock()
        self.wait_guard = threading.Lock()
        self.active_wait = None
        self.command = [sys.executable, str(Path(demux.__file__).with_name("cs-bus"))]
        # None inherits this process's environment; a client may pin an exact one.
        self.environment = None

    def run(self, flags, timeout=30):
        command = [*self.command, "--bridge-home", str(self.root)]
        if self.bus is not None:
            command += ["--bus", str(self.bus)]
        if self.owner is not None:
            command += ["--provider", self.provider, "--session", self.owner]
        result = subprocess.run([*command, *flags], capture_output=True, text=True, timeout=timeout,
                                env=self.environment)
        if result.returncode:
            # Voice failure still carries its durable reply receipt.
            if "--say" in flags and result.stdout.strip():
                return {"receipts": [json.loads(line) for line in result.stdout.splitlines()], "exit_code": result.returncode}
            raise ValueError(result.stderr.strip() or "Codescribe operation refused")
        rows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        return rows[0] if len(rows) == 1 else {"receipts": rows}

    def connect(self, values):
        provider, session = values["provider"], values["session_id"]
        if provider not in WAIT_SECONDS:
            raise ValueError("provider must be one of: " + ", ".join(sorted(WAIT_SECONDS)))
        if not isinstance(session, str) or not session.strip() or any(ord(c) < 32 for c in session):
            raise ValueError("real native session_id required")
        if not isinstance(values["name"], str) or not values["name"].strip():
            raise ValueError("nonempty agent name required")
        if values["channel"] not in tuple(str(n) for n in range(1, 10)):
            raise ValueError("channel must be a single digit 1-9")
        if self.owner is not None and (self.provider, self.owner) != (provider, session):
            raise ValueError("this MCP connection already belongs to another session; detach it first")
        flags = ["--attach", "--name", values["name"], "--channel", values["channel"]]
        for key, flag in (("voice", "--voice"), ("tts_vendor", "--tts-vendor")):
            if key in values:
                flags += [flag, values[key]]
        previous = self.provider, self.owner
        self.provider, self.owner = provider, session
        try:
            return self.run(flags)
        except Exception:
            self.provider, self.owner = previous
            raise

    def read(self):
        result = self.run(["--read-pending", "--read-limit", "1"])
        self.read_ids.update(result["read_delivery_ids"])
        return result

    def stop_wait(self):
        """Supersede the active wait and wait for it to release its slot."""
        with self.wait_guard:
            active = self.active_wait
        if active is None:
            return
        stop, done = active
        stop.set()
        if not done.wait(timeout=5):
            raise ValueError("the previous Codescribe wait did not stop; nothing was read or acknowledged")

    def wait(self, cancel, seconds=None):
        if self.owner is None:
            raise ValueError("connect using THIS session's native session_id first")
        window = WAIT_SECONDS[self.provider]
        if seconds is not None and not 1 <= seconds <= window:
            raise ValueError(f"timeout_seconds must be between 1 and {window} for {self.provider}")
        # A client that gave up on a wait never cancels it (Junie's deadline);
        # the newer wait supersedes that orphan instead of refusing forever.
        self.stop_wait()
        stop, done = threading.Event(), threading.Event()
        with self.wait_guard:
            self.active_wait = stop, done
        try:
            args = SimpleNamespace(bridge_home=self.root, provider=self.provider, session=self.owner,
                                   bus=self.bus, bus_overridden=self.bus is not None)
            source = self.root / "leases" / f"{self.demux.lease_identifier(self.provider, self.owner)}.json"
            trigger = self.demux.BusEventTrigger(source, 0.1)
            started = time.monotonic()
            deadline = started + (window if seconds is None else seconds)
            try:
                while not cancel.is_set() and not stop.is_set():
                    _state, rows = self.demux.unread_pending(args)
                    if rows:
                        with self.lock:
                            if cancel.is_set() or stop.is_set():
                                break
                            result = self.read()
                        if result["deliveries"]:
                            return result
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        rearm = ("Call codescribe_wait again to keep this listening turn open, or end the turn: "
                                 f"the Codescribe {'Stop hook' if self.provider == 'claude-code' else 'OMP extension'} "
                                 "wakes this conversation on the next message."
                                 if self.provider in NATIVE_WAKE_PROVIDERS else "Call codescribe_wait again to keep listening.")
                        return {"kind": "wait_idle", "provider": self.provider, "provider_session_id": self.owner,
                                "waited_seconds": round(time.monotonic() - started), "read_delivery_ids": [],
                                "acknowledged": False,
                                "playback": self.demux.playback_mute_snapshot(
                                    self.root, self.provider, self.owner, Path(_state["bus"])),
                                "instructions": "No message arrived. Nothing was read or acknowledged. " + rearm}
                    trigger.wait(timeout=min(0.5, remaining))
                if stop.is_set() and not cancel.is_set():
                    raise ValueError("Codescribe wait superseded by a newer wait or detach; nothing was acknowledged")
                raise ValueError("Codescribe wait cancelled; nothing was acknowledged")
            finally:
                trigger.close()
        finally:
            with self.wait_guard:
                if self.active_wait == (stop, done):
                    self.active_wait = None
            done.set()

    def status(self):
        result = self.run(["--status"])
        if self.provider in NATIVE_WAKE_PROVIDERS:
            result["native_wake_armed"] = wake_armed(self.root, self.demux.lease_identifier(self.provider, self.owner))
        return result

    def call(self, name, values, cancel):
        definition = next((tool for tool in TOOLS if tool["name"] == name), None)
        if definition is None:
            raise ValueError("unknown Codescribe tool")
        contract = definition["inputSchema"]
        if not isinstance(values, dict) or set(values) - set(contract["properties"]) or any(key not in values for key in contract["required"]):
            raise ValueError("invalid tool arguments")
        for key, value in values.items():
            prop = contract["properties"][key]
            kind = prop["type"]
            if (kind == "string" and not isinstance(value, str) or kind == "array" and not isinstance(value, list)
                    or kind == "integer" and (not isinstance(value, int) or isinstance(value, bool))):
                raise ValueError("invalid tool argument type")
            if "enum" in prop and value not in prop["enum"]:
                raise ValueError(f"{key} must be one of: " + ", ".join(prop["enum"]))
        if name == "codescribe_wait":
            return self.wait(cancel, values.get("timeout_seconds"))
        if name == "codescribe_detach":
            self.stop_wait()
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
                return self.status()
            result = self.run(["--detach"])
            self.owner = self.provider = None
            return result


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
            # Prose is what the conversation shows; the dict travels as
            # structuredContent (MCP 2025-06-18) for programs and tests.
            response = {"content": [{"type": "text", "text": describe(result)}], "structuredContent": result}
        except (ValueError, OSError, subprocess.SubprocessError, KeyError, TypeError) as error:
            # An unexpected helper output shape is still an answer, never a hung request.
            response = {"isError": True, "content": [{"type": "text", "text": str(error) or type(error).__name__}]}
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
                          "capabilities": {"tools": {}}, "serverInfo": {"name": "codescribe", "version": "1.1.0"}}})
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
