use std::io::Write;
use std::time::{Duration, Instant};

#[test]
fn startup_returns_before_large_bus_compaction_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("transcript-events.jsonl");
    let row = format!(
        "{{\"schema\":\"codescribe.transcript-evidence.v1\",\"emitted_at\":\"2020-01-01T00:00:00Z\",\"padding\":\"{}\"}}\n",
        "x".repeat(2048)
    );
    let mut out = std::fs::File::create(&path).unwrap();
    while out.metadata().unwrap().len()
        < codescribe::presentation::transcript_bus_maintenance::COMPACTION_THRESHOLD_BYTES
    {
        out.write_all(row.as_bytes()).unwrap();
    }
    loop {
        out.flush().unwrap();
        let started = Instant::now();
        let _ = codescribe::presentation::transcript_bus_maintenance::bus_status(&path).unwrap();
        if started.elapsed() >= Duration::from_secs(1) {
            break;
        }
        let size = out.metadata().unwrap().len();
        while out.metadata().unwrap().len() < size * 2 {
            out.write_all(row.as_bytes()).unwrap();
        }
    }
    drop(out);
    let original_len = std::fs::metadata(&path).unwrap().len();
    // This test binary owns its environment and never points at Founder's data.
    unsafe {
        std::env::set_var("CODESCRIBE_TRANSCRIPT_BUS_PATH", &path);
        std::env::set_var("CODESCRIBE_DATA_DIR", dir.path());
    }
    let started = Instant::now();
    codescribe_ffi::start_application_runtime().unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(800),
        "startup waited {elapsed:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::metadata(&path).unwrap().len() == original_len && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(std::fs::metadata(&path).unwrap().len() < original_len);
}
