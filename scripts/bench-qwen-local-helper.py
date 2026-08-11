#!/usr/bin/env python3
"""Opt-in, content-free Qwen3-ASR q5/q8 local-helper benchmark.

The runner is supplied by the operator. This harness downloads nothing, keeps
audio/reference text private, and writes only aggregate metrics plus model
provenance. Runner stdout is parsed in memory and never copied to the result.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import threading
import time
import unicodedata
import wave
from pathlib import Path
from typing import Any


SCHEMA_VERSION = "codescribe.local-helper-bench.v1"


def normalized(text: str) -> str:
    folded = unicodedata.normalize("NFKC", text).casefold()
    return " ".join("".join(ch if ch.isalnum() else " " for ch in folded).split())


def edit_distance(left: list[str], right: list[str]) -> int:
    row = list(range(len(right) + 1))
    for i, lhs in enumerate(left, 1):
        next_row = [i]
        for j, rhs in enumerate(right, 1):
            next_row.append(
                min(next_row[-1] + 1, row[j] + 1, row[j - 1] + (lhs != rhs))
            )
        row = next_row
    return row[-1]


def artifact_digest(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    files = [path] if path.is_file() else sorted(item for item in path.rglob("*") if item.is_file())
    for item in files:
        relative = item.name if path.is_file() else item.relative_to(path).as_posix()
        digest.update(relative.encode("utf-8"))
        with item.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
                total += len(chunk)
    return digest.hexdigest(), total


def wav_seconds(path: Path) -> float:
    with wave.open(str(path), "rb") as handle:
        return handle.getnframes() / float(handle.getframerate())


def rss_bytes(pid: int) -> int:
    probe = subprocess.run(
        ["/bin/ps", "-o", "rss=", "-p", str(pid)],
        capture_output=True,
        text=True,
        check=False,
    )
    try:
        return int(probe.stdout.strip()) * 1024
    except ValueError:
        return 0


def run_case(runner: Path, model_path: Path, audio_path: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    started = time.monotonic()
    process = subprocess.Popen(
        [str(runner), "--model", str(model_path), "--audio", str(audio_path), "--json"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    peak_rss = 0
    sampling = True

    def sample() -> None:
        nonlocal peak_rss
        while sampling:
            peak_rss = max(peak_rss, rss_bytes(process.pid))
            time.sleep(0.05)

    sampler = threading.Thread(target=sample, name="qwen-bench-rss", daemon=True)
    sampler.start()
    stdout, _stderr = process.communicate()
    sampling = False
    sampler.join(timeout=1.0)
    elapsed = time.monotonic() - started
    if process.returncode != 0:
        raise RuntimeError(f"helper runner exited with status {process.returncode}")
    try:
        payload = json.loads(stdout)
        transcript = payload["transcript"]
        cold_load = float(payload["cold_load_seconds"])
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise RuntimeError("helper runner returned an invalid content contract") from error
    if not isinstance(transcript, str) or cold_load < 0:
        raise RuntimeError("helper runner returned an invalid content contract")
    duration = float(payload.get("audio_seconds", wav_seconds(audio_path)))
    if duration <= 0:
        raise RuntimeError("audio duration must be positive")
    segments = payload.get("segments", [])
    timestamp_monotonic = all(
        isinstance(segment, dict)
        and isinstance(segment.get("start_ms"), (int, float))
        and isinstance(segment.get("end_ms"), (int, float))
        and segment["end_ms"] >= segment["start_ms"]
        and (index == 0 or segment["start_ms"] >= segments[index - 1]["end_ms"])
        for index, segment in enumerate(segments)
    )
    process_exit_rss = rss_bytes(process.pid)
    metrics = {
        "cold_load_seconds": cold_load,
        "wall_seconds": elapsed,
        "audio_seconds": duration,
        "rtf": elapsed / duration,
        "peak_rss_bytes": peak_rss,
        "post_exit_rss_bytes": process_exit_rss,
        "process_exited": process_exit_rss == 0,
        "timestamps_present": bool(segments),
        "timestamps_monotonic": timestamp_monotonic,
    }
    return payload, metrics


def ratio(errors: int, units: int) -> float | None:
    return errors / units if units else None


def benchmark(args: argparse.Namespace) -> dict[str, Any]:
    models_doc = json.loads(args.models.read_text(encoding="utf-8"))
    corpus_doc = json.loads(args.corpus.read_text(encoding="utf-8"))
    models = models_doc.get("models", [])
    cases = corpus_doc.get("cases", [])
    quants = {model.get("quantization") for model in models}
    if quants != {"q5", "q8"}:
        raise RuntimeError("models manifest must contain exactly q5 and q8 entries")
    if not cases:
        raise RuntimeError("private corpus manifest contains no cases")

    results = []
    for model in sorted(models, key=lambda item: item["quantization"]):
        model_path = Path(model["artifact_path"]).expanduser().resolve()
        checksum, bundle_size = artifact_digest(model_path)
        if model.get("sha256") and model["sha256"].lower() != checksum:
            raise RuntimeError(f"{model['quantization']} model checksum mismatch")
        totals = {
            "word_errors": 0,
            "words": 0,
            "char_errors": 0,
            "chars": 0,
            "terms_hit": 0,
            "terms": 0,
            "code_switch_errors": 0,
            "code_switch_words": 0,
        }
        runs = []
        for case in cases:
            audio_path = Path(case["audio_path"]).expanduser().resolve()
            reference = Path(case["reference_path"]).expanduser().read_text(encoding="utf-8")
            payload, process_metrics = run_case(args.runner, model_path, audio_path)
            hypothesis = normalized(payload["transcript"])
            truth = normalized(reference)
            truth_words, hypothesis_words = truth.split(), hypothesis.split()
            word_errors = edit_distance(truth_words, hypothesis_words)
            char_errors = edit_distance(list(truth.replace(" ", "")), list(hypothesis.replace(" ", "")))
            tags = case.get("tags", [])
            if "pl_vet" in tags:
                totals["word_errors"] += word_errors
                totals["words"] += len(truth_words)
                totals["char_errors"] += char_errors
                totals["chars"] += len(truth.replace(" ", ""))
            terms = [normalized(term) for term in case.get("terms", [])]
            totals["terms"] += len(terms)
            totals["terms_hit"] += sum(term in hypothesis for term in terms)
            if "pl_en_code_switch" in tags:
                totals["code_switch_errors"] += word_errors
                totals["code_switch_words"] += len(truth_words)
            runs.append({"case_id": case["id"], **process_metrics})
        results.append(
            {
                "model": {
                    "model_id": model["model_id"],
                    "quantization": model["quantization"],
                    "revision": model["revision"],
                    "sha256": checksum,
                    "license": model["license"],
                    "bundle_size_bytes": bundle_size,
                },
                "quality": {
                    "pl_vet_wer": ratio(totals["word_errors"], totals["words"]),
                    "pl_vet_cer": ratio(totals["char_errors"], totals["chars"]),
                    "term_recall": ratio(totals["terms_hit"], totals["terms"]),
                    "pl_en_code_switch_wer": ratio(
                        totals["code_switch_errors"], totals["code_switch_words"]
                    ),
                },
                "runs": runs,
            }
        )
    return {
        "schema": SCHEMA_VERSION,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "content_retained": False,
        "results": results,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--models", type=Path)
    parser.add_argument("--corpus", type=Path)
    parser.add_argument("--runner", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        assert edit_distance("kot ma".split(), "kot nie ma".split()) == 1
        assert normalized("ŻÓŁĆ, Qwen!") == "żółć qwen"
        print("SELF_TEST_OK")
        return 0
    if not all((args.models, args.corpus, args.runner, args.out)):
        parser.error("--models, --corpus, --runner, and --out are required")
    result = benchmark(args)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"BENCH_OK schema={SCHEMA_VERSION} models={len(result['results'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
