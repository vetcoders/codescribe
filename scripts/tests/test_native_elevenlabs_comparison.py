#!/usr/bin/env python3
"""Offline contract tests. No real credentials, HTTP, microphone or model."""
import importlib.util
import io
import json
import tempfile
import unittest
import urllib.error
import wave
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "comparison", Path(__file__).resolve().parents[1] / "compare-native-elevenlabs.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class Response(io.BytesIO):
    status = 200
    headers = {"request-id": "fixture-request"}


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.wav = self.root / "speech.wav"
        with wave.open(str(self.wav), "wb") as audio:
            audio.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
            audio.writeframes(b"\x01\x00" * 1600)
        self.key = self.root / "key"
        self.key.write_text("fake-test-key")
        self.out = self.root / "result"

    def testBareAndNamedCredentialWithoutShellEvaluation(self):
        self.assertEqual(MODULE.credential(self.key), "fake-test-key")
        self.key.write_text('export ELEVENLABS_API_KEY="literal-$(no-execution)"\n')
        self.assertEqual(MODULE.credential(self.key), "literal-$(no-execution)")

    def testExactWaveAndVerbatimRequestAreRetainedWithoutCredential(self):
        requests = []
        def opener(request, timeout):
            requests.append(request)
            self.assertEqual(timeout, 300)
            self.assertEqual(request.get_header("Xi-api-key"), "fake-test-key")
            self.assertIn(self.wav.read_bytes(), request.data)
            self.assertIn(b'name="no_verbatim"\r\n\r\nfalse', request.data)
            return Response(json.dumps(dict(text="osiem osiem", words=[
                dict(type="word", text="osiem", start=0, end=.04),
                dict(type="word", text="osiem", start=.05, end=.1)])).encode())
        result = MODULE.transcribe(self.wav, self.out, self.key, opener=opener)
        self.assertTrue(result["ok"])
        self.assertTrue(result["input_unchanged"])
        self.assertEqual(len(requests), 1)
        self.assertEqual((self.out / "input.wav").read_bytes(), self.wav.read_bytes())
        self.assertEqual((self.out / "transcript.txt").read_text(), "osiem osiem\n")
        for p in self.out.iterdir():
            self.assertNotIn(b"fake-test-key", p.read_bytes())

    def testFailedAttemptIsRetainedWithoutRetry(self):
        calls = []
        def opener(request, timeout):
            calls.append(request)
            raise urllib.error.HTTPError(MODULE.ENDPOINT, 429, "rate", {}, io.BytesIO(b'{"detail":"rate limit"}'))
        result = MODULE.transcribe(self.wav, self.out, self.key, opener=opener)
        self.assertFalse(result["ok"])
        self.assertEqual(result["http_status"], 429)
        self.assertEqual(len(calls), 1)
        self.assertTrue((self.out / "receipt.json").exists())
        self.assertFalse((self.out / "transcript.txt").exists())

    def testRerunCannotReplacePreviousAttempt(self):
        self.out.mkdir()
        sentinel = self.out / "receipt.json"
        sentinel.write_text("retained")
        with self.assertRaises(FileExistsError):
            MODULE.transcribe(self.wav, self.out, self.key,
                              opener=lambda *_args, **_kwargs: self.fail("must not upload"))
        self.assertEqual(sentinel.read_text(), "retained")

    def testUnexpectedResponseShapeIsNotSuccessful(self):
        result = MODULE.transcribe(self.wav, self.out, self.key,
                                   opener=lambda *_args, **_kwargs: Response(b'{"transcripts": []}'))
        self.assertFalse(result["ok"])
        self.assertTrue((self.out / "response.json").exists())

    def testCredentialEchoIsNeverSaved(self):
        result = MODULE.transcribe(self.wav, self.out, self.key,
                                   opener=lambda *_args, **_kwargs: Response(b'{"text":"fake-test-key"}'))
        self.assertFalse(result["ok"])
        for p in self.out.iterdir():
            self.assertNotIn(b"fake-test-key", p.read_bytes())

    def testScoringUsesExternalOrderedAlignmentAndPreservesDenominator(self):
        alignment = [dict(operation="equal", reference="osiem"),
                     dict(operation="delete", reference="osiem"),
                     dict(operation="substitute", reference="nie", hypothesis="tak"),
                     dict(operation="insert", hypothesis="dodatkowe")]
        scorer = dict(tokens=str.split, numeric={"osiem"}, align=lambda r, h: (3, alignment))
        score, actual = MODULE.score_text(scorer, "osiem osiem nie", "osiem tak dodatkowe")
        self.assertEqual(actual, alignment)
        self.assertEqual((score["S"], score["D"], score["I"]), (1, 1, 1))
        self.assertEqual(score["reference_tokens"], 3)
        self.assertEqual(score["numeric_reference_occurrences"], 2)
        self.assertEqual(score["numeric_ordered_correct"], 1)

    def paired_fixture(self):
        native, remote = self.root / "native", self.root / "eleven"
        native.mkdir()
        remote.mkdir()
        (native / "PROVENANCE.json").write_text('{"build": "historical-fixture"}')
        clips = []
        for cid in ("09", "10"):
            take, cloud = native / cid, remote / cid
            take.mkdir()
            cloud.mkdir()
            for directory in (take, cloud):
                (directory / "input.wav").write_bytes(self.wav.read_bytes())
                (directory / "transcript.txt").write_text("osiem osiem\n")
            (take / "reference.txt").write_text("osiem osiem")
            (take / "receipt.json").write_text('{"terminal":{"end_reason":"coverage_refused"}}')
            MODULE.write_json(cloud / "request.json", dict(input_sha256=MODULE.sha(self.wav)))
            (cloud / "response.json").write_text('{"text":"osiem osiem","words":[]}')
            MODULE.write_json(cloud / "receipt.json", dict(
                ok=True, input_unchanged=True, response_sha256=MODULE.sha(cloud / "response.json")))
            clips.append(dict(id=cid, sha256=MODULE.sha(self.wav),
                              human_reference=["human.txt"] if cid == "10" else [],
                              reference_sha256=[MODULE.sha(take / "reference.txt")]))
        MODULE.write_json(native / "MANIFEST.json", dict(clips=clips))
        scorer_path = self.root / "scorer.py"
        scorer_path.write_text("# external scorer fixture")
        scorer = dict(tokens=str.split, numeric={"osiem"}, align=lambda r, h: (
            0, [dict(operation="equal", reference=t, hypothesis=t) for t in r]))
        return native, remote, scorer_path, scorer

    def testPairedReportKeepsSealAndNoReferenceHasNoWER(self):
        native, remote, scorer_path, scorer = self.paired_fixture()
        with patch.object(MODULE.runpy, "run_path", return_value=scorer):
            result = MODULE.compare(native, remote, self.out, scorer_path)
        self.assertFalse(result["cloud_is_ground_truth"])
        self.assertFalse(result["fresh_native_acceptance"])
        self.assertTrue(all(a["score"] is None for a in result["takes"][0]["arms"]))
        self.assertEqual(result["takes"][1]["arms"][0]["terminal"]["end_reason"], "coverage_refused")
        self.assertEqual(result["totals"]["clip10"]["elevenlabs"]["numeric_ordered_correct"], 2)
        self.assertEqual(result["totals"]["without_clip10"]["elevenlabs"]["WER"], None)

    def testPairingRejectsDifferentPCMBeforePublishingScore(self):
        native, remote, scorer_path, scorer = self.paired_fixture()
        (remote / "10/input.wav").write_bytes(b"different wave")
        with patch.object(MODULE.runpy, "run_path", return_value=scorer):
            with self.assertRaisesRegex(ValueError, "WAV mismatch"):
                MODULE.compare(native, remote, self.out, scorer_path)
        self.assertFalse(self.out.exists())

    def testVoiceLabPackRetainsReferenceTextsAlignmentsAndDeliveryObservation(self):
        native, remote, scorer_path, scorer = self.paired_fixture()
        confirmation = native / "10/DELIVERY-FOUNDER-CONFIRMATION.json"
        confirmation.write_text('{"external_paste":"FOUNDER_CONFIRMED"}')
        with patch.object(MODULE.runpy, "run_path", return_value=scorer):
            result = MODULE.compare(native, remote, self.out, scorer_path)
        row = result["takes"][1]
        self.assertEqual(MODULE.sha(self.out / row["reference_file"]), row["reference_sha256"])
        self.assertEqual(row["input_sha256"], MODULE.sha(self.wav))
        for arm in row["arms"]:
            self.assertEqual(MODULE.sha(self.out / arm["text_file"]), arm["text_sha256"])
            self.assertEqual(MODULE.sha(self.out / arm["alignment_file"]), arm["alignment_sha256"])
        arm = row["arms"][0]
        self.assertEqual(arm["external_paste_delivery"], "NOT_PROVEN")
        evidence = arm["evidence_files"][0]
        self.assertEqual((self.out / evidence["path"]).read_bytes(), confirmation.read_bytes())
        self.assertEqual(evidence["sha256"], MODULE.sha(confirmation))

    def testEditedProviderTranscriptIsNotAcceptedAsRawOutput(self):
        native, remote, scorer_path, scorer = self.paired_fixture()
        (remote / "10/transcript.txt").write_text("prettier edited output\n")
        with patch.object(MODULE.runpy, "run_path", return_value=scorer):
            with self.assertRaisesRegex(ValueError, "transcript changed"):
                MODULE.compare(native, remote, self.out, scorer_path)
        self.assertFalse(self.out.exists())

    def testPriorVersionCannotUseDifferentReference(self):
        native, remote, scorer_path, scorer = self.paired_fixture()
        prior = self.root / "prior"
        import shutil
        shutil.copytree(native, prior)
        (prior / "10/reference.txt").write_text("changed reference")
        with patch.object(MODULE.runpy, "run_path", return_value=scorer):
            with self.assertRaisesRegex(ValueError, "input/reference mismatch"):
                MODULE.compare(native, remote, self.out, scorer_path, [prior])
        self.assertFalse(self.out.exists())


if __name__ == "__main__":
    unittest.main()
