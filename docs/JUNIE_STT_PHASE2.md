# Junie - STT Phase 2: Production Readiness

> **From: Klaudiusz (Claude Opus 4.5)**
> **Date: 2026-01-10**
> **Branch: feat/pure-rust-flow**
> **Status: Phase 1 complete, Phase 2 starting**

---

## Context

Phase 1 is done. Local Whisper STT works:
- Q8 dequantization from MLX weights
- Metal GPU acceleration
- Polish transcription with `<|pl|>` token
- ~20s for 30s audio on Apple Silicon

**Key question:** Can this replace cloud API as primary STT backend?

---

## Phase 2 Goals

### 1. Long Audio Support (Chunking)

**Problem:** Whisper has 30-second context window. Audio longer than ~30s may produce incomplete or degraded results.

**Task:** Implement chunking strategy.

```rust
// Pseudocode - implement in src/local_stt.rs
pub fn transcribe_long(&mut self, audio: &[f32], sample_rate: u32, language: Option<&str>) -> Result<String> {
    let samples = resample_to_16k(audio, sample_rate);
    let chunk_samples = 16000 * 25; // 25s chunks with overlap
    let overlap = 16000 * 5;        // 5s overlap

    let mut full_text = String::new();
    let mut offset = 0;

    while offset < samples.len() {
        let end = (offset + chunk_samples).min(samples.len());
        let chunk = &samples[offset..end];

        let text = self.transcribe_chunk(chunk, language)?;
        full_text.push_str(&text);
        full_text.push(' ');

        offset += chunk_samples - overlap;
    }

    Ok(deduplicate_overlap(&full_text))
}
```

**Considerations:**
- Overlap prevents cutting words mid-sentence
- Need deduplication logic for overlapping segments
- VAD (Voice Activity Detection) would be better but more complex

---

### 2. Language Auto-Detection

**Problem:** Currently requires explicit `language` parameter or defaults to English.

**Task:** Implement Whisper's built-in language detection.

**How Whisper does it:**
1. Run encoder on first 30s
2. Decode with only `<|startoftranscript|>` token
3. Look at logits for language tokens (`<|en|>`, `<|pl|>`, etc.)
4. Pick highest probability language

```rust
// Pseudocode - add to LocalWhisperEngine
pub fn detect_language(&mut self, audio: &[f32], sample_rate: u32) -> Result<String> {
    let samples = resample_to_16k(audio, sample_rate);
    let samples = &samples[..samples.len().min(480000)]; // max 30s

    self.model.reset_kv_cache();
    let mel = compute_mel(&samples);
    let encoder_output = self.model.encoder.forward(&mel, true)?;

    let start_token = self.tokenizer.token_to_id("<|startoftranscript|>").unwrap();
    let tokens = Tensor::new(&[start_token], &self.device)?.unsqueeze(0)?;

    let hidden = self.model.decoder.forward(&tokens, &encoder_output, true)?;
    let logits = self.model.decoder.final_linear(&hidden)?;

    // Language tokens are in range [50259, 50358] for Whisper
    // Find max logit among language tokens
    let lang_tokens = ["en", "pl", "de", "fr", "es", "it", "pt", "nl", "ru", "uk", "cs", "sk"];
    let mut best_lang = "en";
    let mut best_score = f32::NEG_INFINITY;

    for lang in lang_tokens {
        if let Some(token_id) = self.tokenizer.token_to_id(&format!("<|{}|>", lang)) {
            let score = logits.get(token_id)?;
            if score > best_score {
                best_score = score;
                best_lang = lang;
            }
        }
    }

    Ok(best_lang.to_string())
}
```

**Then update transcribe:**
```rust
pub fn transcribe_with_language(..., language: Option<&str>) -> Result<String> {
    let lang = match language {
        Some(l) => l.to_string(),
        None => self.detect_language(audio, sample_rate)?,
    };
    // ... rest of transcription with detected language
}
```

---

### 3. Stability & Memory Tests

**Problem:** Need to verify model stays loaded and responsive across multiple transcriptions.

**Tasks:**

#### 3a. Unit test for model persistence
```rust
// tests/local_stt_test.rs
#[test]
fn test_model_stays_loaded() {
    let mut engine = LocalWhisperEngine::new(&model_path).unwrap();

    // Transcribe 10 times in sequence
    for i in 0..10 {
        let result = engine.transcribe_file_with_language(&audio_path, Some("pl"));
        assert!(result.is_ok(), "Transcription {} failed: {:?}", i, result);
    }
}
```

#### 3b. Memory usage test
```rust
#[test]
fn test_memory_stable() {
    let mut engine = LocalWhisperEngine::new(&model_path).unwrap();

    // Get baseline memory
    let baseline = get_process_memory();

    for _ in 0..20 {
        let _ = engine.transcribe_file_with_language(&audio_path, Some("pl"));
    }

    let after = get_process_memory();
    let growth = after - baseline;

    // Should not grow more than 100MB (KV cache etc.)
    assert!(growth < 100_000_000, "Memory grew by {} bytes", growth);
}
```

#### 3c. Concurrent access test
```rust
#[tokio::test]
async fn test_concurrent_transcriptions() {
    let engine = Arc::new(Mutex::new(LocalWhisperEngine::new(&model_path).unwrap()));

    let handles: Vec<_> = (0..5).map(|_| {
        let engine = engine.clone();
        tokio::spawn(async move {
            let mut guard = engine.lock().await;
            guard.transcribe_file_with_language(&audio_path, Some("pl"))
        })
    }).collect();

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
```

---

### 4. Integration Validation

**Task:** Verify full app flow works with local STT as primary.

```bash
# Set config to use local STT
echo 'USE_LOCAL_STT=true' >> ~/.codescribe/.env
echo 'LOCAL_MODEL=whisper-large-v3-mlx-q8' >> ~/.codescribe/.env

# Run app and test recording flow
cargo run --release

# Expected: tray icon → record → transcribe locally → paste text
```

**Acceptance criteria:**
- [ ] App starts without crash
- [ ] Model loads on startup (check logs)
- [ ] First transcription works
- [ ] 10th transcription works (model still in RAM)
- [ ] Memory doesn't grow unboundedly
- [ ] Fallback to cloud works if local fails

---

## File Locations

| Task | File |
|------|------|
| Chunking | `src/local_stt.rs` - new `transcribe_long()` |
| Auto-detect | `src/local_stt.rs` - new `detect_language()` |
| Tests | `tests/local_stt_test.rs` (create if missing) |
| Integration | Manual testing with `cargo run --release` |

---

## Priority

1. **Stability tests** - must know if this is production-viable
2. **Auto-detect** - critical for UX (user shouldn't set language manually)
3. **Chunking** - nice-to-have (most voice recordings are <30s)

---

## Success Criteria

Local STT is production-ready when:
- 100 sequential transcriptions complete without error
- Memory stays under 4GB total for app
- Auto-detect correctly identifies Polish 95%+ of time
- No manual language configuration needed

---

*Created by M&K (c)2026 VetCoders*
