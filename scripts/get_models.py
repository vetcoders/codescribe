#!/usr/bin/env python3
"""
get_models.py

Helper script to download local models after cloning the repo.
- Downloads MLX Whisper (choose large-v3-turbo or medium) into ./models/
- Optionally downloads one or more LLMs for formatting (optional feature)

Usage examples:
  uv run python scripts/get_models.py --whisper large-v3-turbo
  uv run python scripts/get_models.py --whisper medium
  uv run python scripts/get_models.py --whisper all --llm mlx-community/Llama-3.2-3B-Instruct-4bit
  uv run python scripts/get_models.py --llm speakleash/Bielik-4.5B-v3.0-Instruct-mlx

Notes:
- Uses huggingface_hub.snapshot_download under the hood.
- On macOS, MLX tooling can be picky about uppercase in absolute paths (e.g., '/Users').
  For runtime, prefer lowercase '/users' in env vars if that path is available on your system.
"""

from __future__ import annotations

import argparse
from pathlib import Path

import os
from typing import Optional

from huggingface_hub import HfHubHTTPError, snapshot_download

# Known MLX Whisper repos
WHISPER_REPOS = {
    "large-v3-turbo": "mlx-community/whisper-large-v3-turbo",
    "large-v3": "mlx-community/whisper-large-v3",
    "medium": "mlx-community/whisper-medium",
}


def ensure_dir(p: Path) -> None:
    p.mkdir(parents=True, exist_ok=True)


def lower_users_path(p: Path) -> Path:
    s = str(p)
    if s.startswith("/Users/"):
        candidate = Path("/users/" + s[len("/Users/") :])
        try:
            if candidate.exists():
                return candidate
        except Exception:
            pass
    return p


def _read_env_token() -> Optional[str]:
    # Prefer HF_TOKEN env; fallback to token in local .env if present
    tok = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")
    if tok:
        return tok.strip()
    # Try reading from repo .env
    try:
        env_path = Path(__file__).resolve().parents[1] / ".env"
        if env_path.exists():
            for line in env_path.read_text(encoding="utf-8").splitlines():
                if line.strip().startswith("HF_TOKEN="):
                    return line.split("=", 1)[1].strip()
    except Exception:
        pass
    return None


def download_repo(repo_id: str, dest_dir: Path, target_name: str | None = None, token: Optional[str] = None) -> Path:
    ensure_dir(dest_dir)
    # Create a stable local folder name from the repo id unless explicit name is given
    base = target_name or repo_id.rstrip("/").split("/")[-1]
    out = dest_dir / base
    if out.exists() and any(out.iterdir()):
        print(f"✔ Model already present: {out}")
        return out
    print(f"⬇ Downloading {repo_id} → {out} …")
    try:
        snapshot_download(
            repo_id=repo_id,
            local_dir=str(out),
            local_dir_use_symlinks=False,
            token=token or _read_env_token(),
            resume_download=True,
        )
    except HfHubHTTPError as e:
        print("[!] Hugging Face download error:", e)
        print("    If this repo is gated or rate-limited, set HF_TOKEN (or pass --hf-token).")
        raise
    print(f"✔ Downloaded to: {out}")
    return out


def download_whisper(which: str, dest_dir: Path) -> list[Path]:
    which = which.lower()
    paths: list[Path] = []
    if which == "none":
        return paths
    if which == "all":
        targets = ["large-v3-turbo", "medium"]
    else:
        if which not in WHISPER_REPOS:
            raise SystemExit(
                f"Unknown whisper variant: {which}. Choose from {list(WHISPER_REPOS)} or 'all'/'none'."
            )
        targets = [which]
    for t in targets:
        repo = WHISPER_REPOS[t]
        local = download_repo(repo, dest_dir, target_name=f"whisper-{t}")
        paths.append(local)
    return paths


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--whisper",
        default="large-v3-turbo",
        choices=["large-v3-turbo", "large-v3", "medium", "all", "none"],
        help="Which Whisper variant(s) to download.",
    )
    parser.add_argument(
        "--llm",
        action="append",
        default=[],
        help="Optional: one or more HF repo IDs for LLMs (e.g., mlx-community/Llama-3.2-3B-Instruct-4bit). Can be repeated.",
    )
    parser.add_argument(
        "--models-dir", default="models", help="Destination models directory (default: ./models)"
    )
    parser.add_argument(
        "--hf-token", default=None, help="Optional Hugging Face token to use for downloads"
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parents[1]
    models_dir = (repo_root / args.models_dir).resolve()
    ensure_dir(models_dir)

    print(f"Models directory: {models_dir}")

    # Whisper
    whisper_paths = download_whisper(args.whisper, models_dir)

    # LLMs (optional)
    llm_paths: list[Path] = []
    for repo_id in args.llm:
        p = download_repo(repo_id, models_dir, token=args.hf_token)
        llm_paths.append(p)

    # Print helpful env configuration
    print("\nNext steps (example environment):")
    # Prefer large if present else medium
    whisper_env: Path | None = None
    for candidate in [models_dir / "whisper-large-v3-turbo", models_dir / "whisper-medium"]:
        if candidate.exists():
            whisper_env = candidate
            break
    if whisper_env is None and whisper_paths:
        whisper_env = whisper_paths[0]

    if whisper_env:
        w = lower_users_path(whisper_env)
        print(f"  export WHISPER_DIR='{w}'  # or set WHISPER_VARIANT=large-v3-turbo|medium")
    else:
        print("  # (No Whisper downloaded; set WHISPER_DIR to your model path when ready)")

    if llm_paths:
        llm = lower_users_path(llm_paths[0])
        print(f"  export LLM_ID='{llm}'     # optional; set FORMAT_ENABLED=0 to disable formatting")
    else:
        print("  # (No LLM downloaded; formatting can be disabled with FORMAT_ENABLED=0)")

    print("\nDone.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
