# Junie Task: Implement DecodingParams Usage

## Context

W `src/local_stt.rs` mamy `DecodingParams` struct z parametrami zgodnymi z mlx_whisper/faster-whisper, ale tylko `no_repeat_ngram_size` jest używane. Reszta to dead code.

## Current State

```rust
pub struct DecodingParams {
    pub temperature: f32,                  // NOT USED
    pub no_repeat_ngram_size: usize,       // USED (line 452)
    pub suppress_blank: bool,              // NOT USED
    pub no_speech_threshold: f32,          // NOT USED
    pub compression_ratio_threshold: f32,  // NOT USED
    pub logprob_threshold: f32,            // NOT USED
}
```

## Task: Implement Usage of All Fields

### 1. Temperature Sampling (temperature > 0)

W `transcribe_samples_16k()` zamiast greedy argmax, użyj softmax + sampling:

```rust
// Current (greedy):
let mut best_token = eot_token;
let mut best_val = f32::NEG_INFINITY;
for (idx, &val) in logits_vec.iter().enumerate() {
    if val > best_val {
        best_val = val;
        best_token = idx as u32;
    }
}

// New (temperature sampling if temperature > 0):
let best_token = if self.decoding_params.temperature > 0.0 {
    // Apply temperature scaling
    let temp = self.decoding_params.temperature;
    let scaled: Vec<f32> = logits_vec.iter().map(|&x| x / temp).collect();

    // Softmax
    let max_val = scaled.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exp_sum: f32 = scaled.iter().map(|&x| (x - max_val).exp()).sum();
    let probs: Vec<f32> = scaled.iter().map(|&x| (x - max_val).exp() / exp_sum).collect();

    // Sample from distribution
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let r: f32 = rng.gen();
    let mut cumsum = 0.0;
    let mut selected = 0u32;
    for (idx, &p) in probs.iter().enumerate() {
        cumsum += p;
        if r < cumsum {
            selected = idx as u32;
            break;
        }
    }
    selected
} else {
    // Greedy (current implementation)
    // ...
};
```

### 2. Suppress Blank (suppress_blank)

Na początku dekodowania (first few tokens), blokuj blank/silence tokeny:

```rust
// After getting logits, if suppress_blank and early in decoding:
if self.decoding_params.suppress_blank && all_tokens.len() < 4 {
    // Block common blank tokens (space, empty, etc.)
    // Token IDs depend on tokenizer - check whisper tokenizer
    let blank_tokens = [220, 50256]; // Example - verify with tokenizer
    for &tok in &blank_tokens {
        if tok < logits_vec.len() {
            logits_vec[tok] = f32::NEG_INFINITY;
        }
    }
}
```

### 3. No-Speech Threshold (no_speech_threshold)

Po pierwszym decoder step, sprawdź probability tokenu `<|nospeech|>`:

```rust
// After first decoder step, check no_speech probability
if step == 0 {
    if let Some(nos) = nospeech_token {
        let nos_idx = nos as usize;
        if nos_idx < logits_vec.len() {
            // Compute softmax probability
            let max_val = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let exp_sum: f32 = logits_vec.iter().map(|&x| (x - max_val).exp()).sum();
            let nos_prob = (logits_vec[nos_idx] - max_val).exp() / exp_sum;

            if nos_prob > self.decoding_params.no_speech_threshold {
                tracing::debug!("No speech detected (prob={:.3})", nos_prob);
                return Ok(String::new()); // Return empty for silence
            }
        }
    }
}
```

### 4. Compression Ratio Threshold (compression_ratio_threshold)

Po transkrypcji, sprawdź gzip compression ratio tekstu:

```rust
fn compression_ratio(text: &str) -> f32 {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let original_len = text.len();
    if original_len == 0 {
        return 0.0;
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).ok();
    let compressed = encoder.finish().unwrap_or_default();

    original_len as f32 / compressed.len() as f32
}

// After transcription:
let ratio = compression_ratio(&text);
if ratio > self.decoding_params.compression_ratio_threshold {
    tracing::warn!("High compression ratio ({:.2}) - possible hallucination", ratio);
    // Optionally: retry with higher temperature or return error
}
```

### 5. Logprob Threshold (logprob_threshold)

Track average log probability podczas dekodowania:

```rust
// In decode loop, track logprobs:
let mut sum_logprob = 0.0f32;
let mut token_count = 0usize;

// After selecting best_token:
let max_val = logits_vec.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
let exp_sum: f32 = logits_vec.iter().map(|&x| (x - max_val).exp()).sum();
let token_prob = (logits_vec[best_token as usize] - max_val).exp() / exp_sum;
sum_logprob += token_prob.ln();
token_count += 1;

// After loop:
let avg_logprob = sum_logprob / token_count as f32;
if avg_logprob < self.decoding_params.logprob_threshold {
    tracing::warn!("Low avg logprob ({:.2}) - possible hallucination", avg_logprob);
}
```

## Files to Modify

- `src/local_stt.rs` - implement all above in `transcribe_samples_16k()`

## Dependencies

May need to add:
- `rand` crate for temperature sampling (check if already in Cargo.toml)
- `flate2` crate for gzip compression ratio

## Testing

```bash
cargo clippy --release -- -D warnings
cargo test --release
cargo run --release --example test_audio_long -- --model models/whisper-large-v3-turbo-mlx-q8 tests/recordings/1.fretka-Ziggy.mp3
```

## Notes

- Defaults are already set correctly in `DecodingParams::default()`
- temperature=0.0 means greedy (current behavior)
- These features match mlx_whisper CLI parameters

---
*Task for Junie - VetCoders CodeScribe*
