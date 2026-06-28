# CodeScribe - Team Setup (Pure Rust Era)

## Quick Start

### 1. Prerequisites

- macOS 14+ (Apple Silicon ARM64 only)
- Rust 1.83+

### 2. Build & Run (CLI)

```bash
# Clone
git clone git@github.com:VetCoders/CodeScribe.git
cd CodeScribe

# Build and run CLI
cargo build --release -p codescribe
./target/release/codescribe
```

### 3. Development Mode

```bash
# Run debug binary
cargo run
```

## Permissions Required

Grant in: System Settings > Privacy & Security

1. **Microphone** - for audio recording
2. **Accessibility** - for global hotkeys
3. **Input Monitoring** - for hotkey capture

## Hotkeys

| Key                         | Action                         | AI Mode               |
| --------------------------- | ------------------------------ | --------------------- |
| Hold **Fn**                 | Record → paste raw transcript  | ALWAYS RAW (no AI)    |
| Hold **Fn+Shift**           | Record → AI assistant response | ALWAYS Assistive      |
| Hold **Fn+Cmd**             | Send selection + transcript    | Assistive (selection) |
| Double-tap **Left Option**  | Hands‑free toggle (normal)     | Respects AI toggle    |
| Double-tap **Right Option** | Hands‑free toggle (assistive)  | Assistive             |

### Mode Behavior

- **RAW mode (Fn)**: Fast dictation. Transcript is pasted as-is (only local repetition cleanup).
  Ignores AI_FORMATTING_ENABLED setting.
- **Toggle mode (Double Option)**: Respects the AI Formatting toggle. If enabled, sends to AI
  for formatting. If disabled, pastes raw.
- **Assistive mode (Fn+Shift)**: Full AI assistant. Model can answer questions, expand ideas,
  or pass through dictation based on detected intent (KURIER/ASYSTENT system).

## Model

**Embedded-first Whisper policy**: `whisper-large-v3-turbo-mlx-q8`
**Embedded Embedder**: `paraphrase-multilingual-MiniLM-L12-v2` (for semantic gating)

- `core/build.rs` embeds Whisper by default when a complete model is available at build time.
- Runtime fallback resolves Whisper from exactly one shared contract in `core/config/models.rs`:
  `CODESCRIBE_MODEL_PATH` → configured local model path/alias → configured HF repo snapshot →
  default local turbo model → default HF cache snapshot.
- `make install` / `scripts/ensure-models.sh` are the easiest way to warm the expected cache paths.

**Developer note:**
If runtime lookup cannot find the model, point `CODESCRIBE_MODEL_PATH` at a valid Whisper directory.

## CLI Usage

```bash
# Transcribe audio file
codescribe transcribe audio.wav

# With AI formatting
codescribe transcribe audio.wav --format

# Specify language
codescribe transcribe audio.wav --language pl
```

## Quality & Tools

New CLI tools for batch processing and automation:

```bash
# Batch quality report (renamed from codescribe-quality in 0.9.0)
qube-report --help

# Self-improving quality daemon (renamed from codescribe-loop in 0.9.0)
qube-daemon --help
```

## Configuration

File: `~/.codescribe/.env`

```env
USE_LOCAL_STT=1

# Whisper
WHISPER_LANGUAGE=pl

# AI formatting (optional) - OpenAI Responses by default
AI_FORMATTING_ENABLED=1

# Formatting mode - used by cleanup/formatting paths
LLM_FORMATTING_ENDPOINT=https://api.openai.com/v1/responses
LLM_FORMATTING_MODEL=gpt-4.1
LLM_FORMATTING_API_KEY=sk-xxx

# Assistive mode - dictation-driven agent
LLM_ASSISTIVE_ENDPOINT=https://api.openai.com/v1/responses
LLM_ASSISTIVE_MODEL=gpt-5.5
LLM_ASSISTIVE_API_KEY=sk-xxx

# Shared fallback (if mode-specific not set)
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1
LLM_API_KEY=sk-proj-xxx
```

### Custom Prompts

Prompts are loaded from `~/.codescribe/prompts/` at each request (no restart needed):

- `formatting.txt` - System prompt for formatting mode (punctuation, structure)
- `assistive.txt` - System prompt for assistive mode (KURIER/ASYSTENT logic)

Edit these files to customize AI behavior. Changes take effect immediately.

## Quality Assurance

### Local (recommended)

```bash
# Install pre-commit hooks (runs check/fmt on commit, clippy/semgrep on push)
make hooks

# Manual quality gate
make check       # fmt + clippy + unit tests

# E2E tests with real API
make test-sse    # SSE streaming tests (requires ~/.codescribe/.env)
```

### CI (GitHub Actions)

**Note:** Full build requires macOS + Swift 6.0 (CoreML, Metal). GitHub runners have Swift 5.10, so CI only runs:

- **Format check** (`cargo fmt --check`) on Linux
- **Semgrep** security scan on Linux

Clippy and tests run **locally** via pre-commit hooks or `make check`.

For full CI, configure a self-hosted macOS runner (a high-RAM Apple Silicon workstation recommended).

## Troubleshooting

### App doesn't start

- Check Console.app for crash logs
- If building locally: ensure the model exists in `models/` (for embedding at build time)

### Hotkeys don't work

- Grant Accessibility permission
- Grant Input Monitoring permission
- Restart app after granting

### No transcription

- Check `USE_LOCAL_STT=1` in config
- If using local STT: confirm the app is using the embedded engine (default in release builds)

---

_Created by M&K (c)2026 VetCoders_
