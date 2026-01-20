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

| Key                        | Action                                  | AI Mode            |
|----------------------------|-----------------------------------------|--------------------|
| Hold **Ctrl**              | Record → paste raw transcript           | ALWAYS RAW (no AI) |
| Hold **Ctrl+Shift**        | Record → AI assistant response          | ALWAYS Assistive   |
| Double-tap **Option**      | Toggle recording (hands-free)           | Respects AI toggle |
| Triple-tap **Option**      | Toggle AI Formatting on/off             | Shows toast        |
| **Shift** during Ctrl hold | Upgrade to Assistive mode mid-recording | —                  |

### Mode Behavior

- **RAW mode (Ctrl)**: Fast dictation. Transcript is pasted as-is (only local repetition cleanup).
  Ignores AI_FORMATTING_ENABLED setting.
- **Toggle mode (Double Option)**: Respects the AI Formatting toggle. If enabled, sends to AI
  for formatting. If disabled, pastes raw.
- **Assistive mode (Ctrl+Shift)**: Full AI assistant. Model can answer questions, expand ideas,
  or pass through dictation based on detected intent (KURIER/ASYSTENT system).

## Model

**Strictly Embedded (Release Policy)**: `whisper-large-v3-turbo-mlx-q8` (~888MB)

- **Zero Exceptions:** Release binaries ALWAYS contain the model.
- **No external files:** We never bundle `Resources/models/*`.
- **Zero I/O:** Model loads from memory directly to Metal.

**Developer note (Build Time):**
You still need the model files locally to *build* the app (because they are `include_bytes!`-ed into the binary).

```bash
make download-model  # Required for build
```

Location (build-time only): `models/whisper-large-v3-turbo-mlx-q8/`

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
# Batch quality report
codescribe-quality --help

# Self-improving quality loop
codescribe-loop --help
```

## Configuration

File: `~/.codescribe/.env`

```env
USE_LOCAL_STT=1

# Whisper
WHISPER_LANGUAGE=pl

# AI formatting (optional) - separate providers for formatting vs assistive
AI_FORMATTING_ENABLED=1

# Formatting mode (fast, cheap) - for Ctrl Hold with AI toggle
LLM_FORMATTING_ENDPOINT=https://api.libraxis.cloud/v1/responses
LLM_FORMATTING_MODEL=gpt-5-mini
LLM_FORMATTING_API_KEY=sk-xxx

# Assistive mode (smart) - for Ctrl+Shift Hold
LLM_ASSISTIVE_ENDPOINT=https://api.libraxis.cloud/v1/responses
LLM_ASSISTIVE_MODEL=gpt-5.2
LLM_ASSISTIVE_API_KEY=sk-xxx

# Shared fallback (if mode-specific not set)
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
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

For full CI, configure a self-hosted macOS runner (Dragon recommended).

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
*Created by M&K (c)2026 VetCoders*
