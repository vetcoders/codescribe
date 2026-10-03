#!/usr/bin/env python3
"""Summarize W-0b instrumentation; missing measurements stay null. No capture."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import wave


def quantile(values, fraction):
    """Nearest-rank quantile, with an explicit empty-sample result."""
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)] if values else None


def summarize(samples):
    result = {}
    for field in (
        "cpu_percent", "rss_bytes", "projection_main_ms", "engine_to_visible_ms",
        "light_plus_total_ms", "stop_to_delivery_ms",
    ):
        values = [row[field] for row in samples if row.get(field) is not None]
        if any(isinstance(value, bool) or not isinstance(value, (int, float))
               or not math.isfinite(value) or value < 0 for value in values):
            raise ValueError(f"invalid metric: {field}")
        result[field] = {
            "n": len(values), "mean": sum(values) / len(values) if values else None,
            "p95": quantile(values, .95), "p99": quantile(values, .99),
            "max": max(values) if values else None,
        }
    cpu = result["cpu_percent"]
    result["cpu_cores"] = {key: value / 100 if value is not None else None
                           for key, value in cpu.items() if key != "n"}
    result["cpu_cores"]["n"] = cpu["n"]
    memory = [(row["elapsed_s"], row["rss_bytes"]) for row in samples
              if row.get("elapsed_s") is not None and row.get("rss_bytes") is not None]
    if any(isinstance(time, bool) or not isinstance(time, (int, float))
           or not math.isfinite(time) or time < 0 for time, _ in memory):
        raise ValueError("invalid elapsed_s")
    times = [time for time, _ in memory]
    mean_time = sum(times) / len(times) if times else 0
    mean_rss = sum(rss for _, rss in memory) / len(memory) if memory else 0
    denominator = sum((time - mean_time) ** 2 for time in times)
    result["rss_trend_bytes_per_s"] = (
        sum((time - mean_time) * (rss - mean_rss) for time, rss in memory) / denominator
        if denominator else None
    )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wav", type=Path)
    parser.add_argument("--telemetry", type=Path, help="JSON with samples and metadata; collected externally")
    parser.add_argument("--trail", type=Path)
    parser.add_argument("--speech-seconds", type=float, help="independently annotated speech duration")
    parser.add_argument("--dry-run", action="store_true", help="synthetic instrument self-check, no product execution")
    args = parser.parse_args()
    if args.dry_run and args.telemetry:
        parser.error("dry-run cannot consume real telemetry")
    with wave.open(str(args.wav)) as wav:
        duration = wav.getnframes() / wav.getframerate()
        rate = wav.getframerate()
    speech = args.speech_seconds
    if speech is not None and (not math.isfinite(speech) or not 0 < speech <= duration):
        parser.error("speech duration must be positive and no longer than the WAV")
    telemetry = json.loads(args.telemetry.read_text()) if args.telemetry else {"samples": [], "metadata": {}}
    if args.dry_run:
        telemetry = {"samples": [{"projection_main_ms": value} for value in (1, 2, 3, 4, 5)],
                     "metadata": {"provenance": "synthetic-tool-self-check"}}
    report = {
        "schema": "codescribe.w0b-summary.v1", "dry_run": args.dry_run,
        "fixture": args.wav.name, "fixture_sha256": hashlib.sha256(args.wav.read_bytes()).hexdigest(),
        "wav_seconds": duration, "sample_rate_hz": rate,
        "metadata": telemetry.get("metadata", {}), "metrics": summarize(telemetry["samples"]),
        "trail_bytes": args.trail.stat().st_size if args.trail else None,
        "speech_seconds": speech,
        "trail_bytes_per_speech_minute": args.trail.stat().st_size * 60 / speech
            if args.trail and speech else None,
        "thermal_acceptance": "unmeasured", "release_acceptance": "not_evaluated",
    }
    print(json.dumps(report, indent=2, sort_keys=True, allow_nan=False))


if __name__ == "__main__":
    main()
