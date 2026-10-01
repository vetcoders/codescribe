"""Hermetic metrics and privacy regressions; no Apple/microphone/private data."""

import importlib.util
import io
import contextlib
from unittest.mock import patch
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import wave

SCRIPT = Path(__file__).resolve().parents[1] / "stt-vocabulary-ab.py"
SPEC = importlib.util.spec_from_file_location("vocabulary_ab", SCRIPT)
AB = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AB)


class VocabularyABTests(unittest.TestCase):
    def test_sample_is_deterministic_and_ignores_symlinks(self):
        import os
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            for i in range(5):
                path = root / f"{i}.wav"
                with wave.open(str(path), "wb") as audio:
                    audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
                    audio.writeframes(b"\x00\x00" * 160000)
                os.utime(path, ns=(1000, 1000))
            (root / "alias.wav").symlink_to(root / "0.wav")
            first, eligible = AB.select_sample(root, 3)
            self.assertEqual(eligible, 5)
            self.assertEqual([entry["name"] for entry in first], ["0.wav", "2.wav", "4.wav"])
            self.assertEqual(AB.select_sample(root, 3), (first, eligible))

    def test_app_destination_refuses_outside_existing_and_symlinks(self):
        with tempfile.TemporaryDirectory() as root:
            private = Path(root) / ".codescribe"
            private.mkdir()
            output = private / "lab/vocabulary-ab/result.json"
            AB.validate_app_output(private, output)
            with self.assertRaises(ValueError):
                AB.validate_app_output(private, private / "settings.json")
            (private / "lab").symlink_to(Path(root))
            with self.assertRaises(ValueError):
                AB.validate_app_output(private, output)

    def test_app_entry_uses_shared_runner_and_writes_only_metrics(self):
        with tempfile.TemporaryDirectory() as root:
            home = Path(root)
            private = home / ".codescribe"
            archive = private / "sessions"
            archive.mkdir(parents=True)
            for i in range(2):
                with wave.open(str(archive / f"{i}.wav"), "wb") as audio:
                    audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
                    audio.writeframes(b"\x00\x00" * 160000)
            bridge = home / "bridge"
            bridge.write_bytes(b"fixture")
            payload = {"bridge": str(bridge), "count": 2, "result_name": "test.json",
                       "vocabulary": {"terms": ["Iwo"]}}
            calls = []
            def transcribe(bridge, path, terms, *args, **kwargs):
                calls.append(terms)
                self.assertEqual(kwargs.get("command"),
                                 "probe" if path is None else "transcribe_vocabulary_lab")
                return ({"ok": True, "seconds": 1, "backend": "sf_speech_recognizer"},
                        "SECRET_DICTATION Iwo")
            stdout = io.StringIO()
            with patch.object(Path, "home", return_value=home), \
                    patch.object(sys, "stdin", io.StringIO(json.dumps(payload))), \
                    patch.object(AB, "transcribe", side_effect=transcribe), \
                    contextlib.redirect_stdout(stdout):
                self.assertEqual(AB.app_main(), 0)
            output = private / "lab/vocabulary-ab/test.json"
            self.assertNotIn("SECRET_DICTATION", output.read_text() + stdout.getvalue())
            self.assertEqual(calls, [None, None, ["Iwo"], ["Iwo"], None])
            report = json.loads(output.read_text())
            self.assertEqual(report["summary"]["completed_pairs"], 2)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            self.assertFalse((private / "settings.json").exists())
            with self.assertRaises(ValueError):
                AB.write_app_report(private, output, report)

    def test_unicode_multiword_terms_and_occurrences(self):
        hits = AB.term_hits("Iwo, Iwo! STRASSE i ŁÓDŹ. Voice Lab, Voice Laboratory.",
                            ["Iwo", "Straße", "Łódź", "Voice Lab", "Lab"])
        self.assertEqual(hits, {"Iwo": 2, "Straße": 1, "Łódź": 1,
                                "Voice Lab": 1, "Lab": 1})

    def test_reference_excludes_insertions_and_missing_reference_is_proxy(self):
        result = AB.compare_pair("normal words", "Iwo Iwo Zoe words", ["Iwo", "Zoe"], "Zoe")
        self.assertEqual(result["insertion_count"], 2)
        self.assertEqual(result["insertion_terms"], {"Iwo": 2})
        self.assertFalse(result["insertion_is_proxy"])
        self.assertEqual(AB.compare_pair("words", "Iwo Zoe", ["Iwo", "Zoe"])
                         ["insertion_count"], 2)

    def test_omission_threshold_and_empty_baseline(self):
        before = "word " * 100
        self.assertFalse(AB.compare_pair(before, "word " * 95, ["Iwo"])["omission_red_flag"])
        self.assertTrue(AB.compare_pair(before, "word " * 94, ["Iwo"])["omission_red_flag"])
        self.assertFalse(AB.compare_pair("", "Iwo", ["Iwo"])["valid_nonempty_pair"])

    def test_decision_requires_all_30_pairs_and_sf_backend(self):
        def row():
            return {"reference": "none", "baseline": {"seconds": 1, "backend": "sf_speech_recognizer"},
                    "vocabulary": {"seconds": 1, "backend": "sf_speech_recognizer"},
                    "metrics": AB.compare_pair("Iwo is here", "Iwo Iwo here", ["Iwo"])}
        rows = [row() for _ in range(30)]
        self.assertTrue(AB.decision(rows, 30)["enable_apple_live"])
        self.assertFalse(AB.decision(rows[:-1], 30)["enable_apple_live"])
        rows[0]["metrics"] = None
        self.assertFalse(AB.decision(rows, 30)["enable_apple_live"])
        rows[0] = row()
        rows[0]["vocabulary"]["backend"] = "speech_transcriber"
        self.assertFalse(AB.decision(rows, 30)["enable_apple_live"])
        rows[0] = row()
        rows[0]["metrics"] = AB.compare_pair("word " * 100, "Iwo " * 94, ["Iwo"])
        self.assertFalse(AB.decision(rows, 30)["enable_apple_live"])
        rows[0]["metrics"] = AB.compare_pair("word", "Zoe Zoe", ["Zoe"])
        self.assertFalse(AB.decision(rows, 30)["enable_apple_live"])
        for entry in rows:
            entry["metrics"] = None
        blocked = AB.decision(rows, 30)
        self.assertIsNone(blocked["insertion_count"])
        self.assertEqual(blocked["measurement_status"], "blocked")

    def test_corrections_require_explicit_identity(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "corrections.jsonl"
            path.write_text(json.dumps({"timestamp_ms": 1, "edited_text": "private"}) + "\n"
                            + json.dumps({"session_id": "abc", "edited_text": "paired"}) + "\n")
            self.assertEqual(AB.correction_references(path), ({"abc": "paired"}, 1))

    def test_runner_never_persists_or_prints_transcripts(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            archive = root / "sessions"
            archive.mkdir()
            for i in range(2):
                with wave.open(str(archive / f"{i}.wav"), "wb") as audio:
                    audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
                    audio.writeframes(b"\x00\x00" * 160000)
            bridge = root / "bridge"
            bridge.write_text(
                "#!/usr/bin/env python3\nimport json,sys\n"
                "r=json.load(sys.stdin)\n"
                "assert r['command'] in ('transcribe_live','probe') and not r['allow_download']\n"
                "assert ('contextual_strings' not in r or r['contextual_strings']==['Iwo'])\n"
                "print('PRIVATE_STDERR',file=sys.stderr)\n"
                "print(json.dumps({'ok':True,'status':'ok','backend':'sf_speech_recognizer',"
                "'text':'PRIVATE_DICTATION Iwo','segments':[{'text':'PRIVATE_SEGMENT'}]}))\n")
            bridge.chmod(0o755)
            snapshot = root / "vocabulary.json"
            snapshot.write_text('{"terms":["Iwo"]}')
            output = root / "metrics.json"
            command = [sys.executable, str(SCRIPT), "--sessions", str(archive),
                       "--corrections", str(root / "absent.jsonl"), "--bridge", str(bridge),
                       "--vocabulary", str(snapshot), "--count", "2", "--output", str(output)]
            run = subprocess.run(command, capture_output=True, text=True, check=True)
            combined = run.stdout + run.stderr + output.read_text()
            self.assertNotIn("PRIVATE_", combined)
            report = json.loads(output.read_text())
            def check_keys(value):
                if isinstance(value, dict):
                    self.assertFalse(set(value) & {"text", "edited_text", "segments", "transcript", "stdout", "stderr"})
                    for child in value.values():
                        check_keys(child)
                elif isinstance(value, list):
                    for child in value:
                        check_keys(child)
            check_keys(report)
            self.assertEqual(report["summary"]["completed_pairs"], 2)
            self.assertFalse(report["summary"]["enable_apple_live"])
            replay = subprocess.run(command + ["--manifest", str(output)], capture_output=True,
                                    text=True, check=True)
            self.assertNotIn("PRIVATE_", replay.stdout + replay.stderr)
            bridge.write_text("#!/usr/bin/env python3\nimport json\n"
                              "print(json.dumps({'ok':False,'status':'error',"
                              "'speech_auth':'denied','error':'PRIVATE_ERROR'}))\n")
            failed = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertEqual(failed.returncode, 2)
            self.assertNotIn("PRIVATE_", failed.stdout + failed.stderr + output.read_text())
            blocked = json.loads(output.read_text())["summary"]
            self.assertEqual(blocked["completed_pairs"], 0)
            self.assertIsNone(blocked["baseline_hits"])
            duplicate = json.loads(output.read_text())
            duplicate["sample"][1] = duplicate["sample"][0]
            manifest = root / "duplicate.json"
            manifest.write_text(json.dumps(duplicate))
            rejected = subprocess.run(command + ["--manifest", str(manifest)],
                                      capture_output=True, text=True, check=False)
            self.assertEqual(rejected.returncode, 2)
            self.assertIn("distinct audio files", rejected.stderr)


if __name__ == "__main__":
    unittest.main()
