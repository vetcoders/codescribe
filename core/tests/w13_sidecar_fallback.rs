use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use codescribe_core::stt::tail_provider::{
    FakeTailProvider, RemoteTailProvider, STT_SIDECAR_TOKEN_ENV, SidecarTailProvider,
    TailEvidenceSource, TailEvidenceStability, TailProvider, TailProviderEvidence,
    TailProviderFailureKind, TailProviderId, TailProviderPayload, TailProviderRequest,
    TailRequestIdentity, TailSampleRange, TailTimingQuality, TimedTailSegment,
    transcribe_with_fallback,
};

fn request(request_id: u64, sample_start: u64) -> TailProviderRequest {
    TailProviderRequest {
        identity: TailRequestIdentity {
            request_id,
            range: TailSampleRange {
                session: "sidecar-take".to_string(),
                capture_epoch: 9,
                sample_start,
                sample_end: sample_start + 320,
            },
        },
        sample_rate: 16_000,
        language: Some("pl-PL".to_string()),
    }
}

fn fake_payload(request: &TailProviderRequest, text: &str) -> TailProviderPayload {
    TailProviderPayload {
        identity: request.identity.clone(),
        text: text.to_string(),
        segments: vec![TimedTailSegment {
            text: text.to_string(),
            range: request.identity.range.clone(),
        }],
        avg_logprob: Some(-0.1),
        compression_ratio: Some(1.0),
        quality_gate_dropped: false,
        provider_id: TailProviderId::Fake,
        elapsed_ms: 3,
        evidence: TailProviderEvidence {
            source: TailEvidenceSource::Whisper,
            revision: Some("fake-sidecar-r1".to_string()),
            stability: TailEvidenceStability::Final,
            timing_quality: TailTimingQuality::Synthetic,
            avg_logprob: Some(-0.1),
        },
    }
}

#[test]
fn w13_sidecar_fallback_receipts() {
    let remote_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("remote listener");
    let remote_address = remote_listener.local_addr().expect("remote address");
    let remote_server = std::thread::spawn(move || {
        let (mut stream, _) = remote_listener.accept().expect("accept remote request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("remote read timeout");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let read = stream.read(&mut buffer).expect("read multipart request");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n")
                && String::from_utf8_lossy(&bytes).contains("response_format")
            {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&bytes);
        assert!(request_text.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
        let request_lower = request_text.to_ascii_lowercase();
        assert!(!request_lower.contains("x-api-key:"));
        assert!(!request_lower.contains("authorization:"));
        assert!(request_text.contains("verbose_json"));
        let body = r#"{"text":"remote-window","segments":[{"text":"remote-window","start":0.0,"end":0.02}],"avg_logprob":-0.2,"compression_ratio":1.1}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write remote response");
    });
    let remote_request = request(0, 0);
    let remote = RemoteTailProvider::new(
        format!("http://{remote_address}/v1/audio/transcriptions"),
        "",
    )
    .expect("remote provider");
    let remote_payload = remote
        .transcribe(&remote_request, &vec![0.0; 320])
        .expect("multipart remote window");
    assert_eq!(remote_payload.provider_id, TailProviderId::Remote);
    assert_eq!(remote_payload.text, "remote-window");
    assert_eq!(remote_payload.segments[0].range.sample_end, 320);
    remote_server.join().expect("remote server");

    let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve port");
    let address = reservation.local_addr().expect("reserved address");
    drop(reservation);
    let token = "a".repeat(64);
    let first_request = request(1, 0);
    let first_payload = fake_payload(&first_request, "sidecar-window");
    let fixture = tempfile::NamedTempFile::new().expect("fixture file");
    std::fs::write(
        fixture.path(),
        serde_json::to_vec(&first_payload).expect("serialize fixture"),
    )
    .expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_codescribe-stt-sidecar"))
        .arg("--bind")
        .arg(address.to_string())
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .arg("--fake-payload")
        .arg(fixture.path())
        .env(STT_SIDECAR_TOKEN_ENV, &token)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn real sidecar binary");

    let deadline = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_err() {
        assert!(Instant::now() < deadline, "sidecar did not become ready");
        std::thread::sleep(Duration::from_millis(20));
    }

    // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket -- test exercises the production loopback-only exception.
    let endpoint = format!("ws://{address}/tail");
    let sidecar = SidecarTailProvider::new(endpoint, token).expect("sidecar client");
    let first_fallback = FakeTailProvider::new(first_payload).expect("first fallback");
    let pcm = vec![0.0; 320];
    let direct = sidecar
        .transcribe(&first_request, &pcm)
        .expect("real sidecar transport serves the first window");
    assert_eq!(direct.provider_id, TailProviderId::Sidecar);
    let first = transcribe_with_fallback(
        &sidecar,
        &first_fallback,
        TailProviderFailureKind::Unavailable,
        &first_request,
        &pcm,
    )
    .expect("first sidecar window");
    assert_eq!(first.receipt.requested_provider, TailProviderId::Sidecar);
    assert_eq!(first.receipt.served_provider, TailProviderId::Sidecar);
    assert!(!first.receipt.fallback_used);
    assert!(
        !first.payload.text.is_empty(),
        "sidecar must return an applied candidate"
    );
    assert!(
        first.receipt.elapsed_ms < 1_000,
        "fake sidecar window should stay below the delivery bar"
    );
    println!(
        "sidecar_receipt provider={} elapsed_ms={} applied_candidates=1",
        first.receipt.served_provider.as_str(),
        first.receipt.elapsed_ms
    );

    child.kill().expect("kill sidecar mid-take");
    child.wait().expect("reap killed sidecar");

    let second_request = request(2, 320);
    let second_payload = fake_payload(&second_request, "fallback-window");
    let second_fallback = FakeTailProvider::new(second_payload).expect("second fallback");
    let second = transcribe_with_fallback(
        &sidecar,
        &second_fallback,
        TailProviderFailureKind::Unavailable,
        &second_request,
        &pcm,
    )
    .expect("take continues through fallback");
    assert_eq!(second.receipt.requested_provider, TailProviderId::Sidecar);
    assert_eq!(second.receipt.served_provider, TailProviderId::Fake);
    assert!(second.receipt.fallback_used);
    assert_eq!(
        second.receipt.primary_failure,
        Some(TailProviderFailureKind::Unavailable)
    );
    assert_eq!(second.payload.text, "fallback-window");
    println!(
        "fallback_receipt requested={} served={} fallback_used={} take_completed=true",
        second.receipt.requested_provider.as_str(),
        second.receipt.served_provider.as_str(),
        second.receipt.fallback_used
    );
}
