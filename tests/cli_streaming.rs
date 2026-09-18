//! Real decoder witness: CODESCRIBE_STREAM_TEST_WAV must name a multi-window WAV.
//! cargo test --test cli_streaming -- --ignored --nocapture
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

#[test]
#[ignore = "needs a local Whisper model and CODESCRIBE_STREAM_TEST_WAV longer than one window"]
fn file_stream_emits_before_completion_and_matches_the_plain_verdict() {
    let wav = std::env::var("CODESCRIBE_STREAM_TEST_WAV").expect("set the private test WAV path");
    let binary = env!("CARGO_BIN_EXE_codescribe");
    let mut child = Command::new(binary)
        .args([
            "transcribe",
            "--no-bus",
            "--stream",
            "--language",
            "pl",
            &wav,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start CLI");
    let mut output = BufReader::new(child.stdout.take().expect("stdout"));
    let mut text = String::new();
    assert!(output.read_line(&mut text).expect("first streamed line") > 0);
    assert!(
        child.try_wait().expect("poll child").is_none(),
        "first text arrived only after exit"
    );
    let first_text_at = std::time::Instant::now();
    output.read_to_string(&mut text).expect("remaining stream");
    assert!(child.wait().expect("exit").success());
    assert!(
        first_text_at.elapsed() >= std::time::Duration::from_millis(250),
        "output was dumped at completion rather than between decode windows"
    );
    let plain = Command::new(binary)
        .args(["transcribe", "--no-bus", "--language", "pl", &wav])
        .output()
        .expect("plain CLI");
    assert!(plain.status.success());
    let plain = String::from_utf8(plain.stdout).expect("UTF-8 transcript");
    assert_eq!(
        text.split_whitespace().collect::<Vec<_>>(),
        plain.split_whitespace().collect::<Vec<_>>()
    );
}
