"""Bootstrap sys.path for src/ and delegate to tools/sitecustomize if present."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SRC = ROOT / "src"
TOOLS_SITE = ROOT / "tools" / "sitecustomize.py"

if SRC.exists():
    src_str = str(SRC)
    if src_str not in sys.path:
        sys.path.insert(0, src_str)

if TOOLS_SITE.exists():
    spec = importlib.util.spec_from_file_location("_vistascribe_tools_sitecustomize", TOOLS_SITE)
    if spec and spec.loader:
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
