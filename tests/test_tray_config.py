import importlib


def test_load_config_from_env(monkeypatch):
    # Prepare environment
    monkeypatch.setenv("WHISPER_SERVER_URL", "http://127.0.0.1:8238")
    monkeypatch.setenv("LLM_SERVER_URL", "http://127.0.0.1:8239")
    monkeypatch.setenv("FORMAT_ENABLED", "1")
    monkeypatch.setenv("WHISPER_LANGUAGE", "pl")

    import config as cfg
    importlib.reload(cfg)

    c = cfg.load_config()
    assert c.whisper_url == "http://127.0.0.1:8238"
    assert c.llm_url == "http://127.0.0.1:8239"
    assert c.format_enabled is True
    assert c.language == "pl"


def test_serialize_env_and_save(tmp_path, monkeypatch):
    import config as cfg
    c = cfg.Config(whisper_url="", llm_url="http://x", format_enabled=False, language=None)

    content = cfg.serialize_env(c)
    assert "FORMAT_ENABLED=0" in content
    assert "LLM_SERVER_URL=http://x" in content
    assert "WHISPER_SERVER_URL=" in content  # empty means local
    assert "WHISPER_LANGUAGE=" in content

    env_path = tmp_path / ".env"
    cfg.save_config(c, path=str(env_path))
    text = env_path.read_text()
    assert text == content


def test_ui_config_labels(monkeypatch):
    import config as cfg
    import ui as ui_mod
    # ensure we can import without macOS frameworks by not touching UI classes

    c = cfg.Config(whisper_url="", llm_url="", format_enabled=True, language=None)
    labels = ui_mod.config_labels(c)
    assert "Language: auto" in labels[0]
    assert "Formatting: ON" in labels[1]
    assert "Whisper URL: local" in labels[2]
    assert "LLM URL: local" in labels[3]

    c2 = cfg.Config(whisper_url="http://a", llm_url="http://b", format_enabled=False, language="en")
    labels2 = ui_mod.config_labels(c2)
    assert labels2[0].endswith("en")
    assert labels2[1].endswith("OFF")
    assert labels2[2].endswith("http://a")
    assert labels2[3].endswith("http://b")
