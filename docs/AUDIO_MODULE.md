# Audio Module Documentation

## Overview

The `audio` module provides cross-platform audio recording functionality for CodeScribe, with built-in silence detection and automatic stop capabilities. It's designed to be a direct Rust port of the Python implementation, maintaining feature parity and algorithm compatibility.

## Architecture

### Key Components

1. **Recorder** - Main recording interface
2. **RecorderConfig** - Configuration parameters
3. **RecorderDiagnostics** - Recording statistics and metrics

### Dependencies

- `cpal` - Cross-platform audio I/O
- `hound` - WAV file encoding
- `tokio` - Async runtime for concurrent audio processing

## Features

### Core Functionality

- **Audio Capture**: 16kHz mono 16-bit signed integer recording (Whisper standard)
- **Silence Detection**: RMS-based silence detection with configurable threshold
- **Auto-Stop**: Automatic recording termination after configurable hang time
- **Live Snapshots**: Save buffer snapshots without stopping recording
- **Diagnostics**: Detailed recording metrics (frames, bytes, duration)

### Silence Detection Algorithm

The module uses Root Mean Square (RMS) amplitude calculation to detect silence:

```rust
// Calculate RMS amplitude
let rms_amplitude = sqrt(mean(samples^2))

// Convert to dBFS (decibels relative to full scale)
let rms_db = 20 * log10(rms_amplitude + epsilon)

// Check against threshold
if rms_db < SILENCE_DB {
    // Accumulate silent frames
}
```

**Default Values:**
- Silence threshold: `-45 dB` (configurable via `SILENCE_DB` env var)
- Hang time: `0.8 seconds` (configurable via `SILENCE_HANG_SEC` env var)
- Auto-silence: `enabled` (configurable via `AUTO_SILENCE` env var)

## Usage

### Basic Recording with Auto-Silence

```rust
use codescribe::audio::Recorder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut recorder = Recorder::new()?;

    // Start recording
    recorder.start().await?;

    // Recording will auto-stop after silence detected
    // or call stop() manually
    if let Some(path) = recorder.stop().await? {
        println!("Recording saved to: {:?}", path);
        println!("Duration: {:.2}s", recorder.last_duration());
    }

    Ok(())
}
```

### Custom Configuration

```rust
use codescribe::audio::{Recorder, RecorderConfig};

let config = RecorderConfig {
    sample_rate: 16000,
    channels: 1,
    silence_db: -50.0,      // More sensitive
    hang_sec: 1.5,           // Longer hang time
    auto_silence: true,
    block_size: 1024,
};

let mut recorder = Recorder::with_config(config)?;
recorder.start().await?;
```

### Live Streaming with Snapshots

```rust
let mut recorder = Recorder::new()?;
recorder.start().await?;

// Periodically save snapshots while recording
loop {
    tokio::time::sleep(Duration::from_secs(1)).await;

    if let Some(snapshot_path) = recorder.snapshot_wav(0.8)? {
        // Send snapshot_path to transcription service
        transcribe_chunk(snapshot_path).await?;
    }

    if !recorder.is_recording() {
        break;
    }
}

// Get final recording
if let Some(final_path) = recorder.stop().await? {
    println!("Final recording: {:?}", final_path);
}
```

### Diagnostics

```rust
recorder.stop().await?;

let diagnostics = recorder.diagnostics();
println!("Frames captured: {}", diagnostics.frames);
println!("Total bytes: {}", diagnostics.bytes);
println!("Duration: {:.3}s", diagnostics.duration_sec);
println!("Chunks processed: {}", diagnostics.chunks);

// Export as JSON
println!("{}", serde_json::to_string_pretty(&diagnostics.as_json())?);
```

## Configuration

### Environment Variables

The module respects the following environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SILENCE_DB` | `-45.0` | Silence threshold in dB |
| `SILENCE_HANG_SEC` | `0.8` | Hang time before auto-stop (seconds) |
| `AUTO_SILENCE` | `1` | Enable/disable auto-silence detection |

Example:
```bash
export SILENCE_DB=-50.0
export SILENCE_HANG_SEC=1.2
export AUTO_SILENCE=0  # Disable auto-silence
```

### RecorderConfig Fields

```rust
pub struct RecorderConfig {
    pub sample_rate: u32,      // Sample rate in Hz (default: 16000)
    pub channels: u16,          // Number of channels (default: 1)
    pub silence_db: f32,        // Silence threshold in dB (default: -45.0)
    pub hang_sec: f32,          // Hang time in seconds (default: 0.8)
    pub auto_silence: bool,     // Enable auto-silence (default: true)
    pub block_size: usize,      // Audio block size (default: 1024)
}
```

## Audio Format Specifications

### Output WAV Files

All WAV files produced by this module use the following specification:

- **Sample Rate**: 16000 Hz (16 kHz)
- **Channels**: 1 (Mono)
- **Bit Depth**: 16-bit
- **Format**: Signed Integer (PCM)
- **Encoding**: Little-endian

This format is optimized for speech-to-text services like OpenAI Whisper.

### File Locations

Temporary WAV files are created in the system temp directory:

```rust
// Recording files
/tmp/codescribe_recording_{timestamp}.wav

// Snapshot files
/tmp/codescribe_snapshot_{timestamp}.wav
```

## Error Handling

The module uses `anyhow::Result` for error handling:

```rust
use anyhow::Context;

// Common error scenarios
match recorder.start().await {
    Ok(_) => println!("Recording started"),
    Err(e) => {
        if e.to_string().contains("already in progress") {
            println!("Already recording!");
        } else if e.to_string().contains("No input device") {
            println!("No microphone found!");
        } else {
            println!("Error: {}", e);
        }
    }
}
```

### Common Errors

| Error | Cause | Solution |
|-------|-------|----------|
| "Recording is already in progress" | `start()` called twice | Call `stop()` first |
| "No input device available" | No microphone detected | Check system audio settings |
| "Failed to build input stream" | Audio device busy | Close other audio applications |
| "No audio data captured" | Empty buffer on stop | Check microphone permissions |

## Thread Safety

- **Recorder**: Not `Send` or `Sync` (use in single async task)
- **RecorderConfig**: `Clone` + `Send` + `Sync`
- **RecorderDiagnostics**: `Clone` + `Send` + `Sync`

The internal audio buffer uses `Arc<Mutex<Vec<i16>>>` for thread-safe concurrent access.

## Performance Characteristics

### Memory Usage

- **Buffer Growth**: Linear with recording duration
- **16kHz mono**: ~32 KB/second (16,000 samples * 2 bytes)
- **1 minute**: ~1.9 MB
- **5 minutes**: ~9.6 MB

### CPU Usage

- **Idle**: < 1% CPU (waiting for audio input)
- **Active Recording**: 2-5% CPU (single core)
- **RMS Calculation**: O(n) per block (1024 samples)

### Latency

- **Block Processing**: < 64ms (1024 samples @ 16kHz)
- **Silence Detection**: Real-time (per-block)
- **File Writing**: 50-200ms (depends on buffer size)

## Testing

Run the test suite:

```bash
cd codescribe-rs
cargo test audio
```

Run the example:

```bash
cargo run --example audio_example
```

### Unit Tests

The module includes tests for:

- ✅ RMS calculation accuracy
- ✅ Config default values
- ✅ Environment variable parsing
- ✅ Recorder initialization
- ✅ WAV file writing

## Python Compatibility

This Rust implementation maintains algorithm compatibility with the Python version:

| Feature | Python | Rust | Status |
|---------|--------|------|--------|
| Sample Rate | 16kHz | 16kHz | ✅ |
| Channels | Mono | Mono | ✅ |
| Bit Depth | 16-bit int | 16-bit i16 | ✅ |
| Silence Detection | RMS + dB | RMS + dB | ✅ |
| Auto-stop | HANG threshold | hang_sec | ✅ |
| Snapshots | `snapshot_wav()` | `snapshot_wav()` | ✅ |
| Diagnostics | `RecorderDiagnostics` | `RecorderDiagnostics` | ✅ |
| Env Config | `SILENCE_DB`, etc. | Same | ✅ |

## Roadmap

### Planned Enhancements

- [ ] Support for multiple sample rates (8kHz, 22.05kHz, 44.1kHz)
- [ ] Stereo recording support
- [ ] Noise reduction preprocessing
- [ ] VAD (Voice Activity Detection) alternative to RMS
- [ ] Dynamic threshold adjustment based on ambient noise
- [ ] Circular buffer for memory-constrained environments
- [ ] MP3/FLAC output formats

### Known Limitations

- macOS only (as per project scope)
- No real-time visualization (consider separate module)
- No automatic device switching on disconnect
- Fixed 16-bit depth (no 24/32-bit support)

## License

BSD-4-Clause

Copyright (c) 2025 Loctree

---

**Created by M&K (c)2025 The LibraxisAI Team**

Co-Authored-By: [Maciej](void@div0.space) & [Klaudiusz](the1st@whoai.am)
