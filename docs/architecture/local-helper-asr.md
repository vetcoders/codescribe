# Local ASR helper and Qwen evaluation contract

Codescribe's stock GUI process owns Apple Speech and the lexicon. Optional local
weights belong to a separate, killable Layer 1 helper. `LocalHelperAsrSession`
is the provider-compatible boundary: the power-user runtime injects a launcher,
and process exit plus wait/reap is the only reclaim proof.

The lifecycle is `Stopped -> Starting -> Ready -> Cooling -> Stopped`. A failed
spawn, handshake, PCM push, or shutdown is a Layer 1 failure. The recorder keeps
Apple + lexicon and never loads Candle/Whisper as a surprise fallback. There is
no local helper selected by default and no model runtime linked by this cut.

## Opt-in Qwen3-ASR-0.6B q5/q8 benchmark

The harness downloads nothing and retains no audio, reference, or hypothesis in
its output:

```bash
python3 scripts/bench-qwen-local-helper.py \
  --models /private/path/qwen-models.json \
  --corpus /private/path/pl-vet-corpus.json \
  --runner /private/path/codescribe-qwen-runner \
  --out /private/path/qwen-local-helper-result.json
```

The models manifest contains exactly q5 and q8 entries:

```json
{
  "models": [
    {
      "model_id": "Qwen/Qwen3-ASR-0.6B",
      "quantization": "q5",
      "revision": "exact-revision",
      "sha256": "optional-expected-sha256",
      "license": "verified-license",
      "artifact_path": "/private/q5"
    },
    {
      "model_id": "Qwen/Qwen3-ASR-0.6B",
      "quantization": "q8",
      "revision": "exact-revision",
      "sha256": "optional-expected-sha256",
      "license": "verified-license",
      "artifact_path": "/private/q8"
    }
  ]
}
```

The private corpus manifest points to external WAV and reference files; it is
never copied into this repository:

```json
{
  "cases": [
    {
      "id": "vet-pl-01",
      "audio_path": "/private/01.wav",
      "reference_path": "/private/01.txt",
      "terms": ["term weterynaryjny"],
      "tags": ["pl_vet", "pl_en_code_switch"]
    }
  ]
}
```

For each case the injected runner accepts `--model PATH --audio PATH --json`
and returns one JSON object on stdout:

```json
{
  "transcript": "in-memory only",
  "cold_load_seconds": 1.2,
  "audio_seconds": 8.0,
  "segments": [{ "start_ms": 0, "end_ms": 8000 }]
}
```

The result schema is
`scripts/schemas/qwen-local-helper-bench-result.schema.json`. It records exact
revision, computed checksum, declared license, PL/veterinary WER and CER, term
recall, PL-EN code-switch WER, timestamp presence/monotonicity, cold load, RTF,
peak RSS, post-exit RSS, confirmed process exit, and bundle size. A missing
private corpus, runner, or model is an unverified host bench, never a fabricated
number and never a hermetic gate failure.

Qwen3-ASR-0.6B remains an unproven candidate until both quantizations complete
this bench. Parakeet remains plan B. Cohere 2B remains batch-only until measured
evidence establishes a live timestamp and code-switch contract. No default is
selected here.
