#!/usr/bin/env python3
"""Kielbasa demux for the clean transcript bus.

Does not open a microphone. Reads `codescribe.transcript.v1` NDJSON.
Unnamed agents do not pass. Name filter is a whole-word stem plus Polish cases.

  python3 scripts/bus-demux.py --become --follow
  python3 scripts/bus-demux.py --name james --follow
  python3 scripts/bus-demux.py --name james --once --bus /tmp/bus.jsonl
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path
from typing import Any, Iterator

BUS_FILENAME = "transcript-events.jsonl"
SEALED = "transcript_sealed"
ASSIGN_RE = re.compile(
    r"(?i)(?:będziesz(?:\s+od)?\s+teraz|nazywam\s+cię|nazywasz\s+się|"
    r"you(?:['’]re|\s+are)|cześć|hello)\s+([A-Za-zĄĆĘŁŃÓŚŹŻąćęłńóśźż]{2,32})"
)


def bus_path() -> Path:
    for key in ("CODESCRIBE_TRANSCRIPT_BUS_PATH", "CODESCRIBE_TRANSCRIPT_BUS"):
        raw = os.environ.get(key, "").strip()
        if raw:
            return Path(os.path.expanduser(raw))
    xdg = os.environ.get("XDG_STATE_HOME", "").strip()
    if xdg:
        return Path(os.path.expanduser(xdg)) / "codescribe" / BUS_FILENAME
    return Path.home() / ".codescribe" / BUS_FILENAME


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


def slim(event: dict[str, Any], audience: str, kind: str = "seal") -> dict[str, Any]:
    return {
        "audience": audience,
        "kind": kind,
        "status": event.get("status"),
        "session_id": event.get("session_id"),
        "emitted_at": event.get("emitted_at"),
        "mode": event.get("mode"),
        "text": event.get("text") or "",
    }


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
    if event.get("schema") not in (None, "codescribe.transcript.v1"):
        return None
    return event


def emit(payload: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
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
    if status != SEALED and not (drafts and status in ("utterance_draft", "utterance_revised")):
        return None
    text = event.get("text") or ""
    claimed = assigned_name(text)
    if claimed:
        payload = slim(event, claimed, kind="name_assignment")
        payload["name"] = claimed
        if hear_all or (name and claimed == name.casefold()) or addressed_to(text, name or ""):
            return payload
        if debug:
            sys.stderr.write(f"bus-demux: drop assignment name={claimed}\n")
        return None
    if hear_all:
        return slim(event, "*")
    if name and addressed_to(text, name):
        return slim(event, name.casefold())
    if debug and status == SEALED:
        sys.stderr.write("bus-demux: drop unnamed-or-other seal\n")
    return None


def iter_new_lines(path: Path, offset: int) -> tuple[list[str], int]:
    try:
        size = path.stat().st_size
    except FileNotFoundError:
        return [], offset
    if size < offset:
        offset = 0
    with path.open("rb") as handle:
        handle.seek(offset)
        chunk = handle.read()
    offset += len(chunk)
    text = chunk.decode("utf-8", errors="replace")
    if not text:
        return [], offset
    lines = text.splitlines()
    if not text.endswith("\n"):
        # incomplete last line: rewind to its start
        incomplete = lines.pop().encode("utf-8")
        offset -= len(incomplete)
    return lines, offset


def replay(path: Path) -> Iterator[str]:
    try:
        raw = path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return
        yield from ()  # pragma: no cover — keeps the generator type
    for line in raw.splitlines():
        yield line


def run(args: argparse.Namespace) -> int:
    path: Path = args.bus
    name: str | None = args.name.casefold() if args.name else None
    hear_all = bool(args.all or args.become)
    if not name and not hear_all:
        sys.stderr.write("bus-demux: unnamed agent does not pass; pass --name or --become/--all\n")
        return 2

    def handle(raw: str) -> None:
        nonlocal name, hear_all
        event = parse_line(raw)
        if event is None:
            return
        payload = consider(
            event,
            name=name,
            hear_all=hear_all,
            drafts=args.drafts,
            debug=args.debug,
        )
        if payload is None:
            return
        if args.become and payload.get("kind") == "name_assignment" and not name:
            name = str(payload["name"])
            hear_all = False
            sys.stderr.write(f"bus-demux: bound name={name}\n")
        emit(payload)

    if args.once:
        last = None
        for raw in replay(path):
            event = parse_line(raw)
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
        emit(last)
        return 0

    offset = 0
    if args.follow and not args.from_start:
        try:
            offset = path.stat().st_size
        except FileNotFoundError:
            offset = 0
    elif args.from_start:
        for raw in replay(path):
            handle(raw)

    sys.stderr.write(
        f"bus-demux: bus={path} name={name or '*'} follow={int(args.follow)}\n"
    )
    while True:
        lines, offset = iter_new_lines(path, offset)
        for raw in lines:
            handle(raw)
        if not args.follow:
            return 0
        time.sleep(args.interval)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bus", type=Path, default=None, help="override bus path")
    parser.add_argument("--name", default=None, help="bound agent name (kielbasa filter)")
    parser.add_argument("--all", action="store_true", help="promiscuous: every seal")
    parser.add_argument(
        "--become",
        action="store_true",
        help="hear all until a name assignment, then filter",
    )
    parser.add_argument("--follow", action="store_true", help="tail the bus")
    parser.add_argument("--once", action="store_true", help="print last matching seal and exit")
    parser.add_argument("--from-start", action="store_true", help="replay existing lines first")
    parser.add_argument("--drafts", action="store_true", help="also emit draft/revised (noisy)")
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--interval", type=float, default=0.15)
    args = parser.parse_args()
    if args.bus is None:
        args.bus = bus_path()
    if args.once and args.follow:
        parser.error("--once and --follow cannot combine")
    if not args.once and not args.follow and not args.from_start:
        args.follow = True
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
