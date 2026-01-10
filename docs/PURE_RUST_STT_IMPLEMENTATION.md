# Pure Rust STT Implementation for CodeScribe

> **Assignment for: Implementation Agent**
> **Created by: Klaudiusz (Claude Opus 4.5)**
> **Date: 2026-01-10**
> **Project: CodeScribe - VetCoders**

---

## Context & Background

### What is CodeScribe?

CodeScribe is a **speech-to-text tray application for macOS** built in Rust. It allows users to dictate text using global hotkeys (hold Ctrl to record, release to transcribe).

**Current repository:** `/Users/maciejgad/hosted/VetCoders/CodeScribe`

### Architecture Evolution

| Phase | STT Backend | Status |
|-------|-------------|--------|
| v1 | Python server + MLX Whisper (local) | ✅ Worked |
| v2 | HTTP/WebSocket to LibraxisAI cloud | ✅ Current |
| v3 | **Pure Rust with ANE acceleration** | 🎯 Your task |

### Why Pure Rust?

1. **Zero Python dependency** - users don't need Python/venv installed
2. **Single binary distribution** - everything bundled in one .app
3. **ANE acceleration** - Apple Neural Engine is faster than GPU for inference
4. **Tighter integration** - no HTTP overhead, direct function calls

---

## Research Summary (Already Completed)

### Available Rust Crates for STT

| Crate | Version | Backend | Speed (1min audio) | Notes |
|-------|---------|---------|-------------------|-------|
| whisper-rs | 0.15.1 | Metal GPU (whisper.cpp) | ~5s | C++ dependency |
| **candle-coreml** | **0.3.1** | **ANE (Neural Engine)** | **~3s** | ✅ Recommended |
| coreml-rs | 0.5.4 | CoreML via swift-bridge | ~3s | Lower level |
| mlx-rs | 0.25.3 | Apple MLX framework | TBD | For MLX models |

### Benchmarks (whisper.coreml on M1 Air, 1-min audio)

| Configuration | Transcription Time |
|---------------|-------------------|
| OpenAI Whisper CPU | 21s |
| CoreML (GPU) | 5.5s |
| **CoreML (ANE)** | **3.1s** ← 7x faster |

### Existing Infrastructure in LIBRAXIS/lbrx

**Path:** `/Users/maciejgad/LIBRAXIS/lbrx`

This is VetCoders' own ML toolkit with:
- `crates/metal/` - Metal GPU bindings (buffer pools, compute pipelines, shaders)
- `mlx-rs = "0.25.1"` dependency - MLX framework integration
- Tensor engine with zero-copy unified memory
- wgpu + Metal backend

**Key files to study:**
- `/Users/maciejgad/LIBRAXIS/lbrx/crates/metal/src/device.rs` - HybridComputeDevice
- `/Users/maciejgad/LIBRAXIS/lbrx/crates/metal/src/context.rs` - MetalContext
- `/Users/maciejgad/LIBRAXIS/lbrx/Cargo.toml` - workspace setup example

### Model Conversion

Whisper models need to be converted to CoreML format (.mlmodelc):

**Tool:** https://github.com/wangchou/whisper.coreml
```bash
./convert_coreml.sh base 1  # base model, beam_size=1
```

**Pre-converted models on HuggingFace:**
- `mlx-community/whisper-tiny-mlx`
- `mlx-community/whisper-base-mlx`
- `mlx-community/whisper-small-mlx`
- `mlx-community/whisper-large-v3-mlx`

---

## Current CodeScribe Architecture

### Key Files to Understand

| File | Purpose | Relevance |
|------|---------|-----------|
| `src/client.rs` | HTTP/WebSocket client to STT endpoints | **Replace with local inference** |
| `src/audio.rs` | Audio capture with cpal | Keep as-is |
| `src/backend.rs` | Python subprocess manager | **Remove in Pure Rust** |
| `src/controller.rs` | Main orchestration logic | Modify to use local STT |
| `src/hotkeys.rs` | CGEventTap global hotkeys | Keep as-is |
| `src/config/` | Configuration management | Add model path config |

### Current STT Flow

```
1. User presses Ctrl (hotkeys.rs)
2. Audio recording starts (audio.rs)
3. User releases Ctrl
4. Audio sent to cloud via HTTP/WebSocket (client.rs)
5. Transcript received and pasted to clipboard
```

### Target STT Flow (Pure Rust)

```
1. User presses Ctrl (hotkeys.rs) - unchanged
2. Audio recording starts (audio.rs) - unchanged
3. User releases Ctrl
4. Audio passed directly to local Whisper inference (NEW)
5. Transcript returned immediately and pasted to clipboard
```

---

## Implementation Requirements

### Core Functionality

1. **Local Whisper inference** using candle-coreml or coreml-rs
2. **Model management** - download/cache models in `~/.codescribe/models/`
3. **Streaming support** (optional) - real-time partial transcripts
4. **Fallback to cloud** - if local inference fails or model not downloaded

### Technical Constraints

- **macOS only** - using CoreML/ANE (no Linux/Windows support needed)
- **Apple Silicon required** - M1/M2/M3 chips (Intel Macs can use CPU fallback)
- **Rust edition 2024** - project uses latest Rust edition
- **Async runtime** - tokio is already used in the project

### Model Requirements

| Model | Size | Quality | Recommended Use |
|-------|------|---------|-----------------|
| tiny | 39MB | ⭐⭐ | Quick testing |
| base | 74MB | ⭐⭐⭐ | Default for most users |
| small | 244MB | ⭐⭐⭐⭐ | Better accuracy |
| large-v3 | 1.5GB | ⭐⭐⭐⭐⭐ | Best quality |

**Default:** Bundle `base` model, allow downloading others on demand.

---

## Actionable TODO List

### Phase 1: Research & Setup
- [ ] Read and understand `src/client.rs` current implementation
- [ ] Read and understand `src/audio.rs` audio format (sample rate, channels, format)
- [ ] Study `/Users/maciejgad/LIBRAXIS/lbrx/crates/metal/` for Metal patterns
- [ ] Test candle-coreml builds on the target machine
- [ ] Verify CoreML model conversion works with whisper.coreml

### Phase 2: Core Implementation
- [ ] Create new module `src/local_stt.rs` or `src/stt/mod.rs`
- [ ] Implement `LocalWhisperEngine` struct with:
  - [ ] `new(model_path: &Path) -> Result<Self>`
  - [ ] `transcribe(audio: &[f32], sample_rate: u32) -> Result<String>`
  - [ ] `transcribe_file(path: &Path) -> Result<String>`
- [ ] Add model download/cache logic in `src/models.rs`
- [ ] Integrate with existing audio pipeline (convert cpal output to Whisper input)

### Phase 3: Integration
- [ ] Modify `src/controller.rs` to use local STT when available
- [ ] Add config option in `src/config/types.rs`: `use_local_stt: bool`
- [ ] Add config option: `local_model: String` (tiny/base/small/large)
- [ ] Implement graceful fallback to cloud if local fails
- [ ] Update `src/client.rs` to be optional (only used for fallback)

### Phase 4: Model Management
- [ ] Create `~/.codescribe/models/` directory structure
- [ ] Implement model download from HuggingFace
- [ ] Add progress indicator for model download
- [ ] Implement model verification (checksum)
- [ ] Add CLI command: `codescribe --download-model base`

### Phase 5: Testing & Polish
- [ ] Test with various audio lengths (5s, 30s, 2min, 5min)
- [ ] Test with different languages (Polish veterinary terms!)
- [ ] Benchmark local vs cloud latency
- [ ] Add tracing/logging for performance monitoring
- [ ] Update `Cargo.toml` with new dependencies
- [ ] Update `README.md` with local STT documentation

### Phase 6: Optional Enhancements
- [ ] Streaming transcription (partial results while speaking)
- [ ] VAD (Voice Activity Detection) to auto-stop recording
- [ ] Speaker diarization (who is speaking)
- [ ] Custom vocabulary/lexicon support (veterinary terms)

---

## Code Snippets to Start

### Cargo.toml additions

```toml
# Add to [dependencies]
candle-core = "0.8"
candle-coreml = "0.3"
# OR alternatively:
# coreml-rs = "0.5"

# For model downloading
reqwest = { version = "0.12", features = ["stream"] }
indicatif = "0.17"  # Progress bars
```

### Basic LocalWhisperEngine skeleton

```rust
// src/local_stt.rs

use anyhow::Result;
use std::path::Path;

pub struct LocalWhisperEngine {
    // CoreML model handle
    model: candle_coreml::Model,  // or coreml_rs equivalent
}

impl LocalWhisperEngine {
    pub fn new(model_path: &Path) -> Result<Self> {
        // Load .mlmodelc from disk
        todo!("Load CoreML model")
    }

    pub fn transcribe(&self, audio: &[f32], sample_rate: u32) -> Result<String> {
        // 1. Resample to 16kHz if needed
        // 2. Convert to mel spectrogram
        // 3. Run encoder
        // 4. Run decoder with beam search
        // 5. Return text
        todo!("Implement inference")
    }
}
```

### Integration point in controller.rs

```rust
// In src/controller.rs, modify the transcription logic:

async fn transcribe_audio(&self, audio_path: &Path) -> Result<String> {
    if self.config.use_local_stt {
        match self.local_engine.transcribe_file(audio_path) {
            Ok(text) => return Ok(text),
            Err(e) => {
                tracing::warn!("Local STT failed, falling back to cloud: {}", e);
                // Fall through to cloud
            }
        }
    }

    // Existing cloud transcription
    self.client.transcribe(audio_path).await
}
```

---

## Important Notes

### DO NOT:
- Remove Python backend support yet (keep as fallback during testing)
- Change hotkey system (CGEventTap works perfectly)
- Modify audio capture (cpal setup is correct)
- Break existing cloud STT functionality

### DO:
- Keep changes additive (new modules, not rewrites)
- Use feature flags for new functionality
- Add comprehensive error handling
- Log performance metrics (transcription time, model load time)
- Test with Polish language (veterinary terminology)

### Git Workflow:
- Create feature branch: `feature/pure-rust-stt`
- Make atomic commits with clear messages
- Do NOT force push or amend existing commits

---

## Resources

### Documentation
- candle-coreml: https://docs.rs/candle-coreml
- coreml-rs: https://docs.rs/coreml-rs
- whisper.coreml: https://github.com/wangchou/whisper.coreml
- MLX Whisper models: https://huggingface.co/mlx-community

### Existing Code to Study
- CodeScribe: `/Users/maciejgad/hosted/VetCoders/CodeScribe/`
- LBRX Metal: `/Users/maciejgad/LIBRAXIS/lbrx/crates/metal/`

### Contact
If blocked, ask questions. Better to clarify than to guess wrong.

---

*Created by M&K (c)2026 VetCoders*
