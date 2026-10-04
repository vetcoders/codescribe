"""Hermetic checks of the real shell reference resolver and bench selector."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]
RESOLVER = SCRIPTS / "lib/data-assets.sh"


class DataAssetReferenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codescribe-reference-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.wav = self.root / "recording with spaces.wav"
        self.wav.touch()
        self.short = self.wav.with_name(self.wav.stem + "_human_transcription.txt")
        self.long = self.wav.with_name(
            self.wav.stem + "_codescribe_raw_human_transcription_from_wav.txt"
        )

    def resolve(self, *args):
        return subprocess.run(
            ["bash", str(RESOLVER), "reference", *map(str, args)],
            capture_output=True, text=True, check=False,
        )

    def test_each_name_and_identical_pair(self):
        for reference in (self.short, self.long):
            with self.subTest(reference=reference.name):
                reference.write_text("A human reference.\n")
                result = self.resolve(self.wav)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), str(reference))
                reference.unlink()
        self.short.write_text("same\n")
        self.long.write_text("same\n")
        self.assertEqual(self.resolve(self.wav).returncode, 0)

    def test_conflict_cannot_select_a_reference(self):
        self.short.write_text("one\n")
        self.long.write_text("two\n")
        result = self.resolve(self.wav)
        self.assertEqual(result.returncode, 4)
        self.assertEqual(result.stdout, "")
        self.assertIn("conflicting", result.stderr)

    def test_empty_reference_is_invalid_even_with_another_valid_label(self):
        self.short.write_text(" \t\n")
        self.long.write_text("words\n")
        self.assertEqual(self.resolve(self.wav).returncode, 4)

    def test_missing_input_audio_and_reference(self):
        self.assertEqual(self.resolve().returncode, 2)
        self.assertEqual(self.resolve(self.root / "absent.wav").returncode, 3)
        self.assertEqual(self.resolve(self.wav).returncode, 3)

    def test_repo_selector_uses_reference_resolver_and_propagates_conflict(self):
        # Exercise the production function without loading .env, building or
        # creating output under a real user's configuration directory.
        source = (SCRIPTS / "bench-stt.sh").read_text()
        selector = source.split("select_repo_pairs() {", 1)[1].split(
            "\nstage_fixtures()", 1
        )[0]
        command = (
            '. "$1"\nselected_tsv="$2"\n'
            + "select_repo_pairs() {" + selector + "\nselect_repo_pairs\n"
        )
        names = ["01_no-to-dobra", "02_kubernetes-wymaga-konfiguracji",
                 "03_algorytm-ma-zlozonosc", "04_runda-3-czyli"]
        for name in names:
            (self.root / (name + ".wav")).touch()
            (self.root / (name + "_codescribe_raw_human_transcription_from_wav.txt")).write_text("words\n")
        output = self.root / "selected.tsv"
        env = dict(os.environ, CODESCRIBE_DATA_ASSETS=str(self.root))
        args = ["bash", "-c", command, "test", str(RESOLVER), str(output)]
        result = subprocess.run(args, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(output.read_text().splitlines()), 4)
        (self.root / (names[0] + "_human_transcription.txt")).write_text("different\n")
        result = subprocess.run(args, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 4, result.stderr)


if __name__ == "__main__":
    unittest.main()
