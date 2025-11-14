from vistascribe.menu_model import build_models_spec


def titles(spec):
    return [getattr(i, "title", None) if i.kind != "sep" else None for i in spec]


def test_models_spec_local():
    spec = build_models_spec(False, "Medium")
    t = titles(spec)
    # Has current label, actions, and open folder shortcut
    assert t[0] == "Whisper: Medium"
    assert "Use Whisper: Medium" in t
    assert "Use Whisper: Large v3" in t
    assert t[-1] == "Open Models Folder"
    # Contains separators as None
    assert None in t


def test_models_spec_remote_prunes_local():
    spec = build_models_spec(True, "Remote")
    t = titles(spec)
    # Should only contain label, separator, and Open Models Folder
    assert "Use Whisper: Medium" not in t
    assert t[0] == "Whisper: Remote"
    assert t[-1] == "Open Models Folder"
