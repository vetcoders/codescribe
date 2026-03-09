//! Backpressure/audio-drop regression scaffolding (no Whisper model required).
//!
//! The live audio callback path uses a bounded channel and `try_send()` to avoid blocking
//! the audio thread; under load, this intentionally drops audio rather than stalling.
//! This test provides a hermetic harness that exercises that behavior.

use std::time::Duration;

use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_try_send_drops_under_backpressure_scaffold() {
    let (tx, mut rx) = mpsc::channel::<usize>(1);

    let consumer = tokio::spawn(async move {
        let mut received = 0usize;
        while rx.recv().await.is_some() {
            received += 1;
            // Simulate a slow downstream stage (e.g., STT).
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        received
    });

    // Producer: burst-sends 1000 items with periodic yields so the consumer
    // actually runs concurrently on the multi-thread runtime.
    let mut dropped = 0usize;
    for i in 0..1000usize {
        if tx.try_send(i).is_err() {
            dropped += 1;
        }
        // Yield every 50 items so the consumer has a chance to drain.
        if i % 50 == 0 {
            tokio::task::yield_now().await;
        }
    }
    drop(tx);

    let received = consumer.await.expect("consumer task should complete");

    assert!(dropped > 0, "expected some drops under backpressure");
    assert!(
        received >= 1,
        "consumer should receive at least one item (got {received})"
    );
    assert!(
        received < 1000,
        "with a 1-slot channel and slow consumer, we should not receive all items (got {received})"
    );
    // Invariant: every item is either received or dropped.
    assert_eq!(
        received + dropped,
        1000,
        "received ({received}) + dropped ({dropped}) should equal total produced"
    );
}
