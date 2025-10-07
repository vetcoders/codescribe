import importlib


def test_load_config_handles_non_string_urls_and_defaults():
    import config as cfg

    importlib.reload(cfg)

    # Provide a custom env mapping with non-string values
    env = {
        "WHISPER_SERVER_URL": None,
        "LLM_SERVER_URL": 123,  # non-string
        # Do not set FORMAT_ENABLED to test default
        "WHISPER_LANGUAGE": 456,  # non-string should become None
    }
    c = cfg.load_config(env)
    assert isinstance(c.whisper_url, str)
    assert c.whisper_url == ""
    assert isinstance(c.llm_url, str)
    assert c.llm_url == ""
    # Default for FORMAT_ENABLED should be disabled (False) for consistency
    assert c.format_enabled is False
    # Language should be None when invalid type provided
    assert c.language is None

    # serialize should not crash and should coerce to strings
    text = cfg.serialize_env(c)
    assert "WHISPER_SERVER_URL=" in text
    assert "LLM_SERVER_URL=" in text
    assert "FORMAT_ENABLED=0" in text
