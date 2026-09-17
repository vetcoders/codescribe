"""Hermetic tests for scripts/truth-regress.sh; never touches ~/.codescribe."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "truth-regress.sh"

STUB_SIDECAR = """{
  "source": "local_final_pass",
  "engine": "whisper",
  "mode": "raw",
  "fallback_class": null,
  "fallback_used": false,
  "vad_speech_pct": 50.0,
  "no_speech_reason": null,
  "avg_logprob": -0.2,
  "confidence_flags": [],
  "sparkline": "░███",
  "commit_trigger": null,
  "display_status": "CLI • Transcript",
  "engine_mode": "embedded_default",
  "fine_sparkline": "▁▃▅",
  "energy_sparkline": "▂▅█"
}
"""

FAKE_CODESCRIBE = r"""#!/usr/bin/env bash
set -euo pipefail
input=""
for arg in "$@"; do
  case "$arg" in
    --|transcribe|--raw|--no-bus) ;;
    --*) ;;
    *) input=$arg ;;
  esac
done
if [[ -z "$input" ]]; then
  printf 'usage: codescribe transcribe <file>\n' >&2
  exit 2
fi
cat >"${input}.truth.json" <<'EOF'
%s
EOF
""" % STUB_SIDECAR

FAKE_QUBE_REPORT = r"""#!/usr/bin/env bash
set -euo pipefail
baseline=""
candidate=""
out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --baseline-dir)
      baseline=$2
      shift 2
      ;;
    --candidate-dir)
      candidate=$2
      shift 2
      ;;
    --out|--output)
      out=$2
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [[ -z "$candidate" ]]; then
  printf 'missing --candidate-dir\n' >&2
  exit 2
fi
if [[ -z "$out" ]]; then
  out=$candidate
fi
mkdir -p -- "$out"
python3 - "$candidate" "$out/comparison.json" <<'PY'
import json, sys
from pathlib import Path
cand = Path(sys.argv[1])
out = Path(sys.argv[2])
stems = sorted(p.name[: -len(".truth.json")] for p in cand.glob("*.truth.json"))
rows = [{"stem": stem, "delta_avg_logprob": 0.1} for stem in stems]
payload = {
    "rows": rows,
    "summary": {
        "paired": len(rows),
        "median_delta_avg_logprob": 0.1 if rows else None,
        "fallback_flips": 0,
    },
}
out.write_text(json.dumps(payload, indent=2) + "\n")
print("comparison:", out)
PY
"""


class TruthRegressTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.day = self.root / "2026-07-12"
        self.day.mkdir()
        self.candidate = self.root / "candidate"
        self.bindir = self.root / "bin"
        self.bindir.mkdir()

    def write_pair(self, stem: str, ext: str = ".m4a") -> None:
        audio = self.day / f"{stem}_raw{ext}"
        audio.write_bytes(b"not-audio")
        sidecar = self.day / f"{stem}_raw.txt.truth.json"
        sidecar.write_text(STUB_SIDECAR)
        txt = self.day / f"{stem}_raw.txt"
        txt.write_text("hello hello world\n")

    def install_fakes(self) -> None:
        codescribe = self.bindir / "codescribe"
        codescribe.write_text(FAKE_CODESCRIBE)
        codescribe.chmod(codescribe.stat().st_mode | stat.S_IXUSR)
        qube = self.bindir / "qube-report"
        qube.write_text(FAKE_QUBE_REPORT)
        qube.chmod(qube.stat().st_mode | stat.S_IXUSR)

    def run_script(self, *args: str, extra_env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
        env = os.environ.copy()
        env["PATH"] = f"{self.bindir}{os.pathsep}{env.get('PATH', '')}"
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            capture_output=True,
            text=True,
            check=False,
            env=env,
        )

    def test_dry_run_lists_pairs_and_writes_nothing(self):
        self.write_pair("alpha")
        self.write_pair("bravo")
        self.write_pair("charlie", ext=".wav")
        result = self.run_script("-n", str(self.day), "2", str(self.candidate))
        self.assertEqual(result.returncode, 0, result.stderr)
        pair_lines = [line for line in result.stdout.splitlines() if line.startswith("PAIR\t")]
        self.assertEqual(len(pair_lines), 2)
        self.assertIn("dry-run: 2 pair(s); wrote nothing", result.stdout)
        self.assertFalse(self.candidate.exists())
        self.assertEqual(list(self.day.glob("*_raw.m4a.truth.json")), [])

    def test_full_pass_with_fake_codescribe_writes_comparison(self):
        self.write_pair("alpha")
        self.install_fakes()
        result = self.run_script(str(self.day), "1", str(self.candidate))
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        comparison_path = self.candidate / "comparison.json"
        self.assertTrue(comparison_path.is_file())
        payload = json.loads(comparison_path.read_text())
        self.assertEqual(len(payload["rows"]), 1)
        sidecar = self.candidate / "alpha_raw.m4a.truth.json"
        self.assertTrue(sidecar.is_file())
        link = self.candidate / "alpha_raw.m4a"
        self.assertTrue(link.is_symlink())
        self.assertEqual(list(self.day.glob("*.m4a.truth.json")), [])


if __name__ == "__main__":
    unittest.main()
