import importlib


def test_ui_help_message_is_english():
    import ui as ui_mod

    importlib.reload(ui_mod)
    msg = ui_mod.toggles_help_message("en")
    assert "Whisper automatically detects the language" in msg
    # Ensure no common Polish words from previous message are present
    assert "Wymusza" not in msg
    assert "domyślnie" not in msg
    assert "Whisper wykrywa" not in msg
