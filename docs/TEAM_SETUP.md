# Codescribe - Team Setup (Rust Core + SwiftUI App)

## Quick Start

### 1. Prerequisites

- macOS 14+ (Apple Silicon ARM64 only)
- Rust 1.83+

### 2. Build & Run (Native App)

```bash
# Clone
git clone git@github.com:Vetcoders/Codescribe.git
cd codescribe

# Build and install the SwiftUI app over the Rust UniFFI core
make app PROFILE=release
make install-app
make start
```

### 3. Development Mode

```bash
# Build and launch the debug app bundle
make app PROFILE=debug
open macos/build/Build/Products/Debug/Codescribe.app
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
| Hold **Fn+Shift** (default arm) | Record → agent voice chat  | ALWAYS Assistive      |
| Hold **Fn+Cmd** (arm alt)   | Same arm when Cmd is selected in Settings | ALWAYS Assistive |
| Armed hold **with selection** | Selection transform lane     | Assistive (selection) |
| Double-tap **Left Option**  | Hands‑free toggle (normal)     | Respects AI toggle    |
| Double-tap **Right Option** | Hands‑free toggle (assistive)  | Assistive             |

### Mode Behavior

- **RAW mode (Fn)**: Fast dictation. Transcript is pasted as-is (only local repetition cleanup).
  Ignores AI_FORMATTING_ENABLED setting.
- **Toggle mode (Double Option)**: Respects the AI Formatting toggle. If enabled, sends to AI
  for formatting. If disabled, pastes raw.
- **Assistive arm (default Fn+Shift; optional Cmd in Settings)**: Sends voice to the agent.
  Without a selection this is voice-chat (spoken text, agent persona). With a selection it
  is act-on-selection (assistive skeleton + `assistive.txt`).

## Model

**Embedded-first Whisper policy**: `whisper-large-v3-turbo-mlx-q8`
**Embedded Embedder**: `paraphrase-multilingual-MiniLM-L12-v2` (for semantic gating)

- `core/build.rs` embeds Whisper by default when a complete model is available at build time.
- Runtime fallback resolves Whisper from exactly one shared contract in `core/config/models.rs`:
  `CODESCRIBE_MODEL_PATH` → configured local model path/alias → configured HF repo snapshot →
  default local turbo model → default HF cache snapshot.
- `make install-app` / `scripts/ensure-models.sh` are the easiest way to warm the expected cache paths.

**Developer note:**
If runtime lookup cannot find the model, point `CODESCRIBE_MODEL_PATH` at a valid Whisper directory.

## Qube CLI Utilities

The app path is the SwiftUI bundle. Terminal utilities are limited to batch quality/reporting tools:

```bash
qube-report --help
qube-daemon --help
```

## Quality & Tools

New CLI tools for batch processing and automation:

```bash
# Batch quality report
qube-report --help

# Quality daemon
qube-daemon --help
```

## Configuration

File: `~/.codescribe/.env`

```env
USE_LOCAL_STT=1

# Whisper
WHISPER_LANGUAGE=auto

# AI formatting (optional) - OpenAI Responses by default
AI_FORMATTING_ENABLED=1

# Formatting mode - used by cleanup/formatting paths
LLM_FORMATTING_ENDPOINT=https://api.openai.com/v1/responses
LLM_FORMATTING_MODEL=gpt-4.1
# Store LLM_FORMATTING_API_KEY in Settings / macOS Keychain.

# Assistive mode - dictation-driven agent
LLM_ASSISTIVE_ENDPOINT=https://api.openai.com/v1/responses
LLM_ASSISTIVE_MODEL=gpt-5.5
# Store LLM_ASSISTIVE_API_KEY in Settings / macOS Keychain.

# Shared fallback (if mode-specific not set)
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1
# Store LLM_API_KEY in Settings / macOS Keychain.
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

_Created by vetcoders (c)2026_
