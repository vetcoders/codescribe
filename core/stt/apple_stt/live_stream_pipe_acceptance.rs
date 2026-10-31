use super::{LiveStreamEvent, LiveStreamSession};
use crate::test_isolation::EnvGuard;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

/// The fake bridge uses the public request/header/event protocol. Its alarm
/// bounds a regression in the test process; no app or Speech service is used.
fn fake_bridge(root: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = root.join("synthetic-apple-bridge");
    let script = format!(
        "#!/usr/bin/python3\n\
         import json, os, signal, sys, time\n\
         signal.alarm(6)\n\
         json.loads(sys.stdin.buffer.readline())\n\
         json.loads(sys.stdin.buffer.readline())\n\
         print(json.dumps(dict(event='ready')), flush=True)\n\
         {body}\n"
    );
    fs::write(&path, script).expect("write synthetic bridge");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("make synthetic bridge executable");
    path
}

fn ready_session() -> LiveStreamSession {
    let session = LiveStreamSession::open(Some("pl"), 48_000).expect("open synthetic bridge");
    assert!(matches!(
        session.events_rx.recv_timeout(Duration::from_secs(2)),
        Ok(LiveStreamEvent::Ready)
    ));
    session
}

fn assert_failed_write_and_cleanup(mut session: LiveStreamSession) {
    // Far above any pipe's initial capacity. A non-reader and a slowly draining
    // reader both prevent this complete frame block from being delivered.
    let samples = vec![0.25; 1_048_576];
    let started = Instant::now();
    let result = session.write_pcm(&samples);
    let elapsed = started.elapsed();
    let finish_started = Instant::now();
    let finished = session.finish();
    let cleanup_elapsed = finish_started.elapsed();

    assert!(result.is_err(), "incomplete PCM must not report success");
    assert!(
        elapsed < Duration::from_secs(3),
        "PCM write waited for the fake bridge's six-second alarm: {elapsed:?}"
    );
    assert!(finished.is_ok(), "cleanup failed: {finished:?}");
    assert!(
        cleanup_elapsed < Duration::from_secs(1),
        "failed write left a live child or reader: {cleanup_elapsed:?}"
    );
}

#[test]
#[serial_test::serial]
fn nonreading_bridge_cannot_pin_pcm_write_or_cleanup() {
    let root = tempfile::tempdir().expect("test root");
    let bridge = fake_bridge(root.path(), "time.sleep(20)");
    let _bridge = EnvGuard::set("CODESCRIBE_APPLE_STT_BRIDGE", bridge);
    assert_failed_write_and_cleanup(ready_session());
}

#[test]
#[serial_test::serial]
fn intermittent_pipe_progress_cannot_extend_the_whole_write_budget() {
    let root = tempfile::tempdir().expect("test root");
    let bridge = fake_bridge(root.path(), "while os.read(0, 1024):\n    time.sleep(0.02)");
    let _bridge = EnvGuard::set("CODESCRIBE_APPLE_STT_BRIDGE", bridge);
    assert_failed_write_and_cleanup(ready_session());
}

#[test]
#[serial_test::serial]
fn backpressure_that_clears_preserves_all_f32le_frames_in_order() {
    let root = tempfile::tempdir().expect("test root");
    let received = root.path().join("received.pcm");
    let samples = [-0.5_f32, 0.25, 1.0, -1.0, 0.0]
        .into_iter()
        .cycle()
        .take(262_144)
        .collect::<Vec<_>>();
    let expected = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let bridge = fake_bridge(
        root.path(),
        &format!(
            "time.sleep(0.15)\n\
             data = sys.stdin.buffer.read({})\n\
             open({:?}, 'wb').write(data)\n\
             print(json.dumps(dict(event='partial', text='complete')), flush=True)",
            expected.len(),
            received.to_string_lossy()
        ),
    );
    let _bridge = EnvGuard::set("CODESCRIBE_APPLE_STT_BRIDGE", bridge);
    let mut session = ready_session();
    session
        .write_pcm(&samples)
        .expect("bounded backpressure clears");
    assert!(matches!(
        session.events_rx.recv_timeout(Duration::from_secs(2)),
        Ok(LiveStreamEvent::Partial { text, .. }) if text == "complete"
    ));
    assert_eq!(fs::read(received).expect("received bytes"), expected);
    assert_eq!(session.frames_written, samples.len() as u64);
    session.finish().expect("complete synthetic stream cleanup");
}

#[test]
#[serial_test::serial]
fn healthy_calls_do_not_spend_a_lifetime_backpressure_budget() {
    let root = tempfile::tempdir().expect("test root");
    // Each complete call has only 150 ms of ordinary backpressure. Across
    // twenty calls that exceeds two seconds, but it is a healthy stream.
    let bridge = fake_bridge(
        root.path(),
        "for index in range(20):\n\
             \x20   time.sleep(0.15)\n\
             \x20   data = sys.stdin.buffer.read(1048576)\n\
             \x20   assert data == b'\\x00\\x00\\x80\\x3e' * 262144\n\
             \x20   print(json.dumps(dict(event='partial', text=str(index))), flush=True)",
    );
    let _bridge = EnvGuard::set("CODESCRIBE_APPLE_STT_BRIDGE", bridge);
    let mut session = ready_session();
    let samples = vec![0.25_f32; 262_144];
    for index in 0..20 {
        session
            .write_pcm(&samples)
            .unwrap_or_else(|error| panic!("healthy write {index} was refused: {error:#}"));
        assert!(matches!(
            session.events_rx.recv_timeout(Duration::from_secs(2)),
            Ok(LiveStreamEvent::Partial { text, .. }) if text == index.to_string()
        ));
    }
    assert_eq!(session.frames_written, 20 * samples.len() as u64);
    session.finish().expect("healthy multi-call cleanup");
}

#[test]
#[serial_test::serial]
fn bridge_diagnostics_cannot_block_an_otherwise_healthy_pcm_stream() {
    let root = tempfile::tempdir().expect("test root");
    let bridge = fake_bridge(
        root.path(),
        "sys.stderr.buffer.write(b'diagnostic\\n' * 16384)\n\
         sys.stderr.buffer.flush()\n\
         data = sys.stdin.buffer.read(1048576)\n\
         assert data == b'\\x00\\x00\\x80\\x3e' * 262144\n\
         print(json.dumps(dict(event='partial', text='complete')), flush=True)",
    );
    let _bridge = EnvGuard::set("CODESCRIBE_APPLE_STT_BRIDGE", bridge);
    let mut session = ready_session();
    let samples = vec![0.25_f32; 262_144];
    session
        .write_pcm(&samples)
        .expect("diagnostic output must not block healthy PCM consumption");
    assert!(matches!(
        session.events_rx.recv_timeout(Duration::from_secs(2)),
        Ok(LiveStreamEvent::Partial { text, .. }) if text == "complete"
    ));
    assert_eq!(session.frames_written, samples.len() as u64);
    session.finish().expect("diagnostic stream cleanup");
}
