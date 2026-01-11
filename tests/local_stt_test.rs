#[cfg(target_os = "macos")]
mod macos_local_stt_tests {
    use codescribe::local_stt::LocalWhisperEngine;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn model_dir() -> Option<PathBuf> {
        let model_path = std::env::var("CODESCRIBE_TEST_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("models/whisper-large-v3-mlx-q8"));
        if model_path.exists() {
            Some(model_path)
        } else {
            None
        }
    }

    fn audio_path() -> Option<PathBuf> {
        let path = std::env::var("CODESCRIBE_TEST_AUDIO_PATH")
            .ok()
            .map(PathBuf::from)?;
        if path.exists() { Some(path) } else { None }
    }

    fn sequential_runs() -> usize {
        std::env::var("CODESCRIBE_TEST_SEQUENTIAL_RUNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(10)
            .max(1)
    }

    fn memory_runs() -> usize {
        std::env::var("CODESCRIBE_TEST_MEMORY_RUNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20)
            .max(1)
    }

    fn max_memory_growth_bytes() -> u64 {
        std::env::var("CODESCRIBE_TEST_MEM_GROWTH_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(200_000_000)
    }

    fn rss_bytes_macos() -> Option<u64> {
        let pid = std::process::id().to_string();
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&output.stdout);
        let kb = s.trim().parse::<u64>().ok()?;
        Some(kb.saturating_mul(1024))
    }

    fn fixtures() -> Option<(PathBuf, PathBuf)> {
        let model = model_dir()?;
        let audio = audio_path()?;
        Some((model, audio))
    }

    #[test]
    fn test_local_stt_instantiation() {
        let Some(model_path) = model_dir() else {
            eprintln!(
                "CODESCRIBE_TEST_MODEL_DIR not set (or missing); skipping instantiation test"
            );
            return;
        };

        let engine = LocalWhisperEngine::new(&model_path);
        assert!(
            engine.is_ok(),
            "Failed to instantiate engine: {:?}",
            engine.err()
        );
    }

    #[test]
    fn test_language_autodetect_smoke() {
        let Some((model_path, audio_path)) = fixtures() else {
            eprintln!(
                "CODESCRIBE_TEST_MODEL_DIR and/or CODESCRIBE_TEST_AUDIO_PATH not set; skipping"
            );
            return;
        };

        let mut engine = LocalWhisperEngine::new(&model_path).unwrap();
        let transcribe = engine.transcribe_file_with_language(&audio_path, None);
        assert!(
            transcribe.is_ok(),
            "Autodetect transcription failed: {:?}",
            transcribe
        );

        // We primarily care that autodetect path doesn't crash; enforce expected language only if provided.
        let expected = std::env::var("CODESCRIBE_TEST_EXPECT_LANG").ok();
        if let Some(expected) = expected {
            let detected = engine.detect_language_file(&audio_path).unwrap();
            assert_eq!(detected, expected);
        }
    }

    #[test]
    fn test_model_stays_loaded_sequential() {
        let Some((model_path, audio_path)) = fixtures() else {
            eprintln!(
                "CODESCRIBE_TEST_MODEL_DIR and/or CODESCRIBE_TEST_AUDIO_PATH not set; skipping"
            );
            return;
        };
        let runs = sequential_runs();

        let mut engine = LocalWhisperEngine::new(&model_path).unwrap();
        for i in 0..runs {
            let result = engine.transcribe_file_with_language(&audio_path, Some("pl"));
            assert!(result.is_ok(), "Transcription {} failed: {:?}", i, result);
        }
    }

    #[test]
    fn test_memory_stable_rss() {
        let Some((model_path, audio_path)) = fixtures() else {
            eprintln!(
                "CODESCRIBE_TEST_MODEL_DIR and/or CODESCRIBE_TEST_AUDIO_PATH not set; skipping"
            );
            return;
        };

        let mut engine = LocalWhisperEngine::new(&model_path).unwrap();

        // Warm-up to stabilize one-time allocations.
        engine
            .transcribe_file_with_language(&audio_path, Some("pl"))
            .unwrap();
        let baseline = rss_bytes_macos().unwrap_or(0);

        for _ in 0..memory_runs() {
            let _ = engine.transcribe_file_with_language(&audio_path, Some("pl"));
        }

        let after = rss_bytes_macos().unwrap_or(0);
        if baseline > 0 && after > 0 {
            let growth = after.saturating_sub(baseline);
            assert!(
                growth < max_memory_growth_bytes(),
                "Memory grew by {} bytes (baseline={}, after={})",
                growth,
                baseline,
                after
            );
        }
    }

    #[tokio::test]
    async fn test_concurrent_transcriptions() {
        let Some((model_path, audio_path)) = fixtures() else {
            eprintln!(
                "CODESCRIBE_TEST_MODEL_DIR and/or CODESCRIBE_TEST_AUDIO_PATH not set; skipping"
            );
            return;
        };

        let engine = Arc::new(Mutex::new(LocalWhisperEngine::new(&model_path).unwrap()));
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let engine = engine.clone();
                let audio_path = audio_path.clone();
                tokio::spawn(async move {
                    let mut guard = engine.lock().await;
                    guard.transcribe_file_with_language(&audio_path, Some("pl"))
                })
            })
            .collect();

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_decoding_params_instantiation() {
        // Just verify we can set the params on the engine struct
        let Some(model_path) = model_dir() else {
            return;
        };
        let mut engine = LocalWhisperEngine::new(&model_path).unwrap();

        // Modify params
        engine.decoding_params.temperature = 0.5;
        engine.decoding_params.suppress_blank = false;
        engine.decoding_params.no_speech_threshold = 0.9;
        engine.decoding_params.compression_ratio_threshold = 10.0;
        engine.decoding_params.logprob_threshold = -5.0;

        // If we have audio, run it
        if let Some(audio_path) = audio_path() {
            let result = engine.transcribe_file_with_language(&audio_path, Some("en"));
            // We don't assert success strongly as params might degrade performance, but it should not crash
            if result.is_err() {
                eprintln!(
                    "Transcription with custom params failed: {:?}",
                    result.err()
                );
            }
        }
    }
}
