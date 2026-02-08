//! Backpressure/audio-drop regression scaffolding (no Whisper model required).
//!
//! The live audio callback path uses a bounded channel and `try_send()` to avoid blocking
//! the audio thread; under load, this intentionally drops audio rather than stalling.
//! This test provides a hermetic harness that exercises that behavior.

use std::time::Duration;

use tokio::sync::mpsc;

#[tokio::test(flavor = "current_thread")]
async fn test_try_send_drops_under_backpressure_scaffold() {
    let (tx, mut rx) = mpsc::channel::<usize>(1);

    let consumer = tokio::spawn(async move {
        let mut received = 0usize;
        while rx.recv().await.is_some() {
            received += 1;
            // Simulate a slow downstream stage (e.g., STT).
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        received
    });

    let mut dropped = 0usize;
    for i in 0..1000usize {
        if tx.try_send(i).is_err() {
            dropped += 1;
        }
    }
    drop(tx);

    let received = consumer.await.expect("consumer task should complete");

    assert!(dropped > 0, "expected some drops under backpressure");
    assert!(received > 0, "consumer should receive at least one item");
    assert!(
        received < 1000,
        "with a 1-slot channel and slow consumer, we should not receive all items"
    );
    assert!(
        received + dropped <= 1000,
        "each produced item is either received or dropped"
    );
}
