use super::wait_with_timeout;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn synthetic_child(script: &str) -> Child {
    Command::new("/usr/bin/python3")
        .args(["-c", script])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn isolated synthetic process group")
}

#[test]
fn exited_wrapper_cannot_leave_descendant_pipe_readers() {
    let mut child = synthetic_child(
        r#"
import os, signal, sys, time
if os.fork() == 0:
    signal.alarm(6)
    time.sleep(20)
else:
    print('wrapper-result', flush=True)
    print('wrapper-diagnostic', file=sys.stderr, flush=True)
    os._exit(0)
"#,
    );
    let started = Instant::now();
    let result = wait_with_timeout(&mut child, Duration::from_millis(200));
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "an exited wrapper left pipe readers waiting for the descendant alarm"
    );
    let output = result.expect("exited wrapper must release its descendant-held pipes");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"wrapper-result\n");
    assert_eq!(output.stderr, b"wrapper-diagnostic\n");
}

#[test]
fn timed_out_wrapper_cannot_leave_its_descendant_running() {
    let root = tempfile::tempdir().expect("fixture directory");
    let survived = root.path().join("descendant-survived");
    let mut child = synthetic_child(&format!(
        r#"
import os, signal, time
signal.alarm(3)
if os.fork() == 0:
    signal.alarm(3)
    time.sleep(0.7)
    open({:?}, 'w').write('still running')
    time.sleep(20)
else:
    time.sleep(20)
"#,
        survived.to_string_lossy()
    ));
    let started = Instant::now();
    let result = wait_with_timeout(&mut child, Duration::from_millis(200));
    assert!(result.is_err(), "hung wrapper must report a timeout");
    assert!(started.elapsed() < Duration::from_millis(1500));
    // A surviving descendant records its own liveness; no PID is signalled
    // after reap. Its alarm bounds even a failed cleanup implementation.
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        !survived.exists(),
        "timeout killed the wrapper but left the descendant running"
    );
}

#[test]
fn healthy_bridge_exit_preserves_complete_stdout_stderr_and_status() {
    let mut child =
        synthetic_child("import sys; print('complete'); print('diagnostic', file=sys.stderr)");
    let output =
        wait_with_timeout(&mut child, Duration::from_secs(2)).expect("healthy bridge result");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"complete\n");
    assert_eq!(output.stderr, b"diagnostic\n");
}
