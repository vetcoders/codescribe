from pathlib import Path
from unittest import mock

import vistascribe.history as hist


def _set_fake_root(tmp_path: Path, monkeypatch):
    monkeypatch.setattr(hist, "user_data_root", lambda: tmp_path, raising=False)


def test_history_save_and_recent_entries(tmp_path: Path, monkeypatch):
    # Redirect storage to a temp dir
    _set_fake_root(tmp_path, monkeypatch)

    entry = hist.save_entry("Hello World")
    assert entry.path.exists()
    assert entry.preview == "Hello World"

    recents = hist.recent_entries(5)
    assert len(recents) >= 1
    assert recents[0].preview.startswith("Hello")


def test_history_clear_and_open_folder(tmp_path: Path, monkeypatch):
    _set_fake_root(tmp_path, monkeypatch)

    # create two entries
    hist.save_entry("one")
    hist.save_entry("two")
    assert hist.recent_entries(10)

    hist.clear_history()
    assert hist.recent_entries(10) == []

    # open_history_folder should not raise; mock subprocess
    with mock.patch("subprocess.run") as run:
        hist.open_history_folder()
        run.assert_called()
