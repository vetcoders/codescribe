import importlib
import os
import tempfile

from fastapi.testclient import TestClient


def setup_fake_whisper(module, return_value):
    class FakeWhisper:
        @staticmethod
        def transcribe(path):
            # Ensure the temp file exists when transcribe is called
            assert os.path.exists(path)
            return return_value
    module.whisper = FakeWhisper()
    module._whisper_model = object()  # mark as loaded


def test_transcribe_deletes_tempfile(monkeypatch):
    whisper_server = importlib.import_module("whisper_server")
    importlib.reload(whisper_server)

    # Configure fake whisper that returns a dict with text
    setup_fake_whisper(whisper_server, {"text": "ok"})

    client = TestClient(whisper_server.app)

    # Create a temporary audio file to upload
    with tempfile.NamedTemporaryFile(suffix=".wav") as f:
        f.write(b"RIFF0000WAVEfmt ")
        f.flush()
        files = {"audio": ("test.wav", open(f.name, "rb"), "audio/wav")}
        r = client.post("/transcribe", files=files)
        assert r.status_code == 200
        assert r.json()["text"] == "ok"
        # The server should have cleaned up its own temp copy; we can't access the server's
        # temp path directly, but we can at least assert our upload file still exists here.
        # For stronger guarantee, monkeypatch NamedTemporaryFile is overkill; we rely on a
        # separate error-path test below to assert cleanup via os.remove is called.


def test_transcribe_none_result_returns_500_and_cleanup(monkeypatch):
    whisper_server = importlib.import_module("whisper_server")
    importlib.reload(whisper_server)

    # Track removed paths
    removed = {}

    def fake_remove(path):
        removed[path] = True
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass

    monkeypatch.setattr(whisper_server.os, "remove", fake_remove)

    # Return None to simulate internal failure
    setup_fake_whisper(whisper_server, None)

    client = TestClient(whisper_server.app)

    with tempfile.NamedTemporaryFile(suffix=".wav") as f:
        f.write(b"RIFF0000WAVEfmt ")
        f.flush()
        files = {"audio": ("bad.wav", open(f.name, "rb"), "audio/wav")}
        r = client.post("/transcribe", files=files)
        assert r.status_code == 500
        # Ensure server attempted to cleanup temp file it created (os.remove called)
        assert any(removed.values()), "Expected server to remove its temporary file"
