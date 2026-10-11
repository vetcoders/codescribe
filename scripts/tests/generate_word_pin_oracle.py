#!/usr/bin/env python3
"""Generate synthetic attention fixtures; never load audio or a speech model.

The frontend is an independent NumPy rendition of the pinned Torch expressions.
DTW/backtrace execute the pinned OpenAI function bodies with only JIT decorators
removed. These fixtures verify alignment mathematics, not native ASR quality.
"""

import argparse
import ast
import hashlib
import json
from pathlib import Path

import numpy as np

REFERENCE_COMMIT = "679ae1d14167541384b4e732f80847e1c5095b19"
REFERENCE_SHA256 = "bb091c973c428697012f0f8d1b4f5f3a605cd93c5d1c129957f712a6a878a59c"


def reference_dtw(source):
    raw = source.read_bytes()
    assert hashlib.sha256(raw).hexdigest() == REFERENCE_SHA256
    tree = ast.parse(raw)
    functions = []
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name in {"backtrace", "dtw_cpu"}:
            node.decorator_list = []
            functions.append(node)
    assert len(functions) == 2
    namespace = {"np": np}
    exec(
        compile(ast.Module(body=functions, type_ignores=[]), str(source), "exec"),
        namespace,
    )
    return namespace["dtw_cpu"]


def oracle(heads, prefix, counts, frames, dtw):
    weights = np.asarray(heads, dtype=np.float32)[..., :frames]
    weights = np.exp(weights - weights.max(axis=-1, keepdims=True))
    weights /= weights.sum(axis=-1, keepdims=True)
    # OpenAI uses population std and includes prefix + EOT at this step.
    weights = (weights - weights.mean(axis=-2, keepdims=True)) / weights.std(
        axis=-2, keepdims=True, ddof=0
    )
    if frames > 3:
        padded = np.pad(weights, ((0, 0), (0, 0), (3, 3)), mode="reflect")
        weights = np.median(
            np.lib.stride_tricks.sliding_window_view(padded, 7, axis=-1), axis=-1
        )
    matrix = weights.mean(axis=0)[prefix:-1]
    text, time = dtw(-matrix.astype(np.float64))
    jumps = np.pad(np.diff(text), (1, 0), constant_values=1).astype(bool)
    jump_frames = time[jumps]
    boundaries = np.pad(np.cumsum(counts), (1, 0))
    return [
        [int(jump_frames[a]), int(jump_frames[b])]
        for a, b in zip(boundaries[:-1], boundaries[1:])
    ]


def attention(prefix, token_count, frames, heads=1):
    rows = prefix + token_count + 2
    output = (
        np.random.default_rng(760)
        .uniform(-0.3, 0.3, (heads, rows, frames))
        .astype(np.float32)
    )
    for head in range(heads):
        for row in range(rows):
            # Fixed distinct prefix/EOT distractors, predictive rows in order.
            if row < prefix:
                center = 80 + row * 7
            elif row == rows - 1:
                center = 100
            else:
                center = 6 + (row - prefix) * 12 + head
            start = min(center, frames - 7)
            output[head, row, start : start + 7] = 8.0
    return output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("reference", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    dtw = reference_dtw(args.reference)
    cases = []
    for name, counts, words, heads, frames, content in [
        ("single_final_word_before_silence", [1], ["rację?"], 1, 120, 120),
        ("five_iwo_occurrences", [1] * 5, ["Iwo"] * 5, 1, 120, 120),
        ("last_word_three_tokens", [2, 3], ["mam", "rację?"], 1, 120, 120),
        ("two_alignment_heads", [1, 2, 2], ["nie", "mam", "racji"], 2, 120, 120),
        ("padded_frames_are_excluded", [1] * 5, ["Iwo"] * 5, 1, 180, 120),
        ("long_trailing_silence", [1] * 5, ["Iwo"] * 5, 1, 180, 180),
    ]:
        qk = attention(3, sum(counts), frames, heads)
        if content < frames:
            qk[..., content:] = 40.0  # padding must not affect softmax or DTW
        expected = oracle(qk, 3, counts, content, dtw)
        assert all(end > start for start, end in expected), (name, expected)
        assert expected[-1][1] < content - 10, (name, expected)
        cases.append(
            dict(
                name=name,
                prefix=3,
                counts=counts,
                words=words,
                content_frames=content,
                heads=qk.tolist(),
                expected_frames=expected,
            )
        )
    payload = dict(
        schema="codescribe.word-pin-oracle.v1",
        reference_commit=REFERENCE_COMMIT,
        reference_sha256=REFERENCE_SHA256,
        frontend="NumPy float32 translation of pinned Torch expressions; no model inference",
        dtw="Pinned backtrace/dtw_cpu function bodies, JIT decorators removed",
        cases=cases,
    )
    args.output.write_text(
        json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n"
    )
    print(json.dumps({case["name"]: case["expected_frames"] for case in cases}))


if __name__ == "__main__":
    main()
