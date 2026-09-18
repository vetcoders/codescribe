#!/usr/bin/env python3
"""Generate the public deterministic P0-B five-Iwo PCM fixture.

The waveform is mathematical synthesis, not speech and not a transformation of
an operator recording.  It models five separated voiced regions carrying the
same lexical label so the verifier can attack text-based occurrence collapse.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import wave
from pathlib import Path


SAMPLE_RATE = 16_000
PRE_ROLL_SAMPLES = 1_600
BURST_SAMPLES = 4_000
VALLEY_SAMPLES = 3_200
BURST_COUNT = 5
CALIBRATION_VERSION = "p0-b-synthetic-energy-v1"


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def voiced_sample(index: int, burst_index: int) -> float:
    """Return one deterministic, vowel-like harmonic sample with soft edges."""

    t = index / SAMPLE_RATE
    fundamental = 170.0 + burst_index * 7.0
    attack = min(1.0, index / 240.0)
    release = min(1.0, (BURST_SAMPLES - 1 - index) / 240.0)
    envelope = max(0.0, min(attack, release))
    carrier = (
        math.sin(2.0 * math.pi * fundamental * t)
        + 0.34 * math.sin(2.0 * math.pi * fundamental * 2.0 * t + 0.21)
        + 0.17 * math.sin(2.0 * math.pi * fundamental * 3.0 * t + 0.47)
    )
    return 0.43 * envelope * carrier / 1.51


def quantize(sample: float) -> int:
    return max(-32_768, min(32_767, round(sample * 32_767.0)))


def dbfs(value: float) -> float:
    return 20.0 * math.log10(max(value, 1.0e-12))


def build_samples() -> tuple[list[int], list[dict[str, object]], list[dict[str, int]]]:
    pcm = [0] * PRE_ROLL_SAMPLES
    bursts: list[dict[str, object]] = []
    valleys: list[dict[str, int]] = []

    for burst_index in range(BURST_COUNT):
        sample_start = len(pcm)
        burst_pcm = [quantize(voiced_sample(i, burst_index)) for i in range(BURST_SAMPLES)]
        pcm.extend(burst_pcm)
        sample_end = len(pcm)

        normalized = [sample / 32_768.0 for sample in burst_pcm]
        energy_integral = sum(sample * sample for sample in normalized)
        rms = math.sqrt(energy_integral / len(normalized))
        peak = max(abs(sample) for sample in normalized)
        bursts.append(
            {
                "ordinal": burst_index + 1,
                "label": "Iwo",
                "sample_start": sample_start,
                "sample_end": sample_end,
                "duration_ms": BURST_SAMPLES * 1000.0 / SAMPLE_RATE,
                "energy_integral": round(energy_integral, 9),
                "mean_rms_dbfs": round(dbfs(rms), 6),
                "peak_dbfs": round(dbfs(peak), 6),
                "vad_open_sample": sample_start,
                "vad_close_sample": sample_end,
                "evidence_calibration_version": CALIBRATION_VERSION,
            }
        )

        valley_start = len(pcm)
        pcm.extend([0] * VALLEY_SAMPLES)
        valleys.append({"sample_start": valley_start, "sample_end": len(pcm)})

    return pcm, bursts, valleys


def wav_bytes(samples: list[int]) -> bytes:
    from io import BytesIO

    buffer = BytesIO()
    with wave.open(buffer, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(SAMPLE_RATE)
        wav.writeframes(b"".join(struct.pack("<h", sample) for sample in samples))
    return buffer.getvalue()


def manifest_for(
    samples: list[int], bursts: list[dict[str, object]], valleys: list[dict[str, int]], digest: str
) -> dict[str, object]:
    return {
        "schema": "codescribe.p0-b.five-iwo-fixture.v1",
        "fixture": "tests/fixtures/p0_b_five_iwo.wav",
        "wav_sha256": digest,
        "sample_rate": SAMPLE_RATE,
        "channels": 1,
        "sample_format": "pcm_s16le",
        "sample_count": len(samples),
        "label": "Iwo",
        "expected_occurrences": BURST_COUNT,
        "minimum_energy_integral": 25.0,
        "minimum_valley_samples": VALLEY_SAMPLES,
        "bursts": bursts,
        "vad_valleys": valleys,
        "provenance": {
            "kind": "deterministic_mathematical_synthesis",
            "generator": "scripts/generate-five-iwo-fixture.py",
            "generator_version": 1,
            "operator_recording": False,
            "derived_from_human_audio": False,
            "description": "Harmonic tone bursts with deterministic envelopes; labels are test metadata only.",
        },
        "controls": [
            {"id": "N1", "mutation": "five_disjoint_ranges_same_label", "expected": "five"},
            {"id": "N2", "mutation": "replay_same_observation", "expected": "refused"},
            {"id": "N3", "mutation": "whisper_relabels_same_range_before_seal", "expected": "correct_one"},
            {"id": "N4", "mutation": "remove_energy", "expected": "zero"},
            {"id": "N5", "mutation": "remove_vad_close", "expected": "not_sealed"},
            {"id": "N6", "mutation": "leave_observation_frontier_open", "expected": "not_sealed"},
            {"id": "N7", "mutation": "automatic_formatter_after_seal", "expected": "refused"},
            {"id": "N8", "mutation": "manual_human_after_seal", "expected": "accepted_with_provenance"},
            {"id": "N9", "mutation": "zero_fifth_burst", "expected": "four_vs_five"},
            {"id": "N10", "mutation": "bus_energy_store_unavailable", "expected": "project_ledger_evidence"},
            {"id": "N11", "mutation": "remove_acoustic_serial_field", "expected": "fail_closed"},
            {"id": "N12", "mutation": "remove_layer_decision", "expected": "fail_closed"},
            {"id": "A1", "mutation": "continuous_voiced_region", "expected": "one_occurrence"},
            {"id": "A2", "mutation": "two_tokens_without_deep_valley", "expected": "no_physical_split"},
            {"id": "A3", "mutation": "conflicting_labels_same_range", "expected": "one_occurrence"},
            {"id": "A4", "mutation": "homophone_labels_same_range", "expected": "one_occurrence"},
            {"id": "A5", "mutation": "below_threshold_noise", "expected": "zero"},
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify committed fixture bytes and manifest without rewriting them",
    )
    args = parser.parse_args()

    root = repo_root()
    wav_path = root / "tests/fixtures/p0_b_five_iwo.wav"
    manifest_path = root / "tests/fixtures/p0_b_five_iwo_manifest.json"
    samples, bursts, valleys = build_samples()
    payload = wav_bytes(samples)
    digest = hashlib.sha256(payload).hexdigest()
    manifest = manifest_for(samples, bursts, valleys, digest)
    manifest_bytes = (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode()

    if args.check:
        if not wav_path.is_file() or wav_path.read_bytes() != payload:
            raise SystemExit("fixture mismatch: regenerate tests/fixtures/p0_b_five_iwo.wav")
        if not manifest_path.is_file() or manifest_path.read_bytes() != manifest_bytes:
            raise SystemExit("manifest mismatch: regenerate p0_b_five_iwo_manifest.json")
        print(f"five-Iwo fixture deterministic: sha256={digest} samples={len(samples)}")
        return 0

    wav_path.write_bytes(payload)
    manifest_path.write_bytes(manifest_bytes)
    print(f"wrote {wav_path.relative_to(root)} sha256={digest}")
    print(f"wrote {manifest_path.relative_to(root)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
