#!/usr/bin/env python3
"""Hermetic JSONL mock for scripts/codex-voice-bridge.py."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


LOG = Path(os.environ["MOCK_CODEX_LOG"])
THREAD_ID = "01999999-0000-7000-8000-000000000001"
active_turn: str | None = None
turn_counter = 0
thread_read_counter = 0


def emit(message: dict) -> None:
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def log(message: dict) -> None:
    with LOG.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(message, separators=(",", ":")) + "\n")


def complete(turn_id: str, prompt: str) -> None:
    text = f"MOCK_FINAL: {prompt}"
    emit(
        {
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": THREAD_ID,
                "turnId": turn_id,
                "itemId": f"item-{turn_id}",
                "delta": text,
            },
        }
    )
    emit(
        {
            "method": "item/completed",
            "params": {
                "threadId": THREAD_ID,
                "turnId": turn_id,
                "completedAtMs": 1,
                "item": {
                    "type": "agentMessage",
                    "id": f"item-{turn_id}",
                    "text": text,
                    "phase": "final_answer",
                    "memoryCitation": None,
                },
            },
        }
    )
    emit(
        {
            "method": "turn/completed",
            "params": {
                "threadId": THREAD_ID,
                "turn": {"id": turn_id, "status": "completed", "items": []},
            },
        }
    )


for raw in sys.stdin:
    message = json.loads(raw)
    log(message)
    method = message.get("method")
    request_id = message.get("id")
    if request_id is None:
        continue
    if method == "initialize":
        emit({"id": request_id, "result": {"userAgent": "mock"}})
    elif method == "thread/start":
        emit(
            {
                "id": request_id,
                "result": {
                    "thread": {
                        "id": THREAD_ID,
                        "status": {"type": "idle"},
                    },
                    "cwd": message["params"]["cwd"],
                },
            }
        )
    elif method == "thread/read":
        thread_read_counter += 1
        turns = [{"id": "stored-turn-1", "status": "completed", "items": []}]
        handoff_after = int(os.environ.get("MOCK_HANDOFF_AFTER_READS", "0"))
        if handoff_after and thread_read_counter >= handoff_after:
            turns.append(
                {"id": "desktop-turn-2", "status": "completed", "items": []}
            )
        emit(
            {
                "id": request_id,
                "result": {
                    "thread": {
                        "id": message["params"]["threadId"],
                        "status": {"type": "notLoaded"},
                        "turns": turns,
                    }
                },
            }
        )
    elif method == "thread/resume":
        status = "active" if os.environ.get("MOCK_THREAD_ACTIVE") == "1" else "idle"
        emit(
            {
                "id": request_id,
                "result": {
                    "thread": {
                        "id": message["params"]["threadId"],
                        "status": {"type": status, "activeFlags": []}
                        if status == "active"
                        else {"type": status},
                    }
                },
            }
        )
    elif method == "thread/name/set":
        emit({"id": request_id, "result": {}})
    elif method == "turn/start":
        turn_counter += 1
        active_turn = f"turn-{turn_counter}"
        prompt = message["params"]["input"][0]["text"]
        emit(
            {
                "id": request_id,
                "result": {"turn": {"id": active_turn, "status": "inProgress", "items": []}},
            }
        )
        if "hold" not in prompt.casefold():
            complete(active_turn, prompt)
    elif method == "turn/interrupt":
        turn_id = message["params"]["turnId"]
        emit({"id": request_id, "result": {}})
        emit(
            {
                "method": "turn/completed",
                "params": {
                    "threadId": THREAD_ID,
                    "turn": {"id": turn_id, "status": "interrupted", "items": []},
                },
            }
        )
        active_turn = None
    else:
        emit({"id": request_id, "result": {}})
