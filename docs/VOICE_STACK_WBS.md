# Voice Stack Implementation - Work Breakdown Structure

> **Project:** CodeScribe Voice Stack (Embedder + TTS)
> **Date:** 2026-01-21
> **Authors:** Klaudiusz, Junie, Codex
> **Status:** Planning

---

## Overview

Integration of E5-large embedder and Sesame CSM-1B TTS into CodeScribe, following the established Whisper embedded model pattern.

### Target Stack

| Component | Model                  | Size (Q8)   | Candle Module | Owner        |
| --------- | ---------------------- | ----------- | ------------- | ------------ |
| STT       | Whisper large-v3-turbo | 894 MB      | `whisper`     | ✅ Existing  |
| Embedder  | E5-large multilingual  | ~670 MB     | `bert`        | 🟢 Junie     |
| TTS       | Sesame CSM-1B          | ~1.0 GB     | `csm`         | 🟣 Klaudiusz |
| **Total** |                        | **~2.6 GB** |               |              |

### Requirements

- Same process (no subprocess/FFI)
- Same Q8 quantization (MLX-style)
- Zero overhead (pure Candle, no ort/whisper-rs)
- `include_bytes!` embedding
- 1.5 GB budget per model

---

## File Ownership Map

```
core/                  ← NEW STRUCTURE (feat/architecture-reorganization)
├── stt/
│   └── whisper/       ← 🔒 LOCKED (existing, don't touch)
├── embedder/          ← 🟢 JUNIE
│   ├── mod.rs
│   ├── embedded.rs
│   ├── engine.rs
│   └── singleton.rs
├── tts/               ← 🟣 KLAUDIUSZ
│   ├── mod.rs
│   ├── embedded.rs
│   ├── engine.rs
│   ├── singleton.rs
│   ├── audio_output.rs
│   └── voices.rs
└── lib.rs             ← 🟣 KLAUDIUSZ (exports only)

tests/
├── e2e_embedder*.rs   ← 🟦 CODEX
├── e2e_tts*.rs        ← 🟦 CODEX
└── e2e_pipeline*.rs   ← 🟦 CODEX

scripts/
├── download-e5.sh     ← 🟦 CODEX
├── download-csm.sh    ← 🟦 CODEX
└── download-all-models.sh ← 🟦 CODEX

build.rs               ← 🟣 KLAUDIUSZ
docs/                  ← 🟦 CODEX
```

---

## 🟢 JUNIE: Embedder Module

**Branch:** `feat/embedder-e5`

**Rationale:** Junie authored the Whisper integration → knows `embedded.rs`, `engine.rs`, `singleton.rs` pattern intimately. E5-large is BERT-based, simpler than Whisper.

### Tasks

| Task             | File                    | Notes                               |
| ---------------- | ----------------------- | ----------------------------------- |
| Module structure | `embedder/mod.rs`       | Follow Whisper pattern              |
| Embedded loader  | `embedder/embedded.rs`  | Same as Whisper                     |
| BERT engine      | `embedder/engine.rs`    | `candle_transformers::models::bert` |
| Singleton        | `embedder/singleton.rs` | Same as Whisper                     |
| Q8 quantization  | `embedder/engine.rs`    | Same dequant as Whisper             |

### Public API

```rust
// embedder/mod.rs
pub fn init() -> Result<()>;
pub fn embed(text: &str) -> Result<Vec<f32>>;
pub fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>>;
pub fn similarity(a: &[f32], b: &[f32]) -> f32;
```

### Model Details

- **URL:** `https://huggingface.co/intfloat/multilingual-e5-large`
- **Params:** 335M
- **Embedding dims:** 1024
- **Languages:** 100 (including Polish)
- **Format:** safetensors (Candle-native)

### Download Command

```bash
hf download intfloat/multilingual-e5-large \
  --include "model.safetensors" "tokenizer.json" "config.json" \
  --local-dir models/e5-large
```

### Estimated Effort

2 days (knows the pattern)

---

## 🟣 KLAUDIUSZ: TTS Module + Integration

**Branch:** `feat/tts-csm`

**Rationale:** CSM is more complex - multi-speaker, audio generation, streaming. Requires more research and experimentation.

### Tasks

| Task             | File                  | Notes                              |
| ---------------- | --------------------- | ---------------------------------- |
| Module structure | `tts/mod.rs`          | New, based on Junie's pattern      |
| Embedded loader  | `tts/embedded.rs`     | Larger model (~1GB)                |
| CSM engine       | `tts/engine.rs`       | `candle_transformers::models::csm` |
| Audio generation | `tts/audio_output.rs` | cpal playback, wav export          |
| Voice management | `tts/voices.rs`       | Voice selection, speaker IDs       |
| Build.rs update  | `build.rs`            | Orchestrate all 3 models           |
| Lib exports      | `lib.rs`              | Wire everything together           |

### Public API

```rust
// tts/mod.rs
pub fn init() -> Result<()>;
pub fn synthesize(text: &str, voice: &str) -> Result<Vec<f32>>;
pub fn synthesize_to_file(text: &str, voice: &str, path: &Path) -> Result<()>;
pub fn play(text: &str, voice: &str) -> Result<()>;
pub fn list_voices() -> Vec<&'static str>;
```

### Model Details

- **URL:** `https://huggingface.co/sesame/csm-1b`
- **Params:** 1B
- **Sample rate:** 24kHz
- **License:** Apache 2.0
- **Format:** safetensors

### Download Command

```bash
hf download sesame/csm-1b \
  --include "*.safetensors" "*.json" \
  --local-dir models/csm-1b
```

### Estimated Effort

4-5 days

---

## 🟦 CODEX: Tests + Scripts + Docs

**Branch:** `feat/voice-stack-support`

**Rationale:** Standardized work that doesn't require deep knowledge of the embedding pattern.

### Tasks

| Task           | File                             | Notes                    |
| -------------- | -------------------------------- | ------------------------ |
| Download E5    | `scripts/download-e5.sh`         | HuggingFace CLI          |
| Download CSM   | `scripts/download-csm.sh`        | HuggingFace CLI          |
| Download all   | `scripts/download-all-models.sh` | Combined script          |
| Embedder tests | `tests/e2e_embedder*.rs`         | After Junie's module     |
| TTS tests      | `tests/e2e_tts*.rs`              | After Klaudiusz's module |
| Pipeline tests | `tests/e2e_voice_pipeline*.rs`   | Full integration         |
| CLAUDE.md      | `CLAUDE.md`                      | Update for new modules   |
| Architecture   | `docs/ARCHITECTURE.md`           | Update diagrams          |

### Test Examples

```rust
#[test]
fn test_embedder_polish_text() {
    embedder::init().unwrap();
    let embedding = embedder::embed("Cześć, jak się masz?").unwrap();
    assert_eq!(embedding.len(), 1024); // E5-large dims
}

#[test]
fn test_tts_synthesis() {
    tts::init().unwrap();
    let audio = tts::synthesize("Hello world", "default").unwrap();
    assert!(audio.len() > 24000); // At least 1 second @ 24kHz
}

#[test]
fn test_full_voice_pipeline() {
    // STT → Embed → (mock LLM) → TTS
    whisper::init().unwrap();
    embedder::init().unwrap();
    tts::init().unwrap();

    let transcript = whisper::transcribe(&audio_samples, 16000, Some("pl")).unwrap();
    let embedding = embedder::embed(&transcript).unwrap();
    let response = "Rozumiem, dziękuję.";
    let speech = tts::synthesize(response, "default").unwrap();

    assert!(speech.len() > 0);
}
```

### Estimated Effort

2-3 days (parallel + after modules)

---

## Timeline

### Week 1

| Day | Junie (Embedder)                       | Klaudiusz (TTS)                     | Codex (Support)                     |
| --- | -------------------------------------- | ----------------------------------- | ----------------------------------- |
| 1-2 | `embedder/` full implementation        | `tts/` scaffolding + CSM research   | Download scripts + test scaffolding |
| 3-4 | `embedder/` done, PR ready             | `tts/engine.rs` + `audio_output.rs` | `e2e_embedder*.rs` tests            |
| 5   | Review Klaudiusz's TTS (pattern check) | `tts/` done, PR ready               | `e2e_tts*.rs` tests                 |

### Week 2

| Day | Junie                                      | Klaudiusz                         | Codex                     |
| --- | ------------------------------------------ | --------------------------------- | ------------------------- |
| 1-2 | Support / bug fixes                        | `build.rs` + `lib.rs` integration | `e2e_pipeline*.rs` + docs |
| 3   | ALL: Merge, integration testing, bug fixes |

---

## Merge Order

1. `feat/embedder-e5` → develop (first, no deps)
2. `feat/tts-csm` → develop (second, after embedder merged)
3. `feat/voice-stack-support` → develop (last, needs both)

---

## Coordination Rules

### No Touch Zones (LOCKED)

- `whisper/*` - Existing, working
- `controller.rs` - State machine
- `hotkeys.rs` - Input handling
- `audio/recorder.rs` - Recording logic

### Shared Files (Coordinate)

| File         | Owner      | Others                 |
| ------------ | ---------- | ---------------------- |
| `lib.rs`     | Klaudiusz  | Notify before touching |
| `build.rs`   | Klaudiusz  | Notify before touching |
| `Cargo.toml` | Coordinate | Discuss dep additions  |

### Junie as Pattern Authority

Junie authored the Whisper integration, so:

- ✅ Final say on `embedded.rs` / `singleton.rs` structure
- ✅ Code review Klaudiusz's TTS for pattern compliance
- ✅ If Klaudiusz deviates from Whisper pattern → Junie must approve

---

## Future: Polish TTS Finetuning

After base implementation:

1. **Data collection:** Common Voice PL, MLS Polish, custom recordings
2. **Finetuning:** LoRA/QLoRA on CSM-1B
3. **Export:** Finetuned weights to safetensors
4. **Deploy:** Replace CSM-1B with CSM-1B-PL (same size, same API)

---

## Model URLs (Exact)

```bash
# Whisper (existing)
# Already embedded in binary

# E5-large Embedder
https://huggingface.co/intfloat/multilingual-e5-large
# Files: model.safetensors (~1.3GB), tokenizer.json, config.json

# Sesame CSM-1B TTS
https://huggingface.co/sesame/csm-1b
# Files: *.safetensors (~2GB), config.json, tokenizer files

# Quantized versions (for reference)
# Will be created during build like Whisper
```

---

_Created by M&K (c)2026 VetCoders_
