#!/usr/bin/env python3
"""Add an ElevenLabs file-decode arm to preserved native BlackHole takes.

Never records, plays audio, launches Codescribe, or treats a provider as truth.
Each upload is explicit, attempted once, and retained even on failure.
"""
import argparse
import collections
import datetime
import hashlib
import json
import runpy
import shutil
import sys
import time
import urllib.error
import urllib.request
import uuid
import wave
from pathlib import Path

ENDPOINT = "https://api.elevenlabs.io/v1/speech-to-text"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def credential(path):
    lines = [s.strip() for s in Path(path).read_text().splitlines()
             if s.strip() and not s.lstrip().startswith("#")]
    if len(lines) == 1 and "=" not in lines[0]:
        key = lines[0]
    else:
        values = dict(s.removeprefix("export ").split("=", 1)
                      for s in lines if "=" in s)
        key = values.get("ELEVENLABS_API_KEY", "").strip().strip("\"'")
    if not key or any(c.isspace() for c in key):
        raise ValueError("Expected a bare key or ELEVENLABS_API_KEY in the credential file")
    return key


def transcribe(wav, out, key_file, model="scribe_v2", language="pl", opener=None):
    wav, out = Path(wav).resolve(), Path(out).resolve()
    # Inspect the input and credential before reserving the result directory.
    with wave.open(str(wav)) as audio:
        metadata = dict(sample_rate=audio.getframerate(), frames=audio.getnframes(),
                        channels=audio.getnchannels(), sample_bytes=audio.getsampwidth())
    key = credential(key_file)
    out.mkdir(parents=True, mode=0o700, exist_ok=False)
    shutil.copyfile(wav, out / "input.wav")
    params = dict(model_id=model, language_code=language, timestamps_granularity="word",
                  diarize="false", tag_audio_events="false", no_verbatim="false",
                  temperature="0", webhook="false")
    request_info = dict(schema="codescribe.elevenlabs-file-arm.v1", endpoint=ENDPOINT,
                        started_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        input_source=str(wav), input_sha256=sha(out / "input.wav"),
                        audio=metadata, parameters=params, attempt=1,
                        mode="whole_file_cloud_decode", native_acceptance=False,
                        source_sha256=sha(__file__))
    write_json(out / "request.json", request_info)
    boundary = "codescribe-" + uuid.uuid4().hex
    parts = []
    for name, value in params.items():
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode())
    parts += [f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="input.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode(),
              (out / "input.wav").read_bytes(), f"\r\n--{boundary}--\r\n".encode()]
    request = urllib.request.Request(ENDPOINT, data=b"".join(parts), method="POST",
                                     headers={"xi-api-key": key,
                                              "Content-Type": f"multipart/form-data; boundary={boundary}"})
    started = time.monotonic()
    result = dict(ok=False, elapsed_seconds=None, http_status=None, error=None)
    try:
        with (opener or urllib.request.urlopen)(request, timeout=300) as response:
            body = response.read()
            result["http_status"] = response.status
            result["request_id"] = response.headers.get("request-id") or response.headers.get("x-request-id")
        # A provider must not cause credential bytes to land in an artifact.
        if key.encode() in body:
            raise ValueError("Provider response contains credential bytes; response withheld")
        (out / "response.json").write_bytes(body)
        data = json.loads(body)
        if not isinstance(data.get("text"), str) or not isinstance(data.get("words"), list):
            raise ValueError("Expected a single synchronous text/words response")
        (out / "transcript.txt").write_text(data["text"] + "\n")
        result.update(ok=True, language_code=data.get("language_code"),
                      language_probability=data.get("language_probability"),
                      word_items=sum(w.get("type") == "word" for w in data["words"]),
                      response_sha256=sha(out / "response.json"))
    except urllib.error.HTTPError as error:
        result.update(http_status=error.code, error="HTTPError; see response.json")
        with error:
            body = error.read()
        if key.encode() not in body:
            (out / "response.json").write_bytes(body)
    except (OSError, ValueError, urllib.error.URLError) as error:
        # Exception messages may contain server data. Retain the class only.
        result["error"] = type(error).__name__
    finally:
        result["elapsed_seconds"] = time.monotonic() - started
        result["input_unchanged"] = sha(wav) == request_info["input_sha256"]
        write_json(out / "receipt.json", result)
    return result


def score_text(scorer, reference, hypothesis):
    ref, hyp = scorer["tokens"](reference), scorer["tokens"](hypothesis)
    errors, alignment = scorer["align"](ref, hyp)
    counts = collections.Counter(a["operation"] for a in alignment)
    return dict(reference_tokens=len(ref), hypothesis_tokens=len(hyp), errors=errors,
                S=counts["substitute"], D=counts["delete"], I=counts["insert"],
                WER=errors / len(ref) if ref else None,
                numeric_reference_occurrences=sum(t in scorer["numeric"] for t in ref),
                numeric_ordered_correct=sum(a["operation"] == "equal" and
                                          a.get("reference") in scorer["numeric"] for a in alignment)), alignment


def compare(native_run, eleven_run, out, scorer_path, prior_runs=()):
    native_run, eleven_run, out = map(lambda p: Path(p).resolve(), (native_run, eleven_run, out))
    scorer_path = Path(scorer_path).resolve()
    scorer = runpy.run_path(str(scorer_path))
    runs = [native_run] + [Path(p).resolve() for p in prior_runs]
    manifest = json.loads((native_run / "MANIFEST.json").read_text())
    clip_ids = [c["id"] for c in manifest["clips"]]
    if not clip_ids or len(set(clip_ids)) != len(clip_ids):
        raise ValueError("Expected nonempty unique clip IDs")
    if any(not isinstance(cid, str) or len(cid) != 2 or not cid.isascii() or not cid.isdigit()
           for cid in clip_ids):
        raise ValueError("Expected two-digit clip IDs")
    prepared, input_manifests = [], []
    for run in runs:
        input_manifests.append(dict(path=str(run), manifest_sha256=sha(run / "MANIFEST.json"),
                                    provenance=json.loads((run / "PROVENANCE.json").read_text())))
    # Validate every pairing before publishing any scores.
    for clip in manifest["clips"]:
        cid = clip["id"]
        take, remote = native_run / cid, eleven_run / cid
        original_hash = sha(take / "input.wav")
        request = json.loads((remote / "request.json").read_text())
        receipt = json.loads((remote / "receipt.json").read_text())
        if original_hash != clip["sha256"] or request["input_sha256"] != original_hash or sha(remote / "input.wav") != original_hash:
            raise ValueError(f"Clip {cid}: source WAV mismatch")
        if not receipt["ok"] or not receipt["input_unchanged"]:
            raise ValueError(f"Clip {cid}: ElevenLabs attempt was not successful")
        if sha(remote / "response.json") != receipt["response_sha256"]:
            raise ValueError(f"Clip {cid}: ElevenLabs response changed")
        provider_text = json.loads((remote / "response.json").read_text())["text"]
        if (remote / "transcript.txt").read_text() != provider_text + "\n":
            raise ValueError(f"Clip {cid}: ElevenLabs transcript changed")
        ref_path = take / "reference.txt"
        reference = ref_path.read_text() if clip["human_reference"] else None
        if reference is not None and sha(ref_path) not in clip["reference_sha256"]:
            raise ValueError(f"Clip {cid}: human reference mismatch")
        arms = []
        for index, run in enumerate(runs):
            source = run / cid
            if sha(source / "input.wav") != original_hash or (reference is not None and sha(source / "reference.txt") != sha(ref_path)):
                raise ValueError(f"Clip {cid}: native arm input/reference mismatch")
            native_receipt = json.loads((source / "receipt.json").read_text())
            text = (source / "transcript.txt").read_text()
            evidence = {}
            for filename in ("STAGE-SUMMARY.json", "ERROR-RANGES.json", "UI.json",
                             "DELIVERY-FOUNDER-CONFIRMATION.json"):
                if (source / filename).is_file():
                    evidence[filename] = source / filename
            arms.append(dict(name="native" if index == 0 else f"prior_{index}", text=text,
                             terminal=native_receipt["terminal"],
                             external_paste_delivery=native_receipt.get("external_paste_delivery", "NOT_PROVEN"),
                             generation=native_receipt.get("installed_generation"),
                             session=native_receipt.get("session"),
                             capture=native_receipt.get("capture"),
                             source_path=str(source), evidence_sources=evidence,
                             receipt_sha256=sha(source / "receipt.json")))
        arms.append(dict(name="elevenlabs", text=(remote / "transcript.txt").read_text(),
                         request=request, receipt=receipt, seal="NOT_APPLICABLE", delivery="NOT_APPLICABLE"))
        prepared.append((clip, reference, arms))
    out.mkdir(parents=True, mode=0o700, exist_ok=False)
    rows = []
    for clip, reference, arms in prepared:
        cid = clip["id"]
        directory = out / cid
        directory.mkdir()
        row = dict(clip=cid, label=clip.get("label", cid), input_sha256=clip["sha256"],
                   has_human_reference=reference is not None, arms=[])
        if reference is not None:
            reference_file = directory / "reference.txt"
            reference_file.write_text(reference)
            row.update(reference_file=reference_file.relative_to(out).as_posix(),
                       reference_sha256=sha(reference_file))
        for arm in arms:
            name, text = arm["name"], arm.pop("text")
            text_file = directory / f"{name}.txt"
            text_file.write_text(text)
            score, alignment = score_text(scorer, reference, text) if reference is not None else (None, None)
            alignment_file = directory / f"{name}-alignment.json"
            write_json(alignment_file, alignment)
            evidence_files = []
            for filename, source in arm.pop("evidence_sources", {}).items():
                target = directory / f"{name}-{filename}"
                shutil.copyfile(source, target)
                evidence_files.append(dict(kind=filename, path=target.relative_to(out).as_posix(),
                                           sha256=sha(target)))
            row["arms"].append(dict(**arm, score=score,
                                    text_file=text_file.relative_to(out).as_posix(), text_sha256=sha(text_file),
                                    alignment_file=alignment_file.relative_to(out).as_posix(),
                                    alignment_sha256=sha(alignment_file), evidence_files=evidence_files))
        rows.append(row)
    totals = {}
    for group in ("corpus", "without_clip10", "clip10"):
        totals[group] = {}
        selected = [r for r in rows if r["has_human_reference"] and
                    (group == "corpus" or (r["clip"] == "10") == (group == "clip10"))]
        for name in [a["name"] for a in rows[0]["arms"]]:
            scores = [a["score"] for r in selected for a in r["arms"] if a["name"] == name]
            total = {key: sum(s[key] for s in scores) for key in
                     ("reference_tokens", "hypothesis_tokens", "S", "D", "I", "errors", "numeric_reference_occurrences", "numeric_ordered_correct")}
            total["WER"] = total["errors"] / total["reference_tokens"] if total["reference_tokens"] else None
            totals[group][name] = total
    result = dict(schema="codescribe.native-elevenlabs-comparison.v1", native_runs=input_manifests,
                  cloud_run=dict(path=str(eleven_run)),
                  scorer=dict(path=str(scorer_path), sha256=sha(scorer_path)),
                  cloud_is_ground_truth=False, fresh_native_acceptance=False,
                  complete_fixed_corpus=clip_ids == [f"{i:02d}" for i in range(1, 11)],
                  takes=rows, totals=totals)
    write_json(out / "RESULTS.json", result)
    lines = ["# Native BlackHole / ElevenLabs comparison", "",
             "Both arms are scored against unchanged human references. ElevenLabs is a whole-file cloud decode; native results retain their recorded generation and seal/delivery status. This report runs no new native take.", "",
             "|Clip|Arm|N|S|D|I|WER|", "|---|---|---:|---:|---:|---:|---:|"]
    for row in rows:
        for arm in row["arms"]:
            s = arm["score"]
            values = [str(s[k]) for k in ("reference_tokens", "S", "D", "I")] + [f'{s["WER"]:.2%}'] if s and s["WER"] is not None else ["—"] * 5
            lines.append("|" + "|".join([row["clip"], arm["name"]] + values) + "|")
    (out / "REPORT.md").write_text("\n".join(lines) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    upload = commands.add_parser("transcribe", help="One explicit billable WAV upload; no retry")
    for name in ("wav", "out", "api-key-file"):
        upload.add_argument("--" + name, type=Path, required=True)
    upload.add_argument("--model", default="scribe_v2")
    upload.add_argument("--language", default="pl")
    report = commands.add_parser("compare", help="Offline scoring of preserved paired arms")
    for name in ("native-run", "eleven-run", "out", "scorer"):
        report.add_argument("--" + name, type=Path, required=True)
    report.add_argument("--prior-run", type=Path, action="append", default=[])
    args = parser.parse_args()
    if args.command == "transcribe":
        result = transcribe(args.wav, args.out, args.api_key_file, args.model, args.language)
        print(json.dumps(result))
        return 0 if result["ok"] else 1
    result = compare(args.native_run, args.eleven_run, args.out, args.scorer, args.prior_run)
    print(json.dumps({"takes": len(result["takes"]), "totals": result["totals"]}))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError) as error:
        print(f"Comparison refused: {type(error).__name__}", file=sys.stderr)
        sys.exit(2)
