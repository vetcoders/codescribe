import os

import path_utils


def test_preserve_hf_repo_id_unchanged():
    # Non-absolute identifiers (HF repo IDs) should pass through unchanged
    repo_id = "mlx-community/Llama-3.2-3B-Instruct-4bit"
    assert path_utils.normalize_model_path(repo_id) == repo_id


def test_users_prefix_normalizes_when_lower_variant_exists(monkeypatch):
    # Given an absolute path under /Users, when a lowercase /users variant exists,
    # the function should prefer the lowercase variant.
    src = "/Users/SomeUser/Models/foo"

    # Patch os.path.exists used by the module to pretend the lowercase variant exists
    original_exists = path_utils.os.path.exists

    def fake_exists(p: str) -> bool:
        if p.startswith("/users/SomeUser/Models/foo"):
            return True
        # passthrough
        return original_exists(p)

    monkeypatch.setattr(path_utils.os.path, "exists", fake_exists)

    out = path_utils.normalize_model_path(src)
    assert out.startswith("/users/SomeUser/Models/foo")


def test_home_expands_and_absolutizes(monkeypatch):
    # "~" should be expanded and result should be absolute (not containing '~')
    home = os.path.expanduser("~")
    candidate = "~/models/foo"
    out = path_utils.normalize_model_path(candidate)
    assert out is not None
    assert out.startswith("/")
    assert "~" not in out
    # It should resolve under the user's home dir (case-insensitive check on prefix)
    assert out.lower().startswith(home.lower().rstrip("/") + "/")
