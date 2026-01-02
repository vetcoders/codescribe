# Audio Module - Quick Start Guide

## 30-Second Start

```rust
use codescribe::audio::Recorder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut recorder = Recorder::new()?;
    recorder.start().await?;
    // Auto-stops after 0.8s silence
    if let Some(path) = recorder.stop().await? {
        println!("Saved: {:?} ({:.2}s)", path, recorder.last_duration());
    }
    Ok(())
}
```

## Common Patterns

### 1. Manual Stop After Fixed Duration

```rust
let mut recorder = Recorder::new()?;
recorder.start().await?;

tokio::time::sleep(Duration::from_secs(5)).await;

if let Some(path) = recorder.stop().await? {
    // Use path for transcription
}
```

### 2. Custom Silence Settings

```rust
use codescribe::audio::RecorderConfig;

let config = RecorderConfig {
    silence_db: -50.0,     // More sensitive
    hang_sec: 1.5,          // Wait longer
    ..Default::default()
};

let mut recorder = Recorder::with_config(config)?;
```

### 3. Disable Auto-Silence

```rust
let config = RecorderConfig {
    auto_silence: false,
    ..Default::default()
};

let mut recorder = Recorder::with_config(config)?;
// Must call stop() manually
```

### 4. Live Streaming with Snapshots

```rust
recorder.start().await?;

loop {
    tokio::time::sleep(Duration::from_secs(1)).await;

    if let Some(chunk) = recorder.snapshot_wav(0.8)? {
        send_to_transcription_service(chunk).await?;
    }

    if !recorder.is_recording() {
        break;
    }
}

recorder.stop().await?;
```

### 5. Check Diagnostics

```rust
recorder.stop().await?;

let diag = recorder.diagnostics();
println!("Captured {} frames ({:.2}s), {} chunks, {} bytes",
    diag.frames,
    diag.duration_sec,
    diag.chunks,
    diag.bytes
);
```

### 6. Environment Variable Configuration

```bash
# Shell
export SILENCE_DB=-50.0
export SILENCE_HANG_SEC=1.2
export AUTO_SILENCE=0

cargo run --example audio_example
```

```rust
// Rust - automatically picked up
let recorder = Recorder::new()?; // Uses env vars
```

## Error Handling

```rust
match recorder.start().await {
    Ok(_) => println!("Recording..."),
    Err(e) => {
        if e.to_string().contains("already in progress") {
            println!("Stop current recording first");
        } else if e.to_string().contains("No input device") {
            println!("No microphone found");
        } else {
            eprintln!("Error: {}", e);
        }
    }
}
```

## Testing

```bash
# Run unit tests
cargo test --lib audio::tests

# Run example
cargo run --example audio_example

# With custom config
SILENCE_DB=-50 SILENCE_HANG_SEC=1.5 cargo run --example audio_example
```

## Output Format

All WAV files:
- 16 kHz sample rate
- Mono (1 channel)
- 16-bit signed integer
- PCM encoding

Perfect for Whisper and similar STT services.

## Key Methods

| Method | Description |
|--------|-------------|
| `new()` | Create with default config |
| `with_config()` | Create with custom config |
| `start()` | Begin recording |
| `stop()` | Stop and save to temp WAV |
| `snapshot_wav(min_sec)` | Save snapshot without stopping |
| `is_recording()` | Check if actively recording |
| `last_duration()` | Get last recording duration |
| `diagnostics()` | Get detailed metrics |

## Defaults

| Setting | Default | Env Var |
|---------|---------|---------|
| Sample Rate | 16000 Hz | - |
| Channels | 1 (mono) | - |
| Silence Threshold | -45.0 dB | `SILENCE_DB` |
| Hang Time | 0.8 sec | `SILENCE_HANG_SEC` |
| Auto-Silence | Enabled | `AUTO_SILENCE` |
| Block Size | 1024 samples | - |

## Integration Example

```rust
// In your app
use codescribe::audio::Recorder;

pub struct App {
    recorder: Option<Recorder>,
}

impl App {
    pub async fn start_recording(&mut self) -> anyhow::Result<()> {
        let mut recorder = Recorder::new()?;
        recorder.start().await?;
        self.recorder = Some(recorder);
        Ok(())
    }

    pub async fn stop_recording(&mut self) -> anyhow::Result<Option<PathBuf>> {
        if let Some(mut recorder) = self.recorder.take() {
            return recorder.stop().await;
        }
        Ok(None)
    }
}
```

## Performance Notes

- **Memory**: ~32 KB/second
- **CPU**: 2-5% single core
- **Latency**: < 64ms per block
- **File write**: 50-200ms

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "No input device" | Check microphone permissions in System Preferences |
| "Already in progress" | Call `stop()` before calling `start()` again |
| Empty recording | Check microphone isn't muted, adjust `silence_db` |
| Auto-stops too soon | Increase `hang_sec` or disable `auto_silence` |
| Auto-stops too late | Decrease `hang_sec` or lower `silence_db` |

## Full Documentation

See `/Users/maciejgad/hosted/Loctree-Repos/Codescribe/codescribe-rs/docs/AUDIO_MODULE.md` for:
- Detailed API reference
- Architecture overview
- Performance characteristics
- Python compatibility
- Roadmap and limitations

---

**Created by M&K (c)2025 The LibraxisAI Team**
