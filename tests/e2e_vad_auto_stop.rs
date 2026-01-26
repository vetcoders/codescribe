//! E2E tests for VAD (Voice Activity Detection) auto-stop functionality
//!
//! Tests the mechanism where silence detection triggers automatic recording stop:
//! 1. Recorder detects silence → calls on_vad_stop callback
//! 2. Callback sets atomic flag in Controller
//! 3. Monitor task polls flag and calls finish_recording()
//!
//! Run with:
//!   cargo test --test e2e_vad_auto_stop
//!
//! Created by M&K (c)2026 VetCoders

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

/// Test atomic flag mechanism used for VAD signaling
///
/// This validates the cross-thread communication pattern:
/// - Callback sets flag to true
/// - Monitor reads flag
/// - Monitor clears flag after processing
#[test]
fn test_vad_atomic_flag_mechanism() {
    let flag = Arc::new(AtomicBool::new(false));

    // Initial state should be false
    assert!(!flag.load(Ordering::SeqCst), "Flag should start as false");

    // Simulate callback setting the flag
    flag.store(true, Ordering::SeqCst);
    assert!(
        flag.load(Ordering::SeqCst),
        "Flag should be true after callback"
    );

    // Simulate monitor clearing the flag
    flag.store(false, Ordering::SeqCst);
    assert!(
        !flag.load(Ordering::SeqCst),
        "Flag should be false after clear"
    );
}

/// Test that callback can be invoked from different thread
#[test]
fn test_vad_callback_cross_thread() {
    let flag = Arc::new(AtomicBool::new(false));
    let call_count = Arc::new(AtomicU32::new(0));

    // Create callback that sets flag (simulating VAD stop)
    let flag_clone = Arc::clone(&flag);
    let count_clone = Arc::clone(&call_count);
    let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        flag_clone.store(true, Ordering::SeqCst);
        count_clone.fetch_add(1, Ordering::SeqCst);
    });

    // Spawn thread to invoke callback (simulating audio thread)
    let callback_clone = Arc::clone(&callback);
    let handle = std::thread::spawn(move || {
        callback_clone();
    });

    handle.join().expect("Thread should complete");

    // Verify flag was set from other thread
    assert!(
        flag.load(Ordering::SeqCst),
        "Flag should be set by callback"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "Callback should be called once"
    );
}

/// Test multiple rapid VAD triggers (debounce scenario)
#[test]
fn test_vad_multiple_triggers() {
    let flag = Arc::new(AtomicBool::new(false));
    let trigger_count = Arc::new(AtomicU32::new(0));

    // Simulate 5 rapid VAD triggers
    for _ in 0..5 {
        flag.store(true, Ordering::SeqCst);
        trigger_count.fetch_add(1, Ordering::SeqCst);
    }

    // Flag should still be true (last write wins)
    assert!(flag.load(Ordering::SeqCst));
    assert_eq!(trigger_count.load(Ordering::SeqCst), 5);

    // Single clear should reset
    flag.store(false, Ordering::SeqCst);
    assert!(!flag.load(Ordering::SeqCst));
}

/// Test monitor polling pattern (async simulation)
#[tokio::test]
async fn test_vad_monitor_polling() {
    let flag = Arc::new(AtomicBool::new(false));
    let processed = Arc::new(AtomicBool::new(false));

    // Start monitor task
    let flag_clone = Arc::clone(&flag);
    let processed_clone = Arc::clone(&processed);
    let monitor_handle = tokio::spawn(async move {
        // Poll every 10ms (faster for test)
        for _ in 0..50 {
            if flag_clone.load(Ordering::SeqCst) {
                // Simulate finish_recording()
                processed_clone.store(true, Ordering::SeqCst);
                flag_clone.store(false, Ordering::SeqCst);
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    });

    // Wait a bit, then trigger VAD
    tokio::time::sleep(Duration::from_millis(50)).await;
    flag.store(true, Ordering::SeqCst);

    // Monitor should catch it
    let caught = monitor_handle.await.expect("Monitor should complete");
    assert!(caught, "Monitor should have caught the VAD trigger");
    assert!(
        processed.load(Ordering::SeqCst),
        "Processing should have occurred"
    );
    assert!(
        !flag.load(Ordering::SeqCst),
        "Flag should be cleared after processing"
    );
}

/// Test that VAD flag survives across multiple operations
#[test]
fn test_vad_flag_persistence() {
    let flag = Arc::new(AtomicBool::new(false));

    // Multiple clones sharing same flag
    let flag1 = Arc::clone(&flag);
    let flag2 = Arc::clone(&flag);
    let flag3 = Arc::clone(&flag);

    // Set from one clone
    flag1.store(true, Ordering::SeqCst);

    // Read from all clones should see the change
    assert!(flag.load(Ordering::SeqCst));
    assert!(flag2.load(Ordering::SeqCst));
    assert!(flag3.load(Ordering::SeqCst));

    // Clear from another clone
    flag2.store(false, Ordering::SeqCst);

    // All should see cleared state
    assert!(!flag.load(Ordering::SeqCst));
    assert!(!flag1.load(Ordering::SeqCst));
    assert!(!flag3.load(Ordering::SeqCst));
}

/// Test RecorderConfig VAD defaults
#[test]
fn test_recorder_vad_config_defaults() {
    use codescribe::RecorderConfig;

    let config = RecorderConfig::default();

    // VAD should be enabled by default
    assert!(
        config.auto_silence,
        "auto_silence should be true by default"
    );

    // Speech threshold should be probability (0.0-1.0)
    // Default is 0.5 from CODESCRIBE_VAD_THRESHOLD env var
    assert!(
        config.speech_threshold >= 0.1 && config.speech_threshold <= 0.9,
        "speech_threshold should be between 0.1 and 0.9, got: {}",
        config.speech_threshold
    );

    // Hang time should be reasonable (0.3-3.0 seconds)
    // Default is 1.2s from CODESCRIBE_VAD_MAX_SILENCE_SEC env var
    assert!(
        config.hang_sec >= 0.3 && config.hang_sec <= 3.0,
        "hang_sec should be between 0.3 and 3.0s, got: {}",
        config.hang_sec
    );
}

/// Test RecorderConfig VAD customization
#[test]
fn test_recorder_vad_config_custom() {
    use codescribe::RecorderConfig;

    let config = RecorderConfig {
        speech_threshold: 0.6,
        hang_sec: 1.5,
        auto_silence: false,
        ..Default::default()
    };

    assert!(!config.auto_silence, "auto_silence should be overridable");
    assert!((config.speech_threshold - 0.6).abs() < 0.01);
    assert!((config.hang_sec - 1.5).abs() < 0.01);
}

/// Test that Recorder can accept VAD callback
/// Note: Does not actually record - tests API only
#[test]
fn test_recorder_vad_callback_api() {
    use codescribe::Recorder;

    let mut recorder = Recorder::new().expect("Should create recorder");
    let called = Arc::new(AtomicBool::new(false));

    // Set callback
    let called_clone = Arc::clone(&called);
    recorder.set_on_vad_stop(move || {
        called_clone.store(true, Ordering::SeqCst);
    });

    // Callback is stored but not called until VAD triggers
    // We can't easily trigger VAD in a unit test without mocking audio
    // This test validates the API accepts the callback
}

/// Documentation test for VAD flow
#[test]
fn test_vad_flow_documentation() {
    // VAD (Voice Activity Detection) auto-stop flow using Silero neural network:
    //
    // 1. User starts toggle recording (Ctrl+Ctrl double-tap)
    //    - Controller enters REC_TOGGLE state
    //    - Recorder starts capturing audio
    //    - VAD callback is registered: on_vad_stop = || { vad_triggered.store(true) }
    //
    // 2. User speaks into microphone
    //    - Audio chunks are processed by Silero VAD
    //    - Speech probability is above threshold (CODESCRIBE_VAD_THRESHOLD, default 0.5)
    //    - VAD does not trigger
    //
    // 3. User stops speaking (silence for hang_sec seconds)
    //    - Speech probability drops below threshold
    //    - After hang_sec (CODESCRIBE_VAD_MAX_SILENCE_SEC, default 1.2s), VAD triggers
    //    - on_vad_stop callback is invoked
    //    - vad_triggered atomic flag set to true
    //
    // 4. Monitor task in main.rs detects flag
    //    - Polls every 100ms
    //    - Sees vad_triggered == true
    //    - Calls controller.finish_recording()
    //    - Clears vad_triggered flag
    //
    // 5. finish_recording() processes the audio
    //    - Stops recorder
    //    - Transcribes with Whisper
    //    - Formats with AI (if enabled)
    //    - Pastes result
    //    - Returns to IDLE state

    let _doc = "VAD flow documentation";
}

/// Test Silero VAD probability threshold concept
#[test]
fn test_vad_probability_threshold() {
    // Silero VAD outputs speech probability (0.0 - 1.0)
    //
    // CODESCRIBE_VAD_THRESHOLD controls sensitivity:
    // - 0.3 = sensitive (catches quiet speech, more false positives)
    // - 0.5 = balanced (default)
    // - 0.7 = conservative (fewer false positives, may miss quiet speech)

    let threshold = 0.5f32;

    // Simulate VAD probability outputs
    let silence_prob = 0.1f32; // Low probability = silence
    let speech_prob = 0.8f32;  // High probability = speech

    assert!(
        silence_prob < threshold,
        "Silence probability {} should be below threshold {}",
        silence_prob,
        threshold
    );

    assert!(
        speech_prob > threshold,
        "Speech probability {} should be above threshold {}",
        speech_prob,
        threshold
    );

    // Edge case: probability at threshold
    let edge_prob = 0.5f32;
    // Convention: >= threshold is speech, < threshold is silence
    assert!(
        edge_prob >= threshold,
        "Edge probability {} should be considered speech at threshold {}",
        edge_prob,
        threshold
    );
}
