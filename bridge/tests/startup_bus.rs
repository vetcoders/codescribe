use std::sync::mpsc;
use std::time::Duration;

#[test]
fn startup_returns_while_compaction_thread_is_held_at_entry() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let snapshot = codescribe_ffi::start_application_runtime_with_compaction(move || {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        finished_tx.send(()).unwrap();
    })
    .unwrap();

    assert_eq!(snapshot.state, "running");
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "compaction must still be held after startup returned"
    );
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
}
