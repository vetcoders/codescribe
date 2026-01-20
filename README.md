# ⌜ CodeScribe ⌟

**Native macOS Audio Intelligence Platform — Embedded Whisper Live, Quality Loop & Semantic Postprocessing.**

## Overview

CodeScribe is a native macOS menu-bar application that captures audio through global hotkeys, transcribes it locally
using Whisper with Metal GPU acceleration, and pastes the transcript directly into the focused application. Optional AI
formatting via LLM polishes the output while keeping everything private and local.

```mermaid
flowchart TB
    %% Minimal monochrome styling
    classDef default fill:#fff,stroke:#333,stroke-width:1px;
    classDef box fill:#fafafa,stroke:#666,stroke-width:1px,stroke-dasharray: 0;

    subgraph APP[CodeScribe Application]
        direction TB

        subgraph UI[Leptos WASM Frontend]
            direction LR
            VL[Voice Lab] --- TE[Teacher] --- SET[Settings]
        end

        subgraph BACKEND[Tauri Rust Backend]
            CMD[Command Handlers]
        end

        UI -->|IPC invoke| BACKEND

        subgraph CORE[Core Library]
            direction TB
            REC[Streaming Recorder]
            POST[Stream Postprocess]
            WH[Whisper Engine]
            IPC[IPC Server]
            QL[Quality Loop]

            REC -->|Live Chunks| POST
            POST -->|Semantic Gating| WH
            WH -->|Transcript| IPC
            QL -.->|Self-Improvement| WH
        end

        BACKEND --> CORE
    end

    MODEL[Embedded Whisper Model\nlarge-v3-turbo-mlx-q8\n(~888MB)]
    WH === MODEL

    subgraph TOOLS[CLI Suite]
        QCLI[codescribe-quality]
        LCLI[codescribe-loop]
    end

    CORE -.-> TOOLS

    class APP,UI,BACKEND,CORE,TOOLS box
```

> **Note:** The diagram above shows the **target architecture** with Tauri GUI. Current release is a **native macOS tray app** (without Tauri). See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for current implementation details.

> **Status:** current release (see `Cargo.toml`) — **Strictly Embedded Model** (~888MB binary, zero exceptions) + *Whisper Live* (streaming transcription).

See: [`docs/WHISPER_LIVE.md`](docs/WHISPER_LIVE.md) | [`docs/BACKLOG.md`](docs/BACKLOG.md) | [`docs/ARCHITECTURE_VISION.md`](docs/ARCHITECTURE_VISION.md)

## API Provider

CodeScribe uses the **Responses API** (`/v1/responses`) for AI formatting. Compatible with OpenAI, LibraxisAI,
Anthropic, and any provider supporting this format.

### Multi-Provider Setup (Recommended)

Use different providers for different modes — e.g., cheaper model for formatting, powerful model for assistive:

```env
# ~/.codescribe/.env

# Shared defaults
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
LLM_API_KEY=sk-proj-xxx

# Formatting mode overrides (Ctrl hold - cleanup only)
LLM_FORMATTING_ENDPOINT=https://api.libraxis.cloud/v1/responses
LLM_FORMATTING_MODEL=gpt-4.1-mini
LLM_FORMATTING_API_KEY=vista-xxx

# Assistive mode overrides (Ctrl+Shift - AI augmentation)
LLM_ASSISTIVE_ENDPOINT=https://api.openai.com/v1/responses
LLM_ASSISTIVE_MODEL=gpt-4.1
LLM_ASSISTIVE_API_KEY=sk-proj-xxx
```

> **Note:** All requests use `previous_response_id` for conversation chaining. Context persists across transcriptions.

## Features

- **Pure Rust Implementation** — Native macOS app built entirely in Rust with candle-core + Metal GPU
- **Strictly Embedded Whisper** — Model is welded into the binary (~888MB). No external files, zero disk I/O, no exceptions.
- **Whisper Live** — Streaming transcription happens *during recording* (chunks + overlap), so `stop()` is
  near-instant
- **Stream postprocess** — semantic gating + cleanup of live chunks before final output
- **IPC Server** — Stable runtime interface for GUI/clients
- **Quality Loop + Report** — Automated quality scoring and batch reports
- **CLI Suite** — `codescribe`, `codescribe-quality`, `codescribe-loop`
- **Metal GPU Acceleration** — Hardware-accelerated inference on Apple Silicon
- **System Tray App** — Minimal menu-bar presence with animated status glyphs
- **Hands-off Chat Overlay** — Live transcription and AI responses in a non-intrusive overlay window with chat history and auto-send capability
- **Global Hotkeys** — Hold Ctrl or double-tap Option to record
- **Provider Separation** — Different LLM providers for formatting vs assistive mode
- **AI Formatting** — Optional post-processing via Responses API
- **Slug Filenames** — Transcripts named with first 3 words for easy identification

## Tech Stack

| Component        | Technology                        | Purpose                    |
|------------------|-----------------------------------|----------------------------|
| Language         | Rust 2024 Edition                 | Native performance         |
| ML Framework     | candle-core + candle-transformers | Whisper inference          |
| GPU Acceleration | Metal (Apple Silicon)             | Hardware-accelerated STT   |
| System Tray      | tray-icon + muda + tao            | Menu bar app               |
| Hotkeys          | CGEventTap (core-graphics)        | Global key detection       |
| Audio            | cpal + hound + symphonia          | Recording & format support |
| HTTP Client      | reqwest                           | LLM API calls              |
| API Format       | openai-harmony                    | Responses API support      |
| Security         | cap-std                           | Path safety hardening      |
| Embeddings       | fastembed                         | Local vector utilities     |

## Installation

### Prerequisites

- **macOS 14+** (Sonoma or later)
- **Apple Silicon** (M1, M2, M3, or later)
- **Rust Toolchain** (1.85+ with edition 2024 support)

### Install from Source

```bash
# Clone the repository
git clone https://github.com/VetCoders/CodeScribe.git
cd CodeScribe

# Download Whisper model (required for embedding)
make download-model

# Install CLI (~888MB with embedded model)
make install

# Verify installation
codescribe --version
```

### Build Options

```bash
make build              # Debug build (external model)
make release            # Release build (embedded model)
make install            # Install with embedded model (~888MB)
make install-no-embed   # Dev-only: install without embedding (needs CODESCRIBE_MODEL_PATH)
```

## Quick Start

```bash
# Start tray app
codescribe

# Open/create config file
make config
# or: codescribe --config

# Verbose logging
codescribe -v

# CLI transcription
codescribe transcribe audio.wav
codescribe transcribe -l pl audio.m4a
codescribe transcribe -f audio.mp3  # with AI formatting
```

## How It Works

```mermaid
flowchart TD
    A[Hotkey Press] --> B{Mode?}
    B -->|Hold Ctrl| C[Start Recording]
    B -->|Double Option| C
    C --> D[Recording]
    D -->|live chunks| E[Whisper STT (streaming)]
    D -->|Release / Toggle| F[Stop]
    F --> G[Finalize last chunk]
    G --> H{AI Enabled?}
    H -->|Yes| I[LLM Formatting]
    H -->|No| J[Raw Transcript]
    I --> K[Paste to Active App]
    J --> K

    E -.- E1[Metal GPU • embedded model]
    I -.- I1[Responses API • previous_response_id]
```

### Recording Modes

| Mode                  | Trigger                    | Description                                    |
|-----------------------|----------------------------|------------------------------------------------|
| **Hold-to-talk**      | Hold `Ctrl` (800ms delay)  | Release to transcribe + paste (raw transcript) |
| **Hold Assistive**    | Hold `Ctrl+Shift`          | AI augmentation mode                           |
| **Toggle Formatting** | Double-tap `Left Option`   | AI-formatted version of speech                 |
| **Toggle Assistive**  | Double-tap `Right Option`  | Augmented AI response                          |

See [`docs/BACKLOG.md`](docs/BACKLOG.md) for detailed mode descriptions and future enhancements (VAD, Overlay).

## Configuration

Config file: `~/.codescribe/.env`

```bash
# Create/edit config
make config
```

### Environment Variables

```env
# STT (Speech-to-Text)
WHISPER_LANGUAGE=pl                  # pl | en | de | fr (no auto!)
# CODESCRIBE_MODEL_PATH=             # Override embedded model

# Hotkeys
HOLD_MODS=ctrl                       # ctrl | ctrl_alt | ctrl_shift | ctrl_cmd
TOGGLE_TRIGGER=double_option         # double_option | double_ralt | none
HOLD_START_DELAY_MS=800              # Delay before recording starts

# AI Formatting
AI_FORMATTING_ENABLED=1              # 1=format via LLM, 0=raw transcript

# LLM Provider (shared defaults)
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
LLM_API_KEY=sk-proj-xxx

# Provider separation (optional)
# LLM_FORMATTING_{ENDPOINT,MODEL,API_KEY}=
# LLM_ASSISTIVE_{ENDPOINT,MODEL,API_KEY}=

# History
HISTORY_ENABLED=1                    # Save transcripts
DUMP_AUDIO_LOGS=0                    # 1=save .wav paired with .txt

# Audio
BEEP_ON_START=1
SOUND_VOLUME=0.5
# AUDIO_INPUT_DEVICE=                # Specific device name

# Logging
LOG_LEVEL=INFO                       # TRACE | DEBUG | INFO | WARN | ERROR
```

See `.env.example` for complete reference.

## CLI Reference

### `codescribe` (Tray App)

Main application — runs as menu bar app with global hotkeys.

```bash
codescribe [OPTIONS]

Options:
  -v, --verbose      Enable verbose logging
  --config           Create/edit config file
  --version          Show version
  -h, --help         Show help
```

### `codescribe transcribe`

CLI transcription without tray app.

```bash
codescribe transcribe FILE [OPTIONS]

Arguments:
  FILE               Audio file (WAV, MP3, M4A)

Options:
  -l, --language     Language hint (pl, en, de, fr)
  -f, --format       Apply AI formatting
  -m, --model        Model name (if using external)
  --llm              LLM model for formatting
  -h, --help         Show help
```

## Model

CodeScribe uses **whisper-large-v3-turbo-mlx-q8**:

- 4-layer turbo architecture (vs 32 layers in full model)
- Q8 quantization (~894MB weights)
- ~10x faster than whisper-large-v3
- Metal GPU acceleration

### Embedded Model (Default)

Release builds include the model via `include_bytes!`:

```bash
cargo build --release          # ~888MB binary with model
CODESCRIBE_NO_EMBED=1 cargo build --release  # Dev-only experiment (not supported for distribution)
```

### External Model (Development)

For development or custom models:

1. `CODESCRIBE_MODEL_PATH` environment variable
2. `~/.codescribe/models/whisper-large-v3-turbo-mlx-q8/`
3. `./models/whisper-large-v3-turbo-mlx-q8/`

Model files required:

- `config.json`
- `weights.safetensors`
- `tokenizer.json`
- `mel_filters.npz`

## Architecture

```
CodeScribe/
├── codescribe-core/           # Core library (Whisper, audio, config, quality)
│   ├── src/
│   │   ├── lib.rs             # Core exports
│   │   ├── whisper/           # Embedded Whisper engine
│   │   ├── audio/             # Recorder + streaming
│   │   ├── config/            # Config + prompts
│   │   ├── quality_loop.rs    # Self-improvement loop
│   │   └── ...
├── src/
│   ├── lib.rs                 # App exports (macOS tray/hotkeys/UI)
│   ├── main.rs                # CLI entry point
│   ├── controller.rs          # Recording/transcription orchestration
│   ├── tray/                  # Tray menu + handlers
│   ├── hotkeys.rs             # CGEventTap hotkey handler
│   └── ...
├── models/                    # Whisper model files (build-time only)
├── tests/                     # Unit + E2E tests
└── docs/
    ├── WHISPER_LIVE.md        # Embedded + streaming transcription (DONE)
    └── ARCHITECTURE.md        # Technical documentation
```

## Development

```bash
# Clone and setup
git clone https://github.com/VetCoders/CodeScribe.git
cd CodeScribe

# Development build (external model)
CODESCRIBE_MODEL_PATH=./models/whisper-large-v3-turbo-mlx-q8 cargo run

# Quality checks
make lint           # clippy + fmt check
make test           # Unit + integration tests
make check          # Full quality gate

# Formatting
make format         # cargo fmt

```

### Makefile Targets

```
make build            # Debug build
make release          # Release build (embedded model)
make install          # Install CLI (~888MB)
make install-no-embed # Dev-only: install without embedding
make config           # Edit ~/.codescribe/.env
make start            # Start as daemon
make stop             # Stop running instance
make logs             # View logs
make lint             # Clippy + format check
make test             # Run tests
make check            # Full quality gate
make download-model   # Download Whisper model
```

## Code Quality

| Tool           | Purpose    | Config            |
|----------------|------------|-------------------|
| **Clippy**     | Linting    | `-D warnings`     |
| **rustfmt**    | Formatting | Rust 2024 edition |
| **cargo test** | Testing    | Unit + E2E        |

## Permissions

CodeScribe requires macOS permissions for:

- **Microphone** — Audio recording
- **Accessibility** — Global hotkey detection
- **Input Monitoring** — Keyboard event capture

Grant permissions in System Settings > Privacy & Security when prompted.

## Roadmap

### Implemented

- [x] Local Whisper STT (Metal GPU)
- [x] Embedded model in binary (~888MB)
- [x] Global hotkeys (CGEventTap)
- [x] AI formatting (Responses API)
- [x] Provider separation (formatting/assistive)
- [x] Conversation chaining (previous_response_id)
- [x] Tray app with submenus
- [x] CLI transcribe command
- [x] History with slug filenames
- [x] Keep Audio toggle
- [x] CodeScribe Core separation (`codescribe-core` crate)
- [x] Quality Loop & Quality Report CLI tools

### In Progress

- [ ] Voice Activity Detection (VAD) for auto-stop — *implemented but not integrated*
- [ ] Overlay text preview — *code exists, not fully integrated*

### Planned

- [ ] Hands-off mode with VAD + Overlay integration
- [ ] Tauri GUI (Voice Lab, Teacher, Settings)
- [ ] TTS integration for assistive mode
- [ ] Libraxis Qube Protocol (WebSocket streaming architecture)
- [ ] Custom prompt editing in GUI
- [ ] More languages for prompts
- [ ] DMG distribution with notarization

See [`docs/BACKLOG.md`](docs/BACKLOG.md) for detailed backlog and [`docs/ARCHITECTURE_VISION.md`](docs/ARCHITECTURE_VISION.md) for future architecture.

## License

Apache License 2.0

---

**Made with (งಠ_ಠ)ง by the ⌜ VetCoders ⌟ 𝖙𝖊𝖆𝖒 (c) 2024-2026
Maciej & Monika + Klaudiusz (AI) + Junie (AI)**
