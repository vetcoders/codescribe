#!/usr/bin/env python3
"""Codescribe transcript bus -> Codex App Server -> local spoken reply.

This bridge never opens a microphone. Codescribe.app remains the single audio
capture owner and ``scripts/bus-demux.py`` remains the transcript parser and
named-mailbox filter. The bridge owns one Codex thread, submits only sealed
transcripts, interrupts the active turn on an addressed live draft, and speaks
the final user-facing answer with macOS ``say``.

The default safety profile is ``workspace-write`` plus ``approvalPolicy=never``.
Voice never approves sandbox escapes, network access, or other privileged work.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time
from collections import deque
from pathlib import Path
from typing import Any


APP_SERVER_TIMEOUT_SECONDS = 30.0
STOP_PHRASES = {
    "stop",
    "przerwij",
    "czekaj",
    "poczekaj",
    "nie rób",
    "nie rob",
    "nie rób tego",
    "nie rob tego",
}
APPROVAL_METHODS = {
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "applyPatchApproval",
    "execCommandApproval",
}


def log(message: str) -> None:
    sys.stderr.write(f"codescribe voice: {message}\n")
    sys.stderr.flush()


def compact_error(message: dict[str, Any]) -> str:
    error = message.get("error")
    if isinstance(error, dict):
        return str(error.get("message") or error)
    return str(error or "unknown app-server error")


def strip_address_prefix(text: str, name: str) -> str:
    """Remove one leading wake-word/name stamp without rewriting the command."""

    escaped = re.escape(name.strip())
    pattern = re.compile(
        rf"^\s*(?:(?:hej|hey|cześć|czesc|hello)\s+)?{escaped}"
        rf"(?:ie|owi|a|em|u)?\b\s*[,.:;!?-]*\s*",
        re.IGNORECASE,
    )
    return pattern.sub("", text, count=1).strip()


def is_stop_only(text: str) -> bool:
    normalized = re.sub(r"[.!?,:;]+", "", text.casefold()).strip()
    return normalized in STOP_PHRASES


def speech_text(text: str, max_chars: int) -> str:
    """Keep spoken output user-facing: no code blocks, raw URLs, or Markdown chrome."""

    text = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
    text = re.sub(r"\[([^\]]+)\]\([^\)]+\)", r"\1", text)
    text = re.sub(r"https?://\S+", " ", text)
    text = text.replace("`", "")
    text = re.sub(r"(?m)^\s{0,3}#{1,6}\s*", "", text)
    text = re.sub(r"(?m)^\s*[-*+]\s+", "", text)
    text = re.sub(r"\s+", " ", text).strip()
    if len(text) <= max_chars:
        return text
    clipped = text[:max_chars].rsplit(" ", 1)[0].rstrip(" ,;:")
    return f"{clipped}. Dalsza część jest w tekście."


class AppServerError(RuntimeError):
    """Codex App Server protocol or process error."""


class CodexAppServer:
    def __init__(
        self,
        codex_bin: Path,
        events: queue.Queue[tuple[str, dict[str, Any]]],
        *,
        debug: bool,
    ) -> None:
        command = [str(codex_bin), "app-server", "--stdio"]
        if codex_bin.suffix == ".py":
            command.insert(0, sys.executable)
        self._events = events
        self._debug = debug
        self._write_lock = threading.Lock()
        self._pending_lock = threading.Lock()
        self._pending: dict[int, queue.Queue[dict[str, Any]]] = {}
        self._next_id = 1
        self._process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr = threading.Thread(target=self._read_stderr, daemon=True)
        self._reader.start()
        self._stderr.start()

    def _send(self, message: dict[str, Any]) -> None:
        if self._process.stdin is None:
            raise AppServerError("app-server stdin is unavailable")
        encoded = json.dumps(message, ensure_ascii=False, separators=(",", ":"))
        with self._write_lock:
            self._process.stdin.write(encoded + "\n")
            self._process.stdin.flush()

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        self._send({"method": method, "params": params or {}})

    def request(
        self,
        method: str,
        params: dict[str, Any],
        *,
        timeout: float = APP_SERVER_TIMEOUT_SECONDS,
    ) -> dict[str, Any]:
        with self._pending_lock:
            request_id = self._next_id
            self._next_id += 1
            response_queue: queue.Queue[dict[str, Any]] = queue.Queue(maxsize=1)
            self._pending[request_id] = response_queue
        try:
            self._send({"method": method, "id": request_id, "params": params})
            response = response_queue.get(timeout=timeout)
        except queue.Empty as error:
            raise AppServerError(f"timeout waiting for {method}") from error
        finally:
            with self._pending_lock:
                self._pending.pop(request_id, None)
        if "error" in response:
            raise AppServerError(f"{method}: {compact_error(response)}")
        result = response.get("result")
        return result if isinstance(result, dict) else {}

    def _read_stdout(self) -> None:
        if self._process.stdout is None:
            return
        for raw in self._process.stdout:
            try:
                message = json.loads(raw)
            except json.JSONDecodeError:
                log("app-server emitted invalid JSON")
                continue
            request_id = message.get("id")
            if request_id is not None and ("result" in message or "error" in message):
                with self._pending_lock:
                    waiter = self._pending.get(request_id)
                if waiter is not None:
                    waiter.put(message)
                    continue
            if request_id is not None and isinstance(message.get("method"), str):
                self._decline_server_request(message)
                continue
            if isinstance(message.get("method"), str):
                self._events.put(("codex", message))
        with self._pending_lock:
            waiters = list(self._pending.values())
        for waiter in waiters:
            try:
                waiter.put_nowait(
                    {
                        "error": {
                            "code": -32000,
                            "message": "app-server exited before responding",
                        }
                    }
                )
            except queue.Full:
                pass
        self._events.put(
            ("system", {"kind": "app_server_exit", "code": self._process.poll()})
        )

    def _decline_server_request(self, message: dict[str, Any]) -> None:
        method = str(message.get("method"))
        request_id = message.get("id")
        if method in APPROVAL_METHODS:
            log(f"approval declined: {method}")
            self._send({"id": request_id, "result": {"decision": "decline"}})
            return
        log(f"unsupported interactive request declined: {method}")
        self._send(
            {
                "id": request_id,
                "error": {
                    "code": -32601,
                    "message": "Codescribe voice bridge does not answer interactive requests",
                },
            }
        )

    def _read_stderr(self) -> None:
        if self._process.stderr is None:
            return
        for raw in self._process.stderr:
            if self._debug:
                log(f"app-server: {raw.rstrip()}")

    def close(self) -> None:
        if self._process.poll() is not None:
            return
        self._process.terminate()
        try:
            self._process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self._process.kill()


class BusDemux:
    def __init__(
        self,
        demux: Path,
        bus: Path,
        name: str,
        events: queue.Queue[tuple[str, dict[str, Any]]],
        *,
        debug: bool,
    ) -> None:
        self._events = events
        self._debug = debug
        self._process = subprocess.Popen(
            [
                sys.executable,
                str(demux),
                "--bus",
                str(bus),
                "--name",
                name,
                "--follow",
                "--drafts",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr = threading.Thread(target=self._read_stderr, daemon=True)
        self._reader.start()
        self._stderr.start()

    def _read_stdout(self) -> None:
        if self._process.stdout is None:
            return
        for raw in self._process.stdout:
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError:
                log("bus demux emitted invalid JSON")
                continue
            if isinstance(payload, dict):
                self._events.put(("bus", payload))
        self._events.put(("system", {"kind": "bus_demux_exit", "code": self._process.poll()}))

    def _read_stderr(self) -> None:
        if self._process.stderr is None:
            return
        for raw in self._process.stderr:
            if self._debug or "unnamed agent" in raw:
                log(f"demux: {raw.rstrip()}")

    def close(self) -> None:
        if self._process.poll() is not None:
            return
        self._process.terminate()
        try:
            self._process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self._process.kill()


class SayPlayer:
    def __init__(self, *, enabled: bool, voice: str | None, max_chars: int) -> None:
        self.enabled = enabled
        self.voice = voice
        self.max_chars = max_chars
        self._lock = threading.Lock()
        self._process: subprocess.Popen[str] | None = None

    def is_playing(self) -> bool:
        with self._lock:
            return self._process is not None and self._process.poll() is None

    def speak(self, text: str) -> None:
        if not self.enabled:
            return
        spoken = speech_text(text, self.max_chars)
        if not spoken:
            return
        self.stop()
        command = ["/usr/bin/say"]
        if self.voice:
            command.extend(["-v", self.voice])
        command.append(spoken)
        with self._lock:
            self._process = subprocess.Popen(
                command,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )

    def stop(self) -> None:
        with self._lock:
            process = self._process
            self._process = None
        if process is None or process.poll() is not None:
            return
        process.terminate()
        try:
            process.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            process.kill()


class VoiceBridge:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.events: queue.Queue[tuple[str, dict[str, Any]]] = queue.Queue()
        self.codex = CodexAppServer(args.codex_bin, self.events, debug=args.debug)
        self.demux: BusDemux | None = None
        self.tts = SayPlayer(
            enabled=not args.no_tts,
            voice=args.voice,
            max_chars=args.tts_max_chars,
        )
        self.thread_id = ""
        self.active_turn_id: str | None = None
        self.active_turn_text = ""
        self.final_by_turn: dict[str, str] = {}
        self.pending: deque[tuple[str, str]] = deque()
        self.seen_seals: set[str] = set()
        self.seen_seal_order: deque[str] = deque()
        self.interrupted_turns: set[str] = set()
        self.terminal_turns = 0

    def initialize(self) -> None:
        self.codex.request(
            "initialize",
            {
                "clientInfo": {
                    "name": "codescribe-voice-bridge",
                    "title": "Codescribe Voice Bridge",
                    "version": "0.1.0",
                },
                "capabilities": {
                    "experimentalApi": False,
                    "requestAttestation": False,
                },
            },
        )
        self.codex.notify("initialized")
        common = {
            "cwd": str(self.args.cwd),
            "approvalPolicy": "never",
            "sandbox": self.args.sandbox,
            "developerInstructions": (
                "This thread receives sealed voice transcripts from Codescribe. "
                "Treat speech as ordinary user input, never as approval for a sandbox escape, "
                "network access, credential use, destructive action, push, merge, or release. "
                "Keep final answers concise and suitable for speech; put code and exact paths in text."
            ),
        }
        if self.args.thread_id:
            result = self.codex.request(
                "thread/resume",
                {"threadId": self.args.thread_id, **common},
            )
            thread = result.get("thread") or {}
            status = (thread.get("status") or {}).get("type")
            if status == "active":
                raise AppServerError(
                    "refusing active thread: Desktop or another writer may own it"
                )
        else:
            result = self.codex.request(
                "thread/start",
                {
                    **common,
                    "ephemeral": self.args.ephemeral,
                    "serviceName": "codescribe-voice",
                    "threadSource": "codescribeVoice",
                },
            )
            thread = result.get("thread") or {}
        self.thread_id = str(thread.get("id") or "")
        if not self.thread_id:
            raise AppServerError("app-server did not return a thread id")
        if not self.args.thread_id:
            try:
                self.codex.request(
                    "thread/name/set",
                    {
                        "threadId": self.thread_id,
                        "name": f"Codescribe voice — {self.args.name}",
                    },
                )
            except AppServerError as error:
                log(f"thread name unavailable: {error}")
        self.demux = BusDemux(
            self.args.demux,
            self.args.bus,
            self.args.name,
            self.events,
            debug=self.args.debug,
        )
        log(
            f"ready name={self.args.name} thread={self.thread_id} "
            f"bus={self.args.bus} tts={'off' if self.args.no_tts else 'say'}"
        )

    def _interrupt_active(self, reason: str) -> None:
        self.tts.stop()
        if self.active_turn_id is None or self.active_turn_id in self.interrupted_turns:
            return
        turn_id = self.active_turn_id
        try:
            self.codex.request(
                "turn/interrupt",
                {"threadId": self.thread_id, "turnId": turn_id},
                timeout=5,
            )
            self.interrupted_turns.add(turn_id)
            log(f"interrupt sent turn={turn_id} reason={reason}")
        except AppServerError as error:
            log(f"interrupt failed turn={turn_id}: {error}")

    def _start_turn(self, prompt: str, session_id: str) -> None:
        if is_stop_only(prompt):
            log("stop phrase consumed; no active turn remains")
            return
        sys.stdout.write(f"\n[{self.args.name} ← voice] {prompt}\n\n")
        sys.stdout.flush()
        result = self.codex.request(
            "turn/start",
            {
                "threadId": self.thread_id,
                "clientUserMessageId": f"codescribe:{session_id}",
                "input": [{"type": "text", "text": prompt, "text_elements": []}],
            },
        )
        turn = result.get("turn") or {}
        turn_id = str(turn.get("id") or "")
        if not turn_id:
            raise AppServerError("turn/start did not return a turn id")
        self.active_turn_id = turn_id
        self.active_turn_text = ""
        log(f"turn started id={turn_id} session={session_id}")

    def _handle_bus(self, payload: dict[str, Any]) -> None:
        status = str(payload.get("status") or "")
        session_id = str(payload.get("session_id") or "").strip()
        if not session_id:
            log("transcript event without session_id ignored")
            return
        if status in {"utterance_draft", "utterance_revised"}:
            if self.active_turn_id is not None:
                self._interrupt_active("addressed_live_speech")
            elif self.tts.is_playing():
                self.tts.stop()
                log("playback stopped on addressed live speech")
            return
        if status != "transcript_sealed" or payload.get("kind") == "name_assignment":
            return
        if session_id in self.seen_seals:
            log(f"duplicate seal ignored session={session_id}")
            return
        self.seen_seals.add(session_id)
        self.seen_seal_order.append(session_id)
        if len(self.seen_seal_order) > 2048:
            expired = self.seen_seal_order.popleft()
            self.seen_seals.discard(expired)
        text = strip_address_prefix(str(payload.get("text") or ""), self.args.name)
        if not text:
            return
        self.tts.stop()
        if self.active_turn_id is not None:
            self.pending.append((text, session_id))
            self._interrupt_active("sealed_new_command")
            return
        self._start_turn(text, session_id)

    def _handle_codex(self, message: dict[str, Any]) -> None:
        method = str(message.get("method") or "")
        params = message.get("params") or {}
        if method == "item/agentMessage/delta":
            if params.get("turnId") != self.active_turn_id:
                return
            delta = str(params.get("delta") or "")
            self.active_turn_text += delta
            sys.stdout.write(delta)
            sys.stdout.flush()
            return
        if method == "item/completed":
            item = params.get("item") or {}
            if item.get("type") != "agentMessage":
                return
            turn_id = str(params.get("turnId") or "")
            phase = item.get("phase")
            text = str(item.get("text") or "")
            if phase == "final_answer" or (phase is None and text):
                self.final_by_turn[turn_id] = text
            return
        if method == "turn/completed":
            turn = params.get("turn") or {}
            turn_id = str(turn.get("id") or "")
            if turn_id != self.active_turn_id:
                return
            status = str(turn.get("status") or "unknown")
            final = self.final_by_turn.pop(turn_id, "")
            if self.active_turn_text and not self.active_turn_text.endswith("\n"):
                sys.stdout.write("\n")
            sys.stdout.flush()
            log(f"turn completed id={turn_id} status={status}")
            self.active_turn_id = None
            self.active_turn_text = ""
            self.interrupted_turns.discard(turn_id)
            self.terminal_turns += 1
            if status == "completed" and final and not self.pending:
                self.tts.speak(final)
            if self.pending:
                self.tts.stop()
                prompt, session_id = self.pending.popleft()
                self._start_turn(prompt, session_id)
            return
        if method == "error":
            log(f"app-server notification: {params}")

    def run(self) -> int:
        try:
            self.initialize()
            while True:
                if (
                    self.args.exit_after_turns
                    and self.terminal_turns >= self.args.exit_after_turns
                    and self.active_turn_id is None
                    and not self.pending
                ):
                    return 0
                try:
                    source, payload = self.events.get(timeout=0.25)
                except queue.Empty:
                    continue
                if source == "bus":
                    self._handle_bus(payload)
                elif source == "codex":
                    self._handle_codex(payload)
                elif source == "system":
                    raise AppServerError(
                        f"helper exited kind={payload.get('kind')} code={payload.get('code')}"
                    )
        except KeyboardInterrupt:
            log("stopping")
            if self.active_turn_id is not None:
                self._interrupt_active("bridge_shutdown")
            return 130
        finally:
            self.tts.stop()
            if self.demux is not None:
                self.demux.close()
            self.codex.close()


def default_bus_path() -> Path:
    explicit = os.environ.get("CODESCRIBE_TRANSCRIPT_BUS_PATH", "").strip()
    if explicit:
        return Path(os.path.expanduser(explicit))
    xdg = os.environ.get("XDG_STATE_HOME", "").strip()
    if xdg:
        return Path(os.path.expanduser(xdg)) / "codescribe" / "transcript-events.jsonl"
    data = os.environ.get("CODESCRIBE_DATA_DIR", "").strip()
    root = Path(os.path.expanduser(data)) if data else Path.home() / ".codescribe"
    return root / "transcript-events.jsonl"


def parse_args() -> argparse.Namespace:
    repo_root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True, help="named mailbox stem, e.g. james")
    parser.add_argument("--cwd", type=Path, default=Path.cwd(), help="Codex workspace root")
    parser.add_argument("--thread-id", help="resume one idle Codex thread; active threads fail closed")
    parser.add_argument("--bus", type=Path, default=default_bus_path())
    parser.add_argument("--demux", type=Path, default=repo_root / "scripts" / "bus-demux.py")
    parser.add_argument("--codex-bin", type=Path, default=Path("codex"))
    parser.add_argument(
        "--sandbox",
        choices=("read-only", "workspace-write"),
        default="workspace-write",
        help="voice bridge never enables danger-full-access",
    )
    parser.add_argument("--ephemeral", action="store_true", help="do not persist a new thread")
    parser.add_argument("--no-tts", action="store_true", help="text only")
    parser.add_argument("--voice", help="macOS say voice; system default when omitted")
    parser.add_argument("--tts-max-chars", type=int, default=1400)
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--exit-after-turns", type=int, default=0, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.cwd = args.cwd.expanduser().resolve()
    args.bus = args.bus.expanduser().resolve()
    args.demux = args.demux.expanduser().resolve()
    if not args.cwd.is_dir():
        parser.error(f"cwd is not a directory: {args.cwd}")
    if not args.bus.exists():
        parser.error(f"transcript bus does not exist: {args.bus}")
    if not args.demux.is_file():
        parser.error(f"bus demux does not exist: {args.demux}")
    if args.tts_max_chars < 100:
        parser.error("--tts-max-chars must be at least 100")
    return args


def main() -> int:
    args = parse_args()
    try:
        return VoiceBridge(args).run()
    except (AppServerError, OSError) as error:
        log(f"fatal: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
