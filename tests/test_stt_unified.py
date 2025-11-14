import pytest

import vistascribe.stt as stt


@pytest.mark.asyncio
async def test_transcribe_remote_success(monkeypatch, tmp_path):
    # Provide a dummy file
    p = tmp_path / "a.wav"
    p.write_bytes(b"RIFFxxxxWAVEfmt ")  # header-like bytes; content not used

    monkeypatch.setattr(stt, "WHISPER_SERVER_URL", "http://localhost:9999")

    def fake_post(url, files):
        return {"text": "hello"}

    monkeypatch.setattr(stt, "_http_post", fake_post)

    res = await stt.transcribe(str(p), mime="audio/wav", lang="pl")
    assert isinstance(res, dict)
    assert res.get("ok") is True
    assert res.get("text") == "hello"


@pytest.mark.asyncio
async def test_transcribe_remote_error(monkeypatch, tmp_path):
    p = tmp_path / "b.wav"
    p.write_bytes(b"WAVE")
    monkeypatch.setattr(stt, "WHISPER_SERVER_URL", "http://localhost:9999")

    def boom(url, files):  # noqa: ARG001 - signature matches
        raise RuntimeError("net down")

    monkeypatch.setattr(stt, "_http_post", boom)

    res = await stt.transcribe(str(p))
    assert isinstance(res, dict)
    assert res.get("ok") is False
    assert res.get("code") == "remote"
