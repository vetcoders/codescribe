use super::*;
const OPENAI: ProviderKind = ProviderKind::OpenAiResponses;
const XAI: ProviderKind = ProviderKind::XaiResponses;
fn options(vendor: ProviderKind) -> SpeechOptions {
    SpeechOptions {
        vendor,
        model: if vendor == OPENAI {
            openai::DEFAULT_TTS_MODEL
        } else {
            ""
        }
        .into(),
        voice: if vendor == OPENAI { "cedar" } else { "eve" }.into(),
        speed: 1.25,
    }
}
#[tokio::test]
async fn signed_in_account_wins_and_never_reads_key() {
    // Founder 2026-09-09 16:41: OAuth before the stored key.
    let auth = resolve_with(
        OPENAI,
        true,
        || async { Ok("oauth-test-token".into()) },
        || panic!("key must be ignored"),
    )
    .await
    .unwrap();
    assert_eq!(auth.bearer, "oauth-test-token");
    assert_eq!(auth.source, AuthSource::OAuth);
    let failure = resolve_with(
        OPENAI,
        true,
        || async { Err(SpeechError::Account("openai")) },
        || panic!("refresh failure cannot fall back"),
    )
    .await;
    assert!(matches!(failure, Err(SpeechError::Account("openai"))));
}
#[tokio::test]
async fn absent_account_uses_key_or_returns_typed_missing_error() {
    let auth = resolve_with(
        OPENAI,
        false,
        || async { panic!("OAuth must not run") },
        || Some("key-test-token".into()),
    )
    .await
    .unwrap();
    assert_eq!(auth.bearer, "key-test-token");
    assert_eq!(auth.source, AuthSource::ApiKey);
    assert!(matches!(
        resolve_with(XAI, false, || async { panic!() }, || None).await,
        Err(SpeechError::MissingCredentials("xai"))
    ));
}
#[test]
fn bodies_match_vendor_fixtures() {
    for (vendor, fixture) in [
        (
            OPENAI,
            include_str!("../vendors/fixtures/openai-speech.json"),
        ),
        (XAI, include_str!("../vendors/fixtures/xai-speech.json")),
    ] {
        let expected: Value = serde_json::from_str(fixture).unwrap();
        let body = options(vendor).request_body("Hello.");
        assert_eq!(body, expected);
        assert_eq!(
            serde_json::from_str::<Value>(&serde_json::to_string(&body).unwrap()).unwrap(),
            expected
        );
    }
}
#[test]
fn pcm_decode_preserves_signed_extrema_and_rejects_partial_frames() {
    assert_eq!(
        decode_pcm(&[0, 128, 0, 0, 255, 127]).unwrap(),
        vec![-1.0, 0.0, 32767.0 / 32768.0]
    );
    assert!(decode_pcm(&[1]).is_err());
    assert!(decode_pcm(&[]).is_err());
}
#[test]
fn unicode_chunking_is_lossless_and_capped_for_both_vendors() {
    let input = "Zażółć 🙂 gęślą. ".repeat(2000);
    for vendor in [OPENAI, XAI] {
        let cap = options(vendor).character_cap();
        let parts = chunks(&input, cap);
        assert!(parts.len() > 1);
        assert!(
            parts
                .iter()
                .all(|s| !s.is_empty() && s.chars().count() <= cap)
        );
        assert_eq!(parts.concat(), input);
        assert_eq!(chunks(&"🙂".repeat(cap + 1), cap).len(), 2);
    }
}
#[test]
fn credentials_only_match_exact_secure_vendor_origins() {
    assert_eq!(vendor_for_endpoint(openai::STT_ENDPOINT), Some(OPENAI));
    assert_eq!(vendor_for_endpoint("wss://api.x.ai/v1/stt"), Some(XAI));
    for url in [
        "http://api.x.ai/v1/stt",
        "https://api.x.ai.evil.test/v1/stt",
        "https://api.x.ai:444/v1/stt",
        "https://user@api.x.ai/v1/stt",
    ] {
        assert_eq!(vendor_for_endpoint(url), None);
    }
}
#[tokio::test]
async fn identical_input_hits_cache_and_changed_voice_misses() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .match_header("authorization", "Bearer test-key")
        .match_body(mockito::Matcher::Json(
            options(OPENAI).request_body("Hello."),
        ))
        .with_status(200)
        .with_body(vec![0, 128, 0, 0, 255, 127])
        .expect(1)
        .create_async()
        .await;
    let dir = tempfile::tempdir().unwrap();
    let auth = SpeechAuth {
        bearer: "test-key".into(),
        source: AuthSource::ApiKey,
    };
    let a = synthesize_at("Hello.", &options(OPENAI), &auth, &server.url(), dir.path())
        .await
        .unwrap();
    let b = synthesize_at("Hello.", &options(OPENAI), &auth, &server.url(), dir.path())
        .await
        .unwrap();
    assert!(!a.cached);
    assert!(b.cached);
    assert_eq!(a.samples, b.samples);
    assert_eq!(b.http_status, None);
    let mut changed = options(OPENAI);
    changed.voice = "marin".into();
    assert_ne!(
        changed.cache_key("Hello."),
        options(OPENAI).cache_key("Hello.")
    );
    mock.assert_async().await;
}
#[tokio::test]
async fn xai_json_envelope_and_raw_pcm_both_decode() {
    for json_envelope in [true, false] {
        let mut server = mockito::Server::new_async().await;
        let body = if json_envelope {
            br#"{"audio":"AIAAAP9/","content_type":"audio/pcm","duration":0.000125}"#.to_vec()
        } else {
            vec![0, 128, 0, 0, 255, 127]
        };
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header(
                "content-type",
                if json_envelope {
                    "application/json"
                } else {
                    "audio/pcm"
                },
            )
            .with_body(body)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let audio = synthesize_at(
            "Hello.",
            &options(XAI),
            &SpeechAuth {
                bearer: "test".into(),
                source: AuthSource::OAuth,
            },
            &server.url(),
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(audio.samples.len(), 3);
        assert_eq!(audio.samples[0], -1.0);
        mock.assert_async().await;
    }
}
#[tokio::test]
async fn http_errors_do_not_expose_response_secrets_or_populate_cache() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .with_status(401)
        .with_body("secret must never escape")
        .create_async()
        .await;
    let dir = tempfile::tempdir().unwrap();
    let result = synthesize_at(
        "Hello.",
        &options(OPENAI),
        &SpeechAuth {
            bearer: "test".into(),
            source: AuthSource::OAuth,
        },
        &server.url(),
        dir.path(),
    )
    .await;
    assert!(matches!(result, Err(SpeechError::Http(401))));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    mock.assert_async().await;
}
