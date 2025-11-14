# MLX toolkit cheat sheet (audio • whisper • lm • vlm)

This page collects practical, copy‑pasteable commands for the MLX launchers that are often useful when working with VistaScribe locally. It’s meant as a quick reference so you don’t have to hunt for `--help` every time.

Notes
- Commands here are thin entry points that ship with the MLX ecosystem. They load heavy models — don’t run them inside the tray process.
- Use `uv run …` to execute in the project’s virtualenv.
- Some CLIs depend on optional packages; if a tool is missing, install the corresponding package (see Embeddings section).

## MLX‑Audio (TTS server / TTS generation)

TTS server (starts a local HTTP server and warms STT + VAD models for voice features):

```bash
uv run mlx_audio.server --host 127.0.0.1 --port 8237 --verbose
```

Quick TTS generation from text:

```bash
# Reads from --text or stdin; can join segments, play the output, etc.
uv run mlx_audio.tts.generate \
  --model mlx-community/MetaVoice-1B-v0.1-mlx \
  --text "Hello! This is a speech synthesis test." \
  --voice default --speed 1.0 --gender female --pitch 0.0 \
  --lang_code pl --file_prefix out --audio_format wav --verbose
```

Common flags (from `--help`): `--voice`, `--speed`, `--gender [male|female]`, `--pitch`, `--lang_code`, `--file_prefix`, `--join_audio`, `--play`, `--audio_format`, `--ref_audio`, `--ref_text`, `--stt_model`, `--temperature`, `--top_p`, `--top_k`, `--repetition_penalty`, `--stream`, `--streaming_interval`.

Tip: The server prints warm‑up logs (“Warming up STT/VAD model”). Keep it running in a separate terminal if you plan to do multiple TTS calls.

VistaScribe helper: You can call a thin wrapper that prefers HTTP (if TTS_SERVER_URL is set) and falls back to CLI in a separate process:

```python
from tts import say
import asyncio
asyncio.run(say("Hello! This is a speech synthesis demo.", play=True))
```

## MLX‑Whisper (STT)

VistaScribe already uses `mlx_whisper` for local STT. For ad‑hoc transcription outside the app you can call it programmatically or via Python module entry points. Example Python usage:

```python
import mlx_whisper as whisper
from mlx_whisper.load_models import load_model

model = load_model("./models/whisper-large-v3-turbo")
result = whisper.transcribe("sample.wav", path_or_hf_repo="./models/whisper-large-v3-turbo", language="pl")
print(result["text"])
```

Model dirs work both as absolute paths and repo‑relative paths. On macOS some absolute paths are case‑sensitive in MLX; VistaScribe normalizes `/Users` → `/users` to avoid issues.

## MLX‑LM (text LLMs)

One‑shot generation:

```bash
uv run mlx_lm.generate \
  --model mlx-community/Llama-3.2-3B-Instruct-4bit \
  --prompt "Format this text: this is an example without commas or periods" \
  --max-tokens 128 --temp 0.2
```

Open a local HTTP server (useful for experimentation; separate from VistaScribe’s own backend):

```bash
uv run mlx_lm.server --host 127.0.0.1 --port 8320 --model mlx-community/Llama-3.2-3B-Instruct-4bit
```

Other helpful tools:

```bash
uv run mlx_lm.perplexity --model <MODEL>
uv run mlx_lm.benchmark  --model <MODEL> --batch-size 1 --num-trials 3
uv run mlx_lm.manage      # download / list models (interactive)
```

Quantization and conversion (advanced):

```bash
uv run mlx_lm.awq  --model <HF_OR_PATH> --bits 4
uv run mlx_lm.gptq --model <HF_OR_PATH>
uv run mlx_lm.dwq  --model <HF_OR_PATH>
uv run mlx_lm.convert --model <HF_OR_PATH> --mlx-path ./models/<name>-mlx
```

## MLX‑VLM (vision‑language)

```bash
uv run mlx_vlm.generate --model <MODEL> --image <IMG_PATH> --prompt "Describe the picture"
uv run mlx_vlm.server   --host 127.0.0.1 --port 8321 --model <MODEL>
uv run mlx_vlm.convert  --model <HF_OR_PATH> --output ./models/<name>-vlm-mlx
```

## Embeddings

If `mlx_embeddings` isn’t available (ModuleNotFoundError), install the package:

```bash
uv add mlx-embeddings
```

Then:

```bash
uv run mlx_embeddings --help
# or example usage (varies by version):
uv run mlx_embeddings --model <MODEL> --input "The quick brown fox jumps over the lazy dog"
```

## How this maps to VistaScribe

- Local STT (Whisper): handled in‑process by VistaScribe with lazy loading to keep the tray light. You can point to a remote STT backend via `WHISPER_SERVER_URL`.
- Local formatting (LLM): also lazy‑loaded; or you can point to a remote formatter via `LLM_SERVER_URL`.
- TTS: not required by VistaScribe, but you can run `mlx_audio.server` for optional voice playback features. We will expose a thin optional client guarded by `TTS_SERVER_URL` in a later step.

Default behavior: formatting is enabled with the `light_plus` strategy unless you set `FORMAT_ENABLED=0` or change the strategy in the app.

Environment variables used in VistaScribe today
- WHISPER_SERVER_URL: if set, audio is sent to that FastAPI endpoint instead of local Whisper.
- LLM_SERVER_URL: if set, formatting requests go to that server instead of local MLX‑LM.
- WHISPER_DIR / WHISPER_VARIANT: local model selection (e.g., `whisper-large-v3-turbo`).
- FORMAT_ENABLED, FORMAT_STRATEGY: enable/choose formatting (`light`, `light_plus`, `llm`, `openai`).

Troubleshooting
- If a CLI prints ModuleNotFoundError, install its package in the venv (e.g., `uv add mlx-audio`, `uv add mlx-lm`, `uv add mlx-embeddings`).
- macOS Gatekeeper/quarantine can block scripts — see README Troubleshooting section.
- On Apple Silicon, MLX uses Metal (AGX); first run may compile kernels.


## Polish Whisper fine‑tunes (Small/Medium)

Lightweight Polish ASR models you can try:
- bardsai/whisper-medium-pl — https://huggingface.co/bardsai/whisper-medium-pl
- bardsai/whisper-small-pl — https://huggingface.co/bardsai/whisper-small-pl
- Collection (overview): https://huggingface.co/collections/bardsai/polish-whisper-659dec07b65ee9ee1fbbcc63

Using them with VistaScribe:
- VistaScribe expects a local MLX‑format directory for Whisper (what `mlx_whisper.load_model(path)` loads).
- Place the converted model under `models/whisper-medium-pl` (or `models/whisper-small-pl`).
- Set `WHISPER_VARIANT=medium-pl` (or `small-pl`) and/or `WHISPER_DIR` to the directory.
- Remote mode (`WHISPER_SERVER_URL`) bypasses local models.

Notes:
- If your HF repo provides Transformers weights, convert to an MLX directory first. See the MLX community Whisper collection for reference:
  https://huggingface.co/collections/mlx-community/whisper-663256f9964fbb1177db93dc
- Quality vs size: Medium‑PL generally outperforms Small‑PL at the cost of more RAM. Choose based on your deployment target.
