#!/usr/bin/env python3
"""Opt-in, read-only Apple vocabulary A/B; outputs metrics, never dictation text.

Build the bridge, export the Rust builder snapshot against copied config, then:
python3 scripts/stt-vocabulary-ab.py --vocabulary SNAPSHOT.json --output metrics.json
No microphone, app, downloads, authorization request, or runtime file writes.
Without paired human references, insertion is a conservative *proxy*, not WER.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time
import wave


def words(text):
    return re.findall(r"\w+", text.casefold(), flags=re.UNICODE)


def term_hits(text, terms):
    # Count canonical occurrences, including repeated and multiword terms.
    tokens = words(text)
    result = {}
    for term in terms:
        needle = words(term)
        if needle:
            count = sum(tokens[i:i + len(needle)] == needle
                        for i in range(len(tokens) - len(needle) + 1))
            if count:
                result[term] = count
    return result


def compare_pair(before, after, terms, reference=None):
    baseline, biased = term_hits(before, terms), term_hits(after, terms)
    reference_hits = term_hits(reference, terms) if reference is not None else {}
    insertions = {term: count for term, count in biased.items()
                  if term not in baseline and term not in reference_hits}
    a, b = len(words(before)), len(words(after))
    return {
        "baseline_words": a, "vocabulary_words": b,
        "word_drop_fraction": (a - b) / a if a else None,
        "omission_red_flag": b * 100 < a * 95,
        "baseline_hits": sum(baseline.values()),
        "vocabulary_hits": sum(biased.values()),
        "baseline_term_hits": baseline, "vocabulary_term_hits": biased,
        "insertion_terms": insertions,
        "insertion_count": sum(insertions.values()),
        "insertion_is_proxy": reference is None,
        # Empty baseline cannot establish non-regression.
        "valid_nonempty_pair": a > 0 and b > 0,
    }


def select_sample(directory, count):
    candidates = []
    for path in directory.glob("*.wav"):
        try:
            with wave.open(str(path), "rb") as audio:
                duration = audio.getnframes() / audio.getframerate()
            if 10 <= duration <= 120:
                candidates.append((path.stat().st_mtime_ns, path.name, path, duration))
        except (wave.Error, EOFError, OSError, ZeroDivisionError):
            continue
    candidates.sort()
    if len(candidates) < count:
        raise ValueError("Insufficient 10–120 second PCM WAV candidates")
    # Even quantiles over stable (mtime_ns, name) order, including both ends.
    indexes = [i * (len(candidates) - 1) // (count - 1) for i in range(count)]
    selected = []
    for i in indexes:
        stamp, name, path, duration = candidates[i]
        selected.append({
            "name": name, "duration_seconds": duration, "mtime_ns": stamp,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        })
    return selected, len(candidates)


def correction_references(path):
    # Require an explicit session/audio identifier; timestamp proximity is not proof.
    references = {}
    unpaired = 0
    if path.is_file():
        with path.open() as source:
            for line in source:
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(row, dict) or not isinstance(row.get("edited_text"), str):
                    continue
                meta = row.get("meta")
                meta = meta if isinstance(meta, dict) else {}
                identity = next((value for key in ("audio_path", "session_id", "session")
                                 for value in (row.get(key), meta.get(key))
                                 if isinstance(value, str) and value), None)
                if identity:
                    references[Path(identity).stem] = row["edited_text"]
                else:
                    unpaired += 1
    return references, unpaired


def paired_reference(directory, name, corrections):
    stem = Path(name).stem
    if stem in corrections:
        return corrections[stem], "corrections.edited_text"
    path = directory / (stem + ".txt")
    if path.is_file():
        return path.read_text(), "same_stem_txt_unverified_human"
    path = directory / (stem + ".jsonl")
    text = None
    if path.is_file():
        for line in path.read_text().splitlines():
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if isinstance(row, dict) and isinstance(row.get("edited_text"), str):
                text = row["edited_text"]
    return (text, "same_stem_jsonl.edited_text") if text is not None else (None, "none")


def transcribe(bridge, path, terms, locale, timeout, command="transcribe_live"):
    request = {"protocol_version": 1, "command": command,
               "locale": locale, "allow_download": False}
    if path is not None:
        request["audio_path"] = str(path)
    if terms is not None:
        request["contextual_strings"] = terms
    started = time.monotonic()
    child_env = os.environ.copy()
    # Keep the shipped default lane even if the invoking shell arms the PoC.
    child_env.pop("CODESCRIBE_APPLE_DICTATION_TRANSCRIBER", None)
    receipt = {"ok": False, "status": "process_or_protocol_error"}
    text = ""
    try:
        child = subprocess.run([str(bridge)], input=json.dumps(request),
                               text=True, capture_output=True, timeout=timeout,
                               env=child_env, check=False)
        receipt.update(exit_code=child.returncode, stdout_bytes=len(child.stdout.encode()),
                       stderr_bytes=len(child.stderr.encode()))
        response = json.loads(child.stdout)
        if not isinstance(response, dict):
            raise ValueError("non-object response")
        ok = child.returncode == 0 and response.get("ok") is True
        if ok and not isinstance(response.get("text"), str):
            raise ValueError("missing text")
        receipt["ok"] = ok
        for key, allowed in {
                "status": {"ok", "error"},
                "backend": {"sf_speech_recognizer", "speech_transcriber", "dictation_transcriber"},
                "speech_auth": {"not_determined", "denied", "restricted", "authorized"}}.items():
            value = response.get(key)
            receipt[key] = value if isinstance(value, str) and value in allowed else None
        text = response.get("text", "") if ok else ""
    except subprocess.TimeoutExpired:
        receipt.update(ok=False, status="timeout")
    except (ValueError, OSError):
        receipt.update(ok=False, status="process_or_protocol_error")
    # Never retain/print child stdout, stderr, error strings, or segments.
    receipt["seconds"] = time.monotonic() - started
    return receipt, text


def decision(rows, count):
    completed = [row for row in rows if row.get("metrics") is not None]
    valid = [row for row in completed if row["metrics"]["valid_nonempty_pair"]]
    summary = {
        "selected": len(rows), "completed_pairs": len(completed),
        "valid_nonempty_pairs": len(valid),
        "omission_red_flags": sum(row["metrics"]["omission_red_flag"] for row in completed),
        "insertion_count": sum(row["metrics"]["insertion_count"] for row in completed),
        "baseline_hits": sum(row["metrics"]["baseline_hits"] for row in completed),
        "vocabulary_hits": sum(row["metrics"]["vocabulary_hits"] for row in completed),
        "paired_references": sum(row["reference"] != "none" for row in rows),
        "baseline_seconds": sum(row["baseline"]["seconds"] for row in rows),
        "vocabulary_seconds": sum(row["vocabulary"]["seconds"] for row in rows),
    }
    summary["enable_apple_live"] = (
        count == 30 and len(rows) == count and len(valid) == count
        and all(row["baseline"].get("backend") == "sf_speech_recognizer"
                and row["vocabulary"].get("backend") == "sf_speech_recognizer" for row in valid)
        and not summary["omission_red_flags"]
        and summary["insertion_count"] <= 1
        and summary["vocabulary_hits"] > summary["baseline_hits"]
    )
    summary["measurement_status"] = "complete" if len(completed) == count else "blocked"
    if not completed:
        for key in ("omission_red_flags", "insertion_count", "baseline_hits", "vocabulary_hits"):
            summary[key] = None
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sessions", type=Path, default=Path.home() / ".codescribe/sessions")
    parser.add_argument("--corrections", type=Path,
                        default=Path.home() / ".codescribe/quality/corrections.jsonl")
    parser.add_argument("--bridge", type=Path, default=Path("target/release/codescribe-stt-bridge"))
    parser.add_argument("--vocabulary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, help="Replay a previous metrics JSON sample")
    parser.add_argument("--count", type=int, default=30)
    parser.add_argument("--locale", default="pl-PL")
    args = parser.parse_args()
    if args.count < 2:
        parser.error("count must be at least 2")
    # The runner may only write outside the private runtime directory.
    private = (Path.home() / ".codescribe").resolve()
    if (args.output.resolve().is_relative_to(private)
            or args.output.resolve().is_relative_to(args.sessions.resolve())):
        parser.error("output must be outside ~/.codescribe and the audio archive")
    snapshot = json.loads(args.vocabulary.read_text())
    terms = snapshot["terms"]
    if not isinstance(terms, list) or not terms or len(terms) > 100 or not all(
            isinstance(term, str) and words(term) for term in terms):
        parser.error("vocabulary must contain 1–100 nonempty canonical strings")
    if args.manifest:
        previous = json.loads(args.manifest.read_text())
        sample, eligible = previous["sample"], previous["eligible"]
        if len(sample) != args.count:
            parser.error("manifest count mismatch")
    else:
        sample, eligible = select_sample(args.sessions, args.count)
    if len({entry["name"] for entry in sample}) != len(sample):
        parser.error("sample must contain distinct audio files")
    corrections, unpaired = correction_references(args.corrections)
    bridge = args.bridge.resolve()
    probe, _ = transcribe(bridge, None, None, args.locale, 30, command="probe")
    report = {"schema": "codescribe.vocabulary-ab.v1", "sample": sample,
              "eligible": eligible, "selection": "mtime_ns/name even quantiles, 10–120s PCM WAV",
              "vocabulary": snapshot,
              "vocabulary_sha256": hashlib.sha256(args.vocabulary.read_bytes()).hexdigest(),
              "bridge_sha256": hashlib.sha256(bridge.read_bytes()).hexdigest(),
              "locale": args.locale, "command": "transcribe_live", "allow_download": False,
              "probe": probe,
              "unpaired_correction_rows": unpaired, "rows": []}
    for index, entry in enumerate(sample):
        # A manifest cannot redirect the runner to arbitrary filesystem paths.
        name = entry["name"]
        if Path(name).name != name or Path(name).suffix != ".wav":
            parser.error("invalid manifest audio name")
        path = (args.sessions / name).resolve()
        if not path.is_relative_to(args.sessions.resolve()):
            parser.error("audio must remain in the archive")
        if hashlib.sha256(path.read_bytes()).hexdigest() != entry["sha256"]:
            parser.error("audio changed since selection")
        reference, source = paired_reference(args.sessions, name, corrections)
        results = {}
        # Alternate first arm to reduce systematic warm-up/order effects.
        order = [("baseline", None), ("vocabulary", terms)]
        if index % 2:
            order.reverse()
        for arm, context in order:
            results[arm] = transcribe(bridge, path, context, args.locale,
                                      min(180, max(30, entry["duration_seconds"] + 20)))
        a, before = results["baseline"]
        b, after = results["vocabulary"]
        metrics = compare_pair(before, after, terms, reference) if a["ok"] and b["ok"] else None
        report["rows"].append({"name": name, "reference": source,
                               "baseline": a, "vocabulary": b, "metrics": metrics})
        print(json.dumps({"file": index + 1, "baseline_ok": a["ok"],
                          "vocabulary_ok": b["ok"], "metrics": metrics}), flush=True)
        report["summary"] = decision(report["rows"], args.count)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(report["summary"]))
    return 0 if report["summary"]["completed_pairs"] == args.count else 2


if __name__ == "__main__":
    raise SystemExit(main())
