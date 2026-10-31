#!/usr/bin/env python3
"""Exercise the actual embedded bundle producer without compiling the app."""

import argparse
import json
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "build-app.sh"


class OperatorPackBundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codescribe-pack-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.primary = self.root / "primary"
        self.fallback = self.root / "fallback"
        self.dest = (
            self.root / "Product.app/Contents/Resources/operator-pack/settings.json"
        )
        self.dest.parent.mkdir(parents=True)
        self.dest.write_text(
            json.dumps({"speech": {"engine": {"asr_mode": "cloud"}}, "stale": True})
        )

    def pack(self, root, name, engine, keys=True):
        folder = root / "examples" / name
        folder.mkdir(parents=True)
        if keys:
            (folder / "keys").mkdir()
            (folder / "keys/private-fixture").write_text("must not be bundled")
        payload = {
            "speech": {"engine": engine},
            "private_fixture": "must not be bundled",
        }
        path = folder / "settings.json"
        path.write_text(json.dumps(payload))
        return path

    def bundle(self):
        source = SOURCE.read_text()
        self.assertEqual(source.count("<<'PY_PACK'"), 1)
        body = source.split("<<'PY_PACK'\n", 1)[1].split("\nPY_PACK", 1)[0]
        inputs = {
            p: p.read_bytes()
            for root in [self.primary, self.fallback]
            for p in root.glob("examples/**/*")
            if p.is_file()
        }
        result = subprocess.run(
            [
                sys.executable,
                "-",
                str(self.dest),
                str(self.primary),
                str(self.fallback),
            ],
            input=body,
            text=True,
            capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        for path, original in inputs.items():
            self.assertEqual(path.read_bytes(), original, "Source profile is read-only")
        return json.loads(self.dest.read_text()) if self.dest.exists() else None

    def assertEndpoint(self, result, endpoint):
        self.assertEqual(
            result, {"speech": {"engine": {"cloud_transcription_endpoint": endpoint}}}
        )
        self.assertEqual(
            sorted(
                p.relative_to(self.dest.parent).as_posix()
                for p in self.dest.parent.rglob("*")
                if p.is_file()
            ),
            ["settings.json"],
            "No keys or unrelated settings enter the bundle",
        )

    def test_modes_and_unrelated_settings_never_enter_product(self):
        for mode in ["local_power", "cloud", "apple_only"]:
            with self.subTest(mode=mode):
                self.pack(
                    self.primary,
                    mode,
                    {
                        "asr_mode": mode,
                        "cloud_transcription_endpoint": "https://example.invalid/asr",
                    },
                )
                self.assertEndpoint(self.bundle(), "https://example.invalid/asr")

    def test_no_sources_removes_stale_pack(self):
        self.assertIsNone(self.bundle())
        self.assertFalse(self.dest.exists())

    def test_missing_keys_removes_stale_pack(self):
        self.pack(
            self.primary,
            "profile",
            {
                "asr_mode": "local_power",
                "cloud_transcription_endpoint": "https://example.invalid/asr",
            },
            keys=False,
        )
        self.assertIsNone(self.bundle())

    def test_mode_only_or_empty_endpoint_does_not_create_pack(self):
        for endpoint in [None, "", "  ", 42]:
            with self.subTest(endpoint=endpoint):
                folder = self.primary / "examples/profile"
                if folder.exists():
                    shutil.rmtree(folder)
                self.pack(
                    self.primary,
                    "profile",
                    {"asr_mode": "cloud", "cloud_transcription_endpoint": endpoint},
                )
                self.assertIsNone(self.bundle())

    def test_mode_only_first_pack_does_not_mask_later_endpoint(self):
        self.pack(self.primary, "a-mode-only", {"asr_mode": "local_power"})
        self.pack(
            self.primary,
            "b-endpoint",
            {"cloud_transcription_endpoint": "https://later.invalid/asr"},
        )
        self.assertEndpoint(self.bundle(), "https://later.invalid/asr")

    def test_home_fallback_survives_unusable_primary(self):
        self.pack(self.primary, "first", {"asr_mode": "cloud"})
        self.pack(
            self.fallback,
            "fallback",
            {
                "asr_mode": "local_power",
                "cloud_transcription_endpoint": "https://fallback.invalid/asr",
            },
        )
        self.assertEndpoint(self.bundle(), "https://fallback.invalid/asr")

    def test_primary_root_keeps_precedence_and_second_run_is_identical(self):
        self.pack(
            self.primary,
            "first",
            {"cloud_transcription_endpoint": "https://primary.invalid/asr"},
        )
        self.pack(
            self.fallback,
            "first",
            {"cloud_transcription_endpoint": "https://fallback.invalid/asr"},
        )
        first = self.bundle()
        self.assertEndpoint(first, "https://primary.invalid/asr")
        self.assertEqual(self.bundle(), first)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=SOURCE)
    args, unittest_args = parser.parse_known_args()
    SOURCE = args.source
    unittest.main(argv=[sys.argv[0], *unittest_args])
