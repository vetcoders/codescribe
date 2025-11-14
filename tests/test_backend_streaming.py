import base64
import json

import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

import vistascribe.backend as backend


def _client(monkeypatch) -> TestClient:
    monkeypatch.setattr(backend, "whisper_model", object(), raising=False)
    return TestClient(backend.app)


def test_stream_transcribe_endpoint_ndjson(monkeypatch):
    client = _client(monkeypatch)
    captured: list[bytes] = []

    async def fake_transcribe(session):
        captured.append(bytes(session.buffer))
        session.buffer.clear()
        return {"text": f"chunk-{len(captured)}", "duration_ms": 111}

    monkeypatch.setattr(backend, "_transcribe_stream_buffer", fake_transcribe)

    payload = "\n".join(
        [
            json.dumps(
                {
                    "type": "chunk",
                    "audio_base64": base64.b64encode(b"hello").decode(),
                    "sample_rate": 16000,
                    "encoding": "pcm16",
                }
            ),
            json.dumps({"type": "flush"}),
            json.dumps({"type": "end"}),
        ]
    )

    resp = client.post(
        "/stream/transcribe",
        data=payload,
        headers={"content-type": "application/x-ndjson"},
    )

    lines = [json.loads(line) for line in resp.text.strip().splitlines()]
    assert lines[0]["type"] == "hello"
    assert any(line["type"] == "transcript.final" for line in lines)
    assert lines[-1]["type"] == "stream.closed"
    assert captured == [b"hello"]


def test_ws_transcribe_roundtrip(monkeypatch):
    client = _client(monkeypatch)

    async def fake_transcribe(session):
        data = bytes(session.buffer)
        session.buffer.clear()
        return {"text": data.decode("latin1"), "duration_ms": 42}

    monkeypatch.setattr(backend, "_transcribe_stream_buffer", fake_transcribe)

    with client.websocket_connect("/ws/transcribe") as ws:
        hello = ws.receive_json()
        assert hello["type"] == "hello"

        ws.send_json(
            {
                "type": "chunk",
                "audio_base64": base64.b64encode(b"abc").decode(),
                "sample_rate": 16000,
                "encoding": "pcm16",
            }
        )
        ack = ws.receive_json()
        assert ack["type"] == "ack"

        ws.send_json({"type": "flush"})
        final = ws.receive_json()
        assert final["type"] == "transcript.final"
        assert final["text"] == "abc"

        ws.send_json({"type": "end"})
        with pytest.raises(WebSocketDisconnect):
            ws.receive_text()
