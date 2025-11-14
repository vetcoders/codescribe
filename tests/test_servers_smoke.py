import pytest
from fastapi.testclient import TestClient


def _health(client):
    r = client.get("/healthz")
    assert r.status_code == 200
    data = r.json()
    assert "ok" in data


@pytest.mark.filterwarnings("ignore::DeprecationWarning")
def test_backend_healthz_smoke(monkeypatch):
    # Import backend app; on import, it may attempt to load models and fail gracefully
    import importlib

    backend = importlib.import_module("backend")
    # Ensure model variables are None to avoid heavy compute in tests
    monkeypatch.setattr(backend, "whisper_model", None, raising=False)
    client = TestClient(backend.app)
    _health(client)


def test_whisper_server_healthz_smoke(monkeypatch):
    import importlib

    ws = importlib.import_module("vistascribe.whisper_server")
    monkeypatch.setattr(ws, "_whisper_model", None, raising=False)
    client = TestClient(ws.app)
    _health(client)
