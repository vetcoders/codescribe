use std::sync::mpsc;
use std::time::Duration;

#[test]
fn startup_returns_while_compaction_thread_is_held_at_entry() {
    let home = tempfile::tempdir().unwrap();
    let prior_home = std::env::var_os("HOME");
    let prior_isolation = std::env::var_os("CODESCRIBE_TEST_ISOLATION");
    struct Restore(Option<std::ffi::OsString>, Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(value) => std::env::set_var("HOME", value),
                    None => std::env::remove_var("HOME"),
                }
                match &self.1 {
                    Some(value) => std::env::set_var("CODESCRIBE_TEST_ISOLATION", value),
                    None => std::env::remove_var("CODESCRIBE_TEST_ISOLATION"),
                }
            }
        }
    }
    let _restore = Restore(prior_home, prior_isolation);
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("CODESCRIBE_TEST_ISOLATION", "1");
    }
    assert_eq!(
        codescribe_core::config::install_interlock_path(),
        home.path().join(".codescribe/install-runtime.lock")
    );
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
    assert!(
        home.path()
            .join(".codescribe/install-runtime.lock")
            .exists()
    );
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "compaction must still be held after startup returned"
    );
    release_tx.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
}
