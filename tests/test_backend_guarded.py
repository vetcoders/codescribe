import pytest
from fastapi.testclient import TestClient


@pytest.fixture()
def app_client(monkeypatch):
    # Import here so env-based flags are computed first; then override directly
    import vistascribe.backend as backend

    # Ensure guarded endpoints are disabled by default for this test
    monkeypatch.setattr(backend, "_ACTIONS_ENABLED", False, raising=False)
    monkeypatch.setattr(backend, "_EVENTS_ENABLED", False, raising=False)

    return TestClient(backend.app)


def test_health_and_version(app_client):
    r = app_client.get("/healthz")
    assert r.status_code == 200
    j = r.json()
    assert "ok" in j
    assert "state" in j

    r2 = app_client.get("/version")
    assert r2.status_code == 200
    j2 = r2.json()
    assert "mlx" in j2 and isinstance(j2["mlx"], dict)
    assert "ready" in j2 and isinstance(j2["ready"], dict)


def test_guarded_endpoints_disabled_by_default(app_client):
    # When guards are false, optional endpoints should respond with 404
    r = app_client.post("/action", json={"action": "activate"})
    assert r.status_code == 404
    j = r.json()
    assert j.get("error") == "/action disabled"

    r2 = app_client.get("/events")
    assert r2.status_code == 404
    j2 = r2.json()
    assert j2.get("error") == "/events disabled"
