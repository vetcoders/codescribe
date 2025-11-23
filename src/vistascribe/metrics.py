"""
metrics.py — Developer metrics for VistaScribe

Writes baseline metrics to logs/metrics.json:
- SLOC per Python file (non-empty, non-comment lines)
- Simple complexity proxy per file (count of branching keywords)
- Totals and timestamp
- Optional process RSS baseline (MB)
- Dead-code deletion proposals for selected legacy files

Usage:
  python metrics.py              # compute code metrics only
  python metrics.py --rss        # include current process RSS

Notes:
- This does not import heavy app modules; it only scans files as text.
- RSS is measured for the current process. To baseline backend/tray, run it
  inside those processes or export METRICS_CONTEXT environment variable.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import sys
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path

from .path_utils import repo_root

logger = logging.getLogger(__name__)

REPO_ROOT = repo_root()
LOGS_DIR = REPO_ROOT / "logs"
METRICS_FILE = LOGS_DIR / "metrics.json"

BRANCH_TOKENS = {
    "if",
    "elif",
    "else",
    "for",
    "while",
    "try",
    "except",
    "with",
    "and",
    "or",
    "case",
    "match",
}


@dataclass
class FileMetric:
    path: str
    sloc: int
    complexity: int


@dataclass
class Metrics:
    generated_at: str
    context: str | None
    total_files: int
    total_sloc: int
    total_complexity: int
    rss_mb: float | None
    files: list[FileMetric]
    deletion_proposals: dict[str, str]


def _iter_py_files(root: Path) -> list[Path]:
    ignore_dirs = {
        ".git",
        "dist",
        "build",
        "__pycache__",
        ".venv",
        "venv",
        "env",
        "packaging",
        "models",
        "extracted_frames",
        "VistaScribe.app",
    }
    out: list[Path] = []
    for p in root.rglob("*.py"):
        parts = set(p.parts)
        if parts & ignore_dirs:
            continue
        out.append(p)
    return out


def _compute_sloc_and_complexity(text: str) -> tuple[int, int]:
    sloc = 0
    complexity = 0
    for line in text.splitlines():
        s = line.strip()
        if not s:
            continue
        if s.startswith("#"):
            continue
        sloc += 1
        # very light complexity proxy: count branch tokens
        lw = re.findall(r"[A-Za-z_]+", s)
        complexity += sum(1 for w in lw if w in BRANCH_TOKENS)
    return sloc, complexity


def _get_rss_mb() -> float | None:
    try:
        import psutil  # type: ignore

        proc = psutil.Process()
        return proc.memory_info().rss / (1024 * 1024)
    except Exception:
        try:
            import resource  # type: ignore

            rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
            # macOS reports bytes, Linux kB; normalize best-effort
            if rss > 10_000_000:  # looks like bytes
                return rss / (1024 * 1024)
            return rss / 1024.0
        except Exception:
            return None


def _propose_deletions(root: Path) -> dict[str, str]:
    """Heuristic keep/delete proposals for legacy files based on references.

    Returns mapping of filename -> 'keep' | 'delete?'
    """
    candidates = [
        "lm_server.py",
        "whisper_server.py",
        "chatclient.py",
        "first_run.py",
    ]
    # Build a simple reference index: filename substring occurrences in other files
    text_index: dict[str, str] = {}
    for p in _iter_py_files(root):
        try:
            text_index[str(p.relative_to(root))] = p.read_text(encoding="utf-8", errors="ignore")
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)
    proposals: dict[str, str] = {}
    for name in candidates:
        target = root / name
        if not target.exists():
            continue
        referenced = False
        for other, txt in text_index.items():
            if other.endswith(name):
                continue
            if re.search(rf"\b{name}\b|\b{Path(name).stem}\b", txt):
                referenced = True
                break
        # crude heuristic: if referenced, keep; if not, tentatively delete
        proposals[name] = "keep" if referenced else "delete?"
    return proposals


def generate_metrics(include_rss: bool) -> Metrics:
    files: list[FileMetric] = []
    total_sloc = 0
    total_cplx = 0
    for p in _iter_py_files(REPO_ROOT):
        try:
            text = p.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            continue
        sloc, cplx = _compute_sloc_and_complexity(text)
        files.append(FileMetric(path=str(p.relative_to(REPO_ROOT)), sloc=sloc, complexity=cplx))
        total_sloc += sloc
        total_cplx += cplx

    rss = _get_rss_mb() if include_rss else None
    ctx = os.environ.get("METRICS_CONTEXT")
    deletion = _propose_deletions(REPO_ROOT)
    return Metrics(
        generated_at=datetime.utcnow().isoformat(),
        context=ctx,
        total_files=len(files),
        total_sloc=total_sloc,
        total_complexity=total_cplx,
        rss_mb=rss,
        files=files,
        deletion_proposals=deletion,
    )


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rss", action="store_true", help="include current process RSS usage in MB")
    args = ap.parse_args(argv)

    LOGS_DIR.mkdir(parents=True, exist_ok=True)
    m = generate_metrics(include_rss=args.rss)
    out = {
        **asdict(m),
        "files": [asdict(f) for f in m.files],
    }
    METRICS_FILE.write_text(json.dumps(out, indent=2), encoding="utf-8")
    print(f"Wrote {METRICS_FILE}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
