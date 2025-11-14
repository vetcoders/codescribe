import pytest
from fastapi.testclient import TestClient


@pytest.fixture()
def chat_client(monkeypatch):
    import vistascribe.backend as backend

    async def fake_chat(messages, settings=None):
        suffix = messages[-1]["content"] if messages else ""
        return f"echo:{suffix}"

    monkeypatch.setattr(backend, "run_chat_session", fake_chat)
    return TestClient(backend.app)


def test_chat_demo_success(chat_client):
    payload = {"messages": [{"role": "user", "content": "Hello"}]}
    resp = chat_client.post("/demo/chat", json=payload)
    assert resp.status_code == 200
    assert resp.json()["text"].startswith("echo:")


def test_chat_demo_requires_messages(chat_client):
    resp = chat_client.post("/demo/chat", json={"messages": []})
    assert resp.status_code == 422
