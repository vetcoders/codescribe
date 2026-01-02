# Controller Implementation Notes

## Problem: cpal Stream is not Send

The `Recorder` struct contains a `cpal::Stream` which has callbacks that are not Send-safe. This prevents us from using `Arc<Mutex<Recorder>>` across tokio spawn boundaries.

## Solution Options

### Option 1: Local Recorder with Message Passing (RECOMMENDED)
Store the Recorder directly in RecordingController (not Arc<Mutex>) and use channels for communication:

```rust
pub struct RecordingController {
    recorder: Recorder,  // Not Arc, not Mutex
    // ... other fields
}

// In schedule_hold_start, don't move recorder into spawn:
let task = tokio::spawn(async move {
    // Just update state, don't touch recorder
    *state.write().await = State::RecHold;
});

// Then call recorder.start() AFTER the delay, from the main controller:
pub async fn check_and_start_recording(&mut self) {
    if *self.state.read().await == State::RecHold {
        self.recorder.start().await?;
    }
}
```

This requires polling or event-driven architecture.

### Option 2: spawn_blocking (Current Approach)
Move audio operations to blocking threads:

```rust
let audio_path = tokio::task::spawn_blocking(move || {
    let mut recorder = Recorder::new()?;
    recorder.start()?;
    // Wait for stop signal
    recorder.stop()
}).await??;
```

This is simpler but requires careful lifecycle management.

### Option 3: Channel-based Recorder Service
Create a dedicated thread that owns the Recorder and communicates via mpsc channels:

```rust
enum RecorderCommand {
    Start,
    Stop(oneshot::Sender<PathBuf>),
}

// Recorder service runs in its own thread
std::thread::spawn(move || {
    let mut recorder = Recorder::new().unwrap();
    for cmd in rx {
        match cmd {
            RecorderCommand::Start => recorder.start().await,
            RecorderCommand::Stop(tx) => {
                let path = recorder.stop().await;
                tx.send(path);
            }
        }
    }
});
```

## Current Status

The controller is currently using **Option 2** (spawn_blocking approach) in process_recording, but the delayed start in schedule_hold_start still has Send issues.

## Recommended Fix

For the immediate fix, we need to:

1. Create the Recorder only when needed (in process_recording)
2. Use a flag or channel to signal "start recording now" from schedule_hold_start
3. Have a separate method that polls this flag and calls recorder.start()

OR

Simply accept that we create/destroy Recorder instances for each recording session (lightweight enough with cpal).
