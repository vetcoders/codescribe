# Junie - STT Fixes & Implementation Summary

> **From: Klaudiusz (Claude Opus 4.5)**
> **Date: 2026-01-10**
> **Status: ✅ All fixes applied and working!**

---

## Implementation Summary

The Pure Rust STT implementation is now fully functional with the following capabilities:

- ✅ Q8 dequantization for MLX-quantized Whisper models
- ✅ Metal GPU acceleration on Apple Silicon
- ✅ Full pipeline: audio → mel spectrogram → encoder → decoder → text
- ✅ KV cache in attention layers
- ✅ Language parameter support
- ✅ Works with both short and medium-length audio files

---

## Key Components

### 1. LocalWhisperEngine (`src/local_stt.rs`)

The main STT engine that handles:
- Model loading from MLX-format directories
- Q8 weight dequantization (uint8 packed in u32 → f32)
- Mel spectrogram computation
- Greedy decoding with EOT detection

**Key methods:**
```rust
// Create engine with model path
pub fn new(model_path: &Path) -> Result<Self>

// Transcribe with optional language hint
pub fn transcribe_with_language(
    &mut self,
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<String>

// Transcribe file directly
pub fn transcribe_file_with_language(
    &mut self,
    path: &Path,
    language: Option<&str>,
) -> Result<String>
```

### 2. Whisper Model (`src/whisper_model.rs`)

Custom Whisper model implementation compatible with candle-core:
- AudioEncoder with Conv1d layers and positional embeddings
- TextDecoder with cross-attention to encoder output
- KV cache for efficient autoregressive decoding

### 3. Audio Loader (`src/audio_loader.rs`)

Handles audio file loading and preprocessing:
- Supports multiple formats via Symphonia (MP3, M4A, WAV, etc.)
- Automatic channel mixing to mono
- Resampling to 16kHz for Whisper

### 4. Model Manager (`src/models.rs`)

Manages model paths and discovery:
- Checks `CODESCRIBE_MODELS_DIR` environment variable
- Falls back to `models/` in repo root
- Falls back to `~/.CodeScribe/models/`

---

## Fixed Issues

### Issue 1: Language Detection ✅ FIXED

**Problem:** Polish audio was being transcribed as English.

**Solution:** Added language parameter support in `transcribe_with_language()`:

```rust
let mut tokens = vec![start_token];
if let Some(lang) = language {
    let lang_tok = format!("<|{}|>", lang.to_lowercase());
    if let Some(t) = self.tokenizer.token_to_id(&lang_tok) {
        tokens.push(t);
    }
}
if let Some(t) = self.tokenizer.token_to_id("<|transcribe|>") {
    tokens.push(t);
}
```

**Usage:**
```rust
// Auto-detect language
engine.transcribe_file_with_language(&path, None)?;

// Force Polish
engine.transcribe_file_with_language(&path, Some("pl"))?;

// Force English
engine.transcribe_file_with_language(&path, Some("en"))?;
```

### Issue 2: Empty Results for Medium Audio ✅ FIXED

**Problem:** Medium-length audio files produced empty transcriptions.

**Root cause:** Early EOT token suppression was needed to prevent the model from terminating too early.

**Solution:** Added token suppression for the first 16 tokens:

```rust
let suppress_tokens = all_tokens.len() < 16;
if suppress_tokens {
    if (eot_token as usize) < logits_vec.len() {
        logits_vec[eot_token as usize] = f32::NEG_INFINITY;
    }
    if let Some(nos) = nospeech_token {
        if (nos as usize) < logits_vec.len() {
            logits_vec[nos as usize] = f32::NEG_INFINITY;
        }
    }
}
```

### Issue 3: Q8 Dequantization ✅ FIXED

**Problem:** MLX uses a specific quantization format that differs from standard int8.

**Solution:** Implemented correct dequantization formula:
- MLX packs 4 uint8 values into each u32
- Formula: `dequantized = uint8_value * scale + bias`
- Group size: 32 elements per scale/bias pair

```rust
fn dequantize_q8(packed: &Tensor, scales: &Tensor, biases: &Tensor, device: &Device) -> Result<Tensor> {
    // Unpack u32 → 4x u8
    for b in 0..4 {
        let w = ((val >> (8 * b)) & 0xff) as u8;
        let scale = scales_data[o][group];
        let bias = biases_data[o][group];
        output.push((w as f32) * scale + bias);
    }
}
```

---

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CODESCRIBE_MODELS_DIR` | Custom models directory | `models/` or `~/.CodeScribe/models/` |
| `CODESCRIBE_TEST_MODEL_DIR` | Model path for tests | `models/whisper-large-v3-mlx-q8` |
| `CODESCRIBE_DEBUG_TOKENS` | Enable token debugging | `false` |

### Config Options (`src/config/types.rs`)

```rust
pub use_local_stt: bool,      // Enable local STT (default: false)
pub local_model: String,       // Model name (default: "base")
```

---

## Testing

### Unit Test
```bash
cargo test --test local_stt_test
```

### E2E Test
```bash
# Short audio only
CODESCRIBE_E2E_RUN_MEDIUM=0 cargo run --release --example e2e_stt

# Short + medium audio
CODESCRIBE_E2E_RUN_MEDIUM=1 cargo run --release --example e2e_stt

# With specific language
CODESCRIBE_E2E_LANG=en cargo run --release --example e2e_stt
```

### Expected Output
```
Engine initialized successfully.
Transcribing short audio: audio-real-short.m4a
Short transcription completed in ~9s:
---
The actual transcribed text appears here...
---
```

---

## Performance

| Audio Length | Transcription Time | Model |
|--------------|-------------------|-------|
| ~5s (short) | ~9s | whisper-large-v3-mlx-q8 |
| ~30s (medium) | ~20s | whisper-large-v3-mlx-q8 |

*Tested on Apple Silicon with Metal acceleration*

---

## Supported Models

The implementation supports MLX-format Whisper models:

| Model | Directory | Size |
|-------|-----------|------|
| whisper-large-v3-mlx-q8 | `models/whisper-large-v3-mlx-q8/` | ~1.5GB |
| whisper-large-v3 | `models/whisper-large-v3/` | ~3GB |
| whisper-small | `models/whisper-small/` | ~500MB |

**Required files in model directory:**
- `config.json` - Model configuration
- `weights.safetensors` - Model weights
- `tokenizer.json` - Tokenizer vocabulary
- `mel_filters.npz` - Mel filterbank coefficients

---

## Integration with Controller

The `RecordingController` (`src/controller.rs`) automatically uses local STT when:
1. `use_local_stt` is enabled in config
2. The specified model exists

Falls back to cloud STT if local inference fails.

```rust
if use_local_stt {
    let local_result = tokio::task::spawn_blocking(move || {
        engine.transcribe_file_with_language(&path, language)
    }).await;

    match local_result {
        Ok(Ok(text)) => return Ok(text),
        _ => { /* Fall back to cloud */ }
    }
}
```

---

*Created by M&K (c)2026 VetCoders*
