import asyncio
import importlib


def test_stt_uses_remote_when_configured(monkeypatch, tmp_path):
    # Configure remote URL and stub HTTP post
    monkeypatch.setenv("WHISPER_SERVER_URL", "http://localhost:9999")

    import stt as stt_mod

    importlib.reload(stt_mod)

    called = {}

    async def fake_http_post(url, files):
        called["url"] = url
        # emulate server response
        return {"text": "remote-transcript"}

    # patch helper used by stt when remote
    monkeypatch.setattr(
        stt_mod,
        "_http_post",
        lambda url, files: asyncio.get_event_loop().run_until_complete(fake_http_post(url, files)),
    )

    # create a dummy wav file path for code that reads it (not strictly required by remote)
    p = tmp_path / "a.wav"
    p.write_bytes(b"RIFF0000WAVEfmt ")

    out = asyncio.run(stt_mod.transcribe(str(p)))
    assert out == "remote-transcript"
    assert called["url"].endswith("/transcribe")


def test_llm_uses_remote_when_configured(monkeypatch):
    monkeypatch.setenv("FORMAT_ENABLED", "1")
    monkeypatch.setenv("LLM_SERVER_URL", "http://localhost:9998")

    import llm as llm_mod

    importlib.reload(llm_mod)

    def fake_http_post(url, json=None):
        assert url.endswith("/format")
        # echo back uppercased for visibility
        return {"text": (json["text"]).upper()}

    monkeypatch.setattr(llm_mod, "_http_post", fake_http_post)

    out = asyncio.run(llm_mod.format_text("ala ma kota"))
    assert out == "ALA MA KOTA"


def test_servers_apps_exist_and_healthz():
    # Import server apps and test basic routes using TestClient
    import importlib

    from fastapi.testclient import TestClient

    whisper_server = importlib.import_module("whisper_server")
    lm_server = importlib.import_module("lm_server")

    wc = TestClient(whisper_server.app)
    lc = TestClient(lm_server.app)

    wr = wc.get("/healthz")
    assert wr.status_code == 200
    assert "ok" in wr.json()

    lr = lc.get("/healthz")
    assert lr.status_code == 200
    assert "ok" in lr.json()

    # format echo
    fr = lc.post("/format", json={"text": "abc"})
    assert fr.status_code == 200
    assert fr.json()["text"] == "abc"


def test_ui_helper_backend_labels():
    # Pure helper must compute menu labels without requiring rumps
    from ui import backend_status_labels

    labels = backend_status_labels(stt_ok=True, llm_ok=False)
    assert "STT: OK" in labels[0]
    assert "LLM: OFF" in labels[1]

    labels2 = backend_status_labels(stt_ok=False, llm_ok=True)
    assert "STT: OFF" in labels2[0]
    assert "LLM: OK" in labels2[1]
