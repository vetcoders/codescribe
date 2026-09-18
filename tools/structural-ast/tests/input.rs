use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn malformed_input_is_nonzero_and_never_emits_acceptance() {
    for input in [
        "",
        "{",
        "null",
        r#"{"schema":"x","bodies":[],"command":"sh"}"#,
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_codescribe-structural-ast"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start neutral parser");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write JSON");
        let output = child.wait_with_output().expect("parser completion");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
