#!/usr/bin/env python3
"""
get_models.py

Helper script to download local models after cloning the repo.
- Downloads MLX Whisper (choose large-v3-turbo or medium) into ./models/
- Optionally downloads one or more LLMs for formatting (optional feature)

Usage examples:
  uv run python scripts/get_models.py                          # Downloads large-v3-turbo-q8 (default)
  uv run python scripts/get_models.py --whisper medium-q8      # Q8 quantized medium
  uv run python scripts/get_models.py --whisper all            # Both large-v3-turbo-q8 and medium-q8
  uv run python scripts/get_models.py --whisper large-v3-turbo # Full precision (mlx-community)
  uv run python scripts/get_models.py --llm speakleash/Bielik-4.5B-v3.0-Instruct-mlx

Notes:
- Uses huggingface_hub.snapshot_download under the hood.
- On macOS, MLX tooling can be picky about uppercase in absolute paths (e.g., '/Users').
  For runtime, prefer lowercase '/users' in env vars if that path is available on your system.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
from pathlib import Path

from huggingface_hub import snapshot_download

# Known MLX Whisper repos (canonical names only — aliases defined below)
# Q8 quantized variants from LibraxisAI (recommended: best quality/size ratio)
# Full-precision variants from mlx-community
WHISPER_REPOS = {
    # Q8 quantized (LibraxisAI) - recommended for production
    "small-q8": "LibraxisAI/whisper-small-mlx-q8",
    "medium-q8": "LibraxisAI/whisper-medium-mlx-q8",
    "large-v3-q8": "LibraxisAI/whisper-large-v3-mlx-q8",
    "large-v3-turbo-q8": "LibraxisAI/whisper-large-v3-turbo-mlx-q8",
    "large-v3-q4": "LibraxisAI/whisper-large-v3-q4",
    # Full precision (mlx-community)
    "tiny": "mlx-community/whisper-tiny-mlx",
    "base": "mlx-community/whisper-base-mlx",
    "small": "mlx-community/whisper-small-mlx",
    "medium": "mlx-community/whisper-medium-mlx",
    "large-v3": "mlx-community/whisper-large-v3-mlx",
    "large-v3-turbo": "mlx-community/whisper-large-v3-turbo",
}

WHISPER_ALIASES = {
    "tiny-mlx": "tiny",
    "base-mlx": "base",
    "small-mlx": "small",
    "medium-mlx": "medium",
    "large-v3-mlx": "large-v3",
    # Q8 aliases
    "q8": "large-v3-turbo-q8",
    "turbo-q8": "large-v3-turbo-q8",
    "turbo": "large-v3-turbo-q8",  # Default turbo now points to Q8
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


def _read_env_token() -> str | None:
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


def _git_available() -> bool:
    return shutil.which("git") is not None and shutil.which("git-lfs") is not None


def _clone_with_git(repo_id: str, out: Path) -> None:
    url = f"https://huggingface.co/{repo_id}"
    out.parent.mkdir(parents=True, exist_ok=True)
    print(f"⎇ git clone {url} → {out}")
    subprocess.run(["git", "clone", "--depth", "1", url, str(out)], check=True)
    try:
        subprocess.run(["git", "lfs", "install", "--local"], cwd=str(out), check=False)
        subprocess.run(["git", "lfs", "pull"], cwd=str(out), check=True)
    except FileNotFoundError as e:
        raise RuntimeError(
            "git-lfs not found; install it (e.g., 'brew install git-lfs') and retry"
        ) from e


def download_repo(
    repo_id: str,
    dest_dir: Path,
    target_name: str | None = None,
    token: str | None = None,
    method: str = "auto",
) -> Path:
    ensure_dir(dest_dir)
    # Create a stable local folder name from the repo id unless explicit name is given
    base = target_name or repo_id.rstrip("/").split("/")[-1]
    out = dest_dir / base
    if out.exists() and any(out.iterdir()):
        print(f"✔ Model already present: {out}")
        return out
    print(f"⬇ Downloading {repo_id} → {out} …")
    prefer_git = method == "git"
    prefer_hf = method == "hf"
    last_err: Exception | None = None
    if not prefer_git:
        try:
            snapshot_download(
                repo_id=repo_id,
                local_dir=str(out),
                local_dir_use_symlinks=False,
                token=token or _read_env_token(),
                resume_download=True,
            )
        except Exception as e:
            last_err = e
            if prefer_hf:
                print("[!] huggingface_hub failed and --method=hf was used; aborting.")
                raise
            print("[i] Falling back to git clone (tokenless, requires git-lfs)…")
        else:
            print(f"✔ Downloaded to: {out}")
            return out
    if not _git_available():
        msg = "git-lfs not found. Install it (e.g., 'brew install git-lfs'), then retry, or set HF_TOKEN and use hub downloader."
        if last_err:
            raise RuntimeError(f"{msg}\nOriginal error: {last_err}")
        raise RuntimeError(msg)
    _clone_with_git(repo_id, out)
    print(f"✔ Downloaded to: {out}")
    return out


def download_whisper(
    which: str, dest_dir: Path, *, method: str = "auto", token: str | None = None
) -> list[Path]:
    which = which.lower()
    paths: list[Path] = []
    if which == "none":
        return paths
    if which == "all":
        targets = ["large-v3-turbo-q8", "medium-q8"]
    else:
        canonical = WHISPER_ALIASES.get(which, which)
        if canonical not in WHISPER_REPOS:
            raise SystemExit(
                f"Unknown whisper variant: {which}. Choose from {list(WHISPER_REPOS)} or 'all'/'none'."
            )
        targets = [canonical]
    for t in targets:
        repo = WHISPER_REPOS[t]
        local = download_repo(
            repo, dest_dir, target_name=f"whisper-{t}", token=token, method=method
        )
        paths.append(local)
    return paths


def _parse_whisper_choice(value: str) -> str:
    val = (value or "").strip().lower()
    if val in {"all", "none"}:
        return val
    val = WHISPER_ALIASES.get(val, val)
    if val not in WHISPER_REPOS:
        raise argparse.ArgumentTypeError(
            f"Unknown Whisper variant '{value}'. Choose one of {', '.join(sorted(WHISPER_REPOS))} or 'all'/'none'."
        )
    return val


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--whisper",
        default="large-v3-turbo-q8",
        type=_parse_whisper_choice,
        metavar="VARIANT",
        help=(
            "Which Whisper variant(s) to download. Options: "
            + ", ".join(sorted(WHISPER_REPOS))
            + ", or 'all'/'none'. Q8 variants from LibraxisAI recommended (best quality/size ratio)."
        ),
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
    parser.add_argument(
        "--method",
        choices=["auto", "hf", "git"],
        default="auto",
        help="Download method: huggingface_hub (hf), git clone with LFS (git), or auto (default).",
    )
    args = parser.parse_args()
    # Ensure default value also passes through the same normalization logic
    args.whisper = _parse_whisper_choice(args.whisper)

    repo_root = Path(__file__).resolve().parents[1]
    models_dir = (repo_root / args.models_dir).resolve()
    ensure_dir(models_dir)

    print(f"Models directory: {models_dir}")

    # Whisper
    whisper_paths = download_whisper(
        args.whisper, models_dir, method=args.method, token=args.hf_token
    )

    # LLMs (optional)
    llm_paths: list[Path] = []
    for repo_id in args.llm:
        p = download_repo(repo_id, models_dir, token=args.hf_token, method=args.method)
        llm_paths.append(p)

    # Print helpful env configuration
    print("\nNext steps (example environment):")
    # Prefer Q8 variants, then full precision
    whisper_env: Path | None = None
    for candidate in [
        models_dir / "whisper-large-v3-turbo-q8",
        models_dir / "whisper-medium-q8",
        models_dir / "whisper-large-v3-turbo",
        models_dir / "whisper-medium",
    ]:
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
